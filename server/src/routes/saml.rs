//! SAML 2.0 service-provider endpoints.
//!
//! - `GET /v1/auth/saml/discover` — `{enabled: bool}` for the web UI.
//! - `GET /v1/auth/saml/{tenant}/metadata` — SP metadata XML for IdP imports.
//! - `POST /v1/auth/saml/{tenant}/acs` — Assertion Consumer Service. Validates
//!   the IdP's signed assertion via xmlsec1 (through `samael`), checks
//!   Conditions, upserts the user, mints a session, redirects to the web's
//!   `/saml-return` with the session token.

use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use samael::metadata::EntityDescriptor;
use samael::service_provider::ServiceProviderBuilder;
use serde::{Deserialize, Serialize};
use std::env;
use std::str::FromStr;
use uuid::Uuid;

use crate::auth::hash_token;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

const ACS_PATH: &str = "/v1/auth/saml";
const SESSION_TTL_DAYS: i64 = 14;

// --- Discover -------------------------------------------------------------

#[derive(Serialize)]
pub struct SamlDiscovery {
    enabled: bool,
}

pub async fn discover(
    State(state): State<AppState>,
    tenant: TenantCtx,
) -> AppResult<Json<SamlDiscovery>> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT tenant_id FROM tenant_saml WHERE tenant_id = $1")
            .bind(tenant.tenant_id)
            .fetch_optional(state.db())
            .await?;
    Ok(Json(SamlDiscovery {
        enabled: row.is_some(),
    }))
}

// --- Metadata -------------------------------------------------------------

pub async fn metadata(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(_slug): Path<String>,
) -> AppResult<Response> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT sp_entity_id FROM tenant_saml WHERE tenant_id = $1")
            .bind(tenant.tenant_id)
            .fetch_optional(state.db())
            .await?;

    let sp_entity_id = match row {
        Some((Some(custom),)) => custom,
        Some((None,)) => default_sp_entity_id(&tenant.tenant_slug),
        None => {
            return Err(AppError::BadRequest(
                "SAML not configured for this tenant".into(),
            ))
        }
    };

    let acs_url = acs_url_for(&tenant.tenant_slug);
    let xml = render_sp_metadata(&sp_entity_id, &acs_url);
    let mut resp = (StatusCode::OK, xml).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/samlmetadata+xml; charset=utf-8"),
    );
    Ok(resp)
}

fn default_sp_entity_id(slug: &str) -> String {
    format!("urn:skill-pool:tenant:{slug}")
}

fn acs_url_for(tenant_slug: &str) -> String {
    let origin = env::var("SKILL_POOL_PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://skill-pool.example.com".to_string())
        .trim_end_matches('/')
        .to_string();
    format!("{origin}{ACS_PATH}/{tenant_slug}/acs")
}

fn web_return_url() -> String {
    env::var("SKILL_POOL_WEB_ORIGIN").unwrap_or_else(|_| "http://localhost:3000".to_string())
}

fn render_sp_metadata(sp_entity_id: &str, acs_url: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  entityID="{sp_entity_id}">
  <SPSSODescriptor AuthnRequestsSigned="false"
                   WantAssertionsSigned="true"
                   protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</NameIDFormat>
    <AssertionConsumerService index="0"
                              isDefault="true"
                              Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
                              Location="{acs_url}"/>
  </SPSSODescriptor>
</EntityDescriptor>
"#,
        sp_entity_id = xml_escape(sp_entity_id),
        acs_url = xml_escape(acs_url),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// --- ACS ------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    pub saml_response: String,
    #[serde(rename = "RelayState", default)]
    pub relay_state: Option<String>,
}

struct SamlConfig {
    idp_x509_cert: String,
    sp_entity_id: String,
    default_role: String,
}

pub async fn acs(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(_slug): Path<String>,
    Form(form): Form<AcsForm>,
) -> AppResult<Response> {
    let cfg = load_saml_config(&state, tenant.tenant_id, &tenant.tenant_slug).await?;

    let xml_bytes = base64::engine::general_purpose::STANDARD
        .decode(form.saml_response.as_bytes())
        .map_err(|e| AppError::BadRequest(format!("SAMLResponse base64: {e}")))?;

    let assertion = validate_response(&xml_bytes, &cfg, &tenant.tenant_slug).map_err(|e| {
        tracing::warn!(error = %e, tenant = %tenant.tenant_slug, "SAML assertion rejected");
        AppError::BadRequest(format!("SAML assertion rejected: {e}"))
    })?;

    let user_id = upsert_user(
        &state,
        &assertion.email,
        &assertion.subject,
        assertion.display_name.as_deref(),
    )
    .await?;
    ensure_membership(&state, tenant.tenant_id, user_id, &cfg.default_role).await?;
    let _ = crate::auth::apply_role_from_groups(
        state.db(),
        tenant.tenant_id,
        user_id,
        &assertion.groups,
    )
    .await?;
    let session_token = mint_session(&state, tenant.tenant_id, user_id).await?;

    let return_to = form
        .relay_state
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/saml-return", web_return_url()));

    let location = format!(
        "{}?token={}&tenant={}",
        return_to,
        urlencoding::encode(&session_token),
        urlencoding::encode(&tenant.tenant_slug),
    );
    Ok(Redirect::to(&location).into_response())
}

// --- Helpers --------------------------------------------------------------

async fn load_saml_config(
    state: &AppState,
    tenant_id: Uuid,
    tenant_slug: &str,
) -> AppResult<SamlConfig> {
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT idp_x509_cert, sp_entity_id, default_role \
         FROM tenant_saml WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(state.db())
    .await?;

    let (idp_x509_cert, sp_entity_id, default_role) =
        row.ok_or_else(|| AppError::BadRequest("SAML not configured for this tenant".into()))?;

    Ok(SamlConfig {
        idp_x509_cert,
        sp_entity_id: sp_entity_id.unwrap_or_else(|| default_sp_entity_id(tenant_slug)),
        default_role,
    })
}

