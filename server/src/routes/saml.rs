//! SAML 2.0 service-provider endpoints.
//!
//! Phase 2 ships:
//!   - `GET /v1/auth/saml/discover`        — `{enabled: bool}` for the web UI
//!   - `GET /v1/auth/saml/{tenant}/metadata` — SP metadata XML for IdP imports
//!   - `POST /v1/auth/saml/{tenant}/acs`     — assertion consumer (stubbed, 501)
//!
//! IdP-initiated SAML flow is the v1 target (matches how Okta/Azure/ADFS
//! typically integrate). SP-initiated (AuthnRequest generation) and the
//! actual `SAMLResponse` signature validator land in a follow-up — both
//! need a real XML signature implementation (xmlsec1 / samael).

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::env;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

const ACS_PATH: &str = "/v1/auth/saml";

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

    let origin = env::var("SKILL_POOL_PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://skill-pool.example.com".to_string())
        .trim_end_matches('/')
        .to_string();
    let acs_url = format!("{origin}{ACS_PATH}/{}/acs", tenant.tenant_slug);

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

fn render_sp_metadata(sp_entity_id: &str, acs_url: &str) -> String {
    // Minimal SP metadata. Valid SAML 2.0 EntityDescriptor per OASIS spec.
    // Single ACS at index 0 using HTTP-POST binding. Wants signed responses;
    // does not sign AuthnRequests (we don't generate them — IdP-initiated).
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

// --- ACS (stub) -----------------------------------------------------------

pub async fn acs(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(_slug): Path<String>,
) -> AppResult<Response> {
    // Confirm the tenant has SAML configured so error messages stay informative.
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT tenant_id FROM tenant_saml WHERE tenant_id = $1")
            .bind(tenant.tenant_id)
            .fetch_optional(state.db())
            .await?;
    if row.is_none() {
        return Err(AppError::BadRequest(
            "SAML not configured for this tenant".into(),
        ));
    }

    // Stubbed. Production handler must: parse the SAMLResponse form field,
    // validate XML signature against tenant_saml.idp_x509_cert, check
    // Conditions / NotOnOrAfter, extract NameID + email + attributes, then
    // upsert_user + ensure_membership + mint_session (same helpers as OIDC).
    let mut resp = (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "not_implemented",
            "message": "SAML assertion validation lands in the next iteration of #8. \
                        Today this endpoint accepts the SP-metadata import flow only; \
                        the IdP-side configuration round-trip is testable end-to-end.",
        })),
    )
        .into_response();
    resp.headers_mut().insert(
        "x-skill-pool-saml-status",
        HeaderValue::from_static("acs-stubbed"),
    );
    Ok(resp)
}

// --- Helper (re-used by start link in the web; here only for tests) ------

#[allow(dead_code)]
pub fn idp_sso_url_for(_state: &AppState) -> String {
    // Reserved — when SP-initiated flow lands, the web will build a redirect
    // through this helper rather than the raw idp_sso_url.
    String::new()
}