struct ValidatedAssertion {
    email: String,
    subject: String,
    display_name: Option<String>,
    groups: Vec<String>,
}

fn validate_response(
    xml: &[u8],
    cfg: &SamlConfig,
    tenant_slug: &str,
) -> Result<ValidatedAssertion, String> {
    let idp_descriptor = build_idp_descriptor(&cfg.idp_x509_cert)?;

    let sp = ServiceProviderBuilder::default()
        .entity_id(cfg.sp_entity_id.clone())
        .acs_url(acs_url_for(tenant_slug))
        .idp_metadata(idp_descriptor)
        .build()
        .map_err(|e| format!("build SP: {e}"))?;

    let xml_str = std::str::from_utf8(xml).map_err(|e| format!("non-utf8 XML: {e}"))?;
    // samael's parse_xml_response validates the signature (against the IdP cert
    // in the descriptor we built) and returns the verified Assertion.
    let assertion = sp
        .parse_xml_response(xml_str, None)
        .map_err(|e| format!("parse / validate: {e}"))?;

    if let Some(conditions) = &assertion.conditions {
        if let Some(not_on_or_after) = &conditions.not_on_or_after {
            if Utc::now() >= *not_on_or_after {
                return Err(format!("assertion expired at {not_on_or_after}"));
            }
        }
    }

    let subject_value = assertion
        .subject
        .as_ref()
        .and_then(|s| s.name_id.as_ref())
        .map(|n| n.value.clone())
        .ok_or_else(|| "missing NameID in assertion".to_string())?;

    let email = first_attribute(&assertion, "email")
        .or_else(|| first_attribute(&assertion, "Email"))
        .or_else(|| first_attribute(&assertion, "mail"))
        .or_else(|| {
            if subject_value.contains('@') {
                Some(subject_value.clone())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            "no email in assertion (tried NameID + email/Email/mail attrs)".to_string()
        })?;

    let display_name = first_attribute(&assertion, "displayName")
        .or_else(|| first_attribute(&assertion, "name"))
        .or_else(|| {
            let g = first_attribute(&assertion, "givenName");
            let s = first_attribute(&assertion, "surname");
            match (g, s) {
                (Some(g), Some(s)) => Some(format!("{g} {s}")),
                (Some(g), None) => Some(g),
                (None, Some(s)) => Some(s),
                _ => None,
            }
        });

    let groups = all_attributes(&assertion, "groups")
        .into_iter()
        .chain(all_attributes(&assertion, "memberOf"))
        .chain(all_attributes(&assertion, "Role"))
        .collect();

    Ok(ValidatedAssertion {
        email: email.to_lowercase(),
        subject: subject_value,
        display_name,
        groups,
    })
}

/// Collect every `<AttributeValue>` for an attribute name.
fn all_attributes(assertion: &samael::schema::Assertion, name: &str) -> Vec<String> {
    assertion
        .attribute_statements
        .iter()
        .flatten()
        .flat_map(|s| s.attributes.iter())
        .filter(|a| a.name.as_deref() == Some(name))
        .flat_map(|a| a.values.iter())
        .filter_map(|v| v.value.clone())
        .collect()
}

fn first_attribute(assertion: &samael::schema::Assertion, name: &str) -> Option<String> {
    assertion
        .attribute_statements
        .iter()
        .flatten()
        .flat_map(|s| s.attributes.iter())
        .find(|a| a.name.as_deref() == Some(name))
        .and_then(|a| a.values.first())
        .and_then(|v| v.value.clone())
}

fn build_idp_descriptor(cert_pem: &str) -> Result<EntityDescriptor, String> {
    let body: String = cert_pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");

    let descriptor_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  xmlns:ds="http://www.w3.org/2000/09/xmldsig#"
                  entityID="placeholder">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol"
                   WantAuthnRequestsSigned="false">
    <KeyDescriptor use="signing">
      <ds:KeyInfo>
        <ds:X509Data>
          <ds:X509Certificate>{body}</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
                        Location="https://placeholder.invalid/sso"/>
  </IDPSSODescriptor>
</EntityDescriptor>
"#
    );

    EntityDescriptor::from_str(&descriptor_xml).map_err(|e| format!("parse IdP descriptor: {e}"))
}

async fn upsert_user(
    state: &AppState,
    email: &str,
    external_idp_id: &str,
    display_name: Option<&str>,
) -> AppResult<Uuid> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO users (email, external_idp_id, display_name) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (email) DO UPDATE SET \
           external_idp_id = EXCLUDED.external_idp_id, \
           display_name = COALESCE(EXCLUDED.display_name, users.display_name) \
         RETURNING id",
    )
    .bind(email)
    .bind(external_idp_id)
    .bind(display_name)
    .fetch_one(state.db())
    .await?;
    Ok(row.0)
}

async fn ensure_membership(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    default_role: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO tenant_users (tenant_id, user_id, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, user_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(default_role)
    .execute(state.db())
    .await?;
    Ok(())
}

async fn mint_session(state: &AppState, tenant_id: Uuid, user_id: Uuid) -> AppResult<String> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = format!("sps_{}", hex::encode(bytes));
    let hashed = hash_token(&raw);
    let expires_at = Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO user_sessions (tenant_id, user_id, hashed_token, expires_at) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(&hashed)
    .bind(expires_at)
    .execute(state.db())
    .await?;
    Ok(raw)
}
