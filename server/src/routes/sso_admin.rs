//! Admin CRUD for per-tenant SSO config (#4).
//!
//! Sits at `/v1/tenant/sso`. All endpoints require the `tenant:admin`
//! scope. Lets the admin portal read / write / clear the OIDC and SAML
//! rows that the runtime `oidc::*` and `saml::*` handlers consume.
//!
//! Why a separate surface from the runtime routes:
//!   - `/v1/auth/oidc/{slug}/start` and friends are tenant-scoped via
//!     subdomain and need to be unauthenticated (browser redirect flow).
//!   - This surface is admin-authenticated, JSON-bodied, and idempotent —
//!     the shape the SvelteKit admin page wants to talk to.
//!
//! Sensitive fields (OIDC `client_secret`) are masked on GET. Plaintext
//! is never echoed back to the caller — the row still stores it
//! verbatim (the runtime OIDC handler needs the raw secret to exchange
//! codes), but the admin GET only exposes a `client_secret_hint` like
//! `"••••f3a2"` so the UI can confirm a value is configured.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use samael::metadata::EntityDescriptor;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const VALID_ROLES: &[&str] = &["viewer", "publisher", "curator", "admin"];

// ---------------------------------------------------------------------------
// GET — full SSO config view
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SsoConfigView {
    /// `oidc`, `saml`, or null when neither is configured. If both rows
    /// exist (rare — operator pre-populated via CLI) we surface `oidc`
    /// here because that's the path the runtime tries first.
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oidc: Option<OidcView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saml: Option<SamlView>,
    /// SCIM endpoint for the IdP to push provisioning into. Independent
    /// of OIDC/SAML — a tenant can SCIM-provision without either.
    pub scim_endpoint: String,
}

#[derive(Serialize)]
pub struct OidcView {
    pub issuer_url: String,
    pub client_id: String,
    /// Last 4 chars of the stored secret, prefixed with `••••`. Never
    /// the full secret — see module docs.
    pub client_secret_hint: String,
    pub default_role: String,
}

#[derive(Serialize)]
pub struct SamlView {
    pub idp_entity_id: String,
    pub idp_sso_url: String,
    /// Length of the IdP cert in bytes — useful for the UI to render
    /// "X bytes of PEM configured" without echoing the cert back.
    pub idp_x509_cert_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sp_entity_id: Option<String>,
    pub default_role: String,
}

pub async fn get_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<SsoConfigView>> {
    require_admin(&caller)?;

    let oidc = load_oidc_row(state.db(), caller.tenant.tenant_id).await?;
    let saml = load_saml_row(state.db(), caller.tenant.tenant_id).await?;

    let kind = match (oidc.is_some(), saml.is_some()) {
        (true, _) => Some("oidc"),
        (false, true) => Some("saml"),
        _ => None,
    };

    Ok(Json(SsoConfigView {
        kind,
        oidc,
        saml,
        scim_endpoint: "/scim/v2/Users".to_string(),
    }))
}

async fn load_oidc_row(db: &sqlx::PgPool, tenant_id: Uuid) -> AppResult<Option<OidcView>> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT issuer_url, client_id, client_secret, default_role \
         FROM tenant_sso WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(issuer_url, client_id, client_secret, default_role)| OidcView {
        issuer_url,
        client_id,
        client_secret_hint: mask_secret(&client_secret),
        default_role,
    }))
}

async fn load_saml_row(db: &sqlx::PgPool, tenant_id: Uuid) -> AppResult<Option<SamlView>> {
    let row: Option<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT idp_entity_id, idp_sso_url, idp_x509_cert, sp_entity_id, default_role \
         FROM tenant_saml WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(
        |(idp_entity_id, idp_sso_url, idp_x509_cert, sp_entity_id, default_role)| SamlView {
            idp_entity_id,
            idp_sso_url,
            idp_x509_cert_bytes: idp_x509_cert.len(),
            sp_entity_id,
            default_role,
        },
    ))
}

/// Show only the trailing 4 chars so the admin can sanity-check what's
/// configured without leaking the secret. Short secrets (≤ 4 chars) are
/// fully masked.
pub fn mask_secret(s: &str) -> String {
    if s.len() <= 4 {
        return "••••".to_string();
    }
    let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

// ---------------------------------------------------------------------------
// PUT /v1/tenant/sso/oidc
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PutOidcBody {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub default_role: String,
}

pub async fn put_oidc(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutOidcBody>,
) -> AppResult<Json<SsoConfigView>> {
    require_admin(&caller)?;
    validate_oidc(&body)?;

    sqlx::query(
        "INSERT INTO tenant_sso (tenant_id, issuer_url, client_id, client_secret, default_role) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
           issuer_url = EXCLUDED.issuer_url, \
           client_id = EXCLUDED.client_id, \
           client_secret = EXCLUDED.client_secret, \
           default_role = EXCLUDED.default_role",
    )
    .bind(caller.tenant.tenant_id)
    .bind(body.issuer_url.trim())
    .bind(body.client_id.trim())
    .bind(body.client_secret.trim())
    .bind(body.default_role.trim())
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.sso.oidc.update",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "issuer_url": body.issuer_url.trim(),
                "client_id": body.client_id.trim(),
                "default_role": body.default_role.trim(),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    // Return the canonical (masked) view.
    get_config(State(state), caller).await
}

fn validate_oidc(body: &PutOidcBody) -> AppResult<()> {
    let issuer = body.issuer_url.trim();
    if issuer.is_empty() {
        return Err(AppError::BadRequest("issuer_url must not be empty".into()));
    }
    let parsed = Url::parse(issuer)
        .map_err(|e| AppError::BadRequest(format!("issuer_url must be a valid URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::BadRequest(
            "issuer_url must use http or https".into(),
        ));
    }
    if body.client_id.trim().is_empty() {
        return Err(AppError::BadRequest("client_id must not be empty".into()));
    }
    if body.client_secret.trim().is_empty() {
        return Err(AppError::BadRequest("client_secret must not be empty".into()));
    }
    validate_role(body.default_role.trim())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PUT /v1/tenant/sso/saml
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PutSamlBody {
    /// Full IdP metadata XML (the document you'd otherwise paste into
    /// an SP's "upload metadata" field).
    pub metadata_xml: String,
    pub default_role: String,
    /// Optional SP entity ID override. If omitted, defaults to
    /// `urn:skill-pool:tenant:<slug>` at the metadata endpoint.
    #[serde(default)]
    pub sp_entity_id: Option<String>,
}

pub async fn put_saml(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutSamlBody>,
) -> AppResult<Json<SsoConfigView>> {
    require_admin(&caller)?;
    let parsed = parse_saml_metadata(&body)?;

    sqlx::query(
        "INSERT INTO tenant_saml \
           (tenant_id, idp_entity_id, idp_sso_url, idp_x509_cert, sp_entity_id, default_role) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
           idp_entity_id = EXCLUDED.idp_entity_id, \
           idp_sso_url = EXCLUDED.idp_sso_url, \
           idp_x509_cert = EXCLUDED.idp_x509_cert, \
           sp_entity_id = EXCLUDED.sp_entity_id, \
           default_role = EXCLUDED.default_role",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&parsed.idp_entity_id)
    .bind(&parsed.idp_sso_url)
    .bind(&parsed.idp_x509_cert_pem)
    .bind(parsed.sp_entity_id.as_deref())
    .bind(body.default_role.trim())
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.sso.saml.update",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "idp_entity_id": parsed.idp_entity_id,
                "idp_sso_url": parsed.idp_sso_url,
                "default_role": body.default_role.trim(),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    get_config(State(state), caller).await
}

#[cfg_attr(test, derive(Debug))]
struct ParsedSamlMetadata {
    idp_entity_id: String,
    idp_sso_url: String,
    /// PEM-formatted (with BEGIN/END markers) so the runtime SAML path
    /// can build an IDP descriptor without re-wrapping.
    idp_x509_cert_pem: String,
    sp_entity_id: Option<String>,
}

fn parse_saml_metadata(body: &PutSamlBody) -> AppResult<ParsedSamlMetadata> {
    validate_role(body.default_role.trim())?;

    let xml = body.metadata_xml.trim();
    if xml.is_empty() {
        return Err(AppError::BadRequest("metadata_xml must not be empty".into()));
    }
    // samael parses the EntityDescriptor and validates the XML
    // structurally. If the IdP gave us a SP descriptor by mistake, this
    // still parses but we'll fail to find IDPSSODescriptor below.
    let descriptor: EntityDescriptor = EntityDescriptor::from_str(xml)
        .map_err(|e| AppError::BadRequest(format!("metadata_xml does not parse: {e}")))?;

    let idp_entity_id = descriptor
        .entity_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("metadata_xml has no entityID".into()))?;

    let idp_descriptors = &descriptor.idp_sso_descriptors;
    let idp = idp_descriptors
        .as_ref()
        .and_then(|v| v.first())
        .ok_or_else(|| {
            AppError::BadRequest(
                "metadata_xml has no IDPSSODescriptor — paste IdP metadata, not SP metadata"
                    .into(),
            )
        })?;

    // Pick the first SSO service. Production IdPs publish at least one;
    // we don't care about Binding here — the runtime path uses HTTP-POST
    // to ACS, the IdP picks its own AuthnRequest binding.
    let sso_url = idp
        .single_sign_on_services
        .first()
        .map(|s| s.location.clone())
        .ok_or_else(|| {
            AppError::BadRequest("metadata_xml has no SingleSignOnService entries".into())
        })?;

    // Extract the *signing* cert. Some IdPs publish two KeyDescriptors
    // (signing + encryption) — we only verify signatures, so grab the
    // first `use="signing"` or fall back to the first overall.
    let cert_b64 = idp
        .key_descriptors
        .iter()
        .find(|k| k.key_use.as_deref() == Some("signing"))
        .or_else(|| idp.key_descriptors.first())
        .and_then(|k| k.key_info.x509_data.as_ref())
        .and_then(|x| x.certificates.first())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| {
            AppError::BadRequest("metadata_xml has no X.509 certificate in any KeyDescriptor".into())
        })?;

    let pem = wrap_pem(&cert_b64);

    Ok(ParsedSamlMetadata {
        idp_entity_id,
        idp_sso_url: sso_url,
        idp_x509_cert_pem: pem,
        sp_entity_id: body
            .sp_entity_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// Wrap a base64 blob in PEM BEGIN/END markers, breaking at 64 chars
/// per line (RFC 7468). The runtime SAML path strips the markers back
/// off, but other tools (e.g. `openssl x509 -text`) expect them.
fn wrap_pem(body: &str) -> String {
    let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = String::with_capacity(cleaned.len() + 64);
    out.push_str("-----BEGIN CERTIFICATE-----\n");
    for chunk in cleaned.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

// ---------------------------------------------------------------------------
// DELETE /v1/tenant/sso
// ---------------------------------------------------------------------------

pub async fn delete_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<StatusCode> {
    require_admin(&caller)?;

    let mut tx = state.db().begin().await?;
    sqlx::query("DELETE FROM tenant_sso WHERE tenant_id = $1")
        .bind(caller.tenant.tenant_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM tenant_saml WHERE tenant_id = $1")
        .bind(caller.tenant.tenant_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.sso.delete",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({}),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_admin(caller: &AuthedCaller) -> AppResult<()> {
    if caller
        .scope
        .split_whitespace()
        .any(|s| s == "tenant:admin" || s == "*")
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn validate_role(role: &str) -> AppResult<()> {
    if VALID_ROLES.contains(&role) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "default_role must be one of {VALID_ROLES:?}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Tests — unit-level (no DB). Integration coverage lives in
// `server/tests/sso_admin.rs`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_redacts_short_values() {
        assert_eq!(mask_secret(""), "••••");
        assert_eq!(mask_secret("ab"), "••••");
        assert_eq!(mask_secret("abcd"), "••••");
    }

    #[test]
    fn mask_secret_keeps_last_four_for_long_values() {
        assert_eq!(mask_secret("super-secret-1234"), "••••1234");
        assert_eq!(mask_secret("0123456789"), "••••6789");
    }

    #[test]
    fn validate_oidc_accepts_https_issuer() {
        let body = PutOidcBody {
            issuer_url: "https://login.example.com/realms/acme".into(),
            client_id: "spk-acme".into(),
            client_secret: "shhh".into(),
            default_role: "viewer".into(),
        };
        validate_oidc(&body).unwrap();
    }

    #[test]
    fn validate_oidc_rejects_garbage_url() {
        let body = PutOidcBody {
            issuer_url: "not a url".into(),
            client_id: "spk".into(),
            client_secret: "x".into(),
            default_role: "viewer".into(),
        };
        let err = validate_oidc(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("issuer_url"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_oidc_rejects_non_http_scheme() {
        let body = PutOidcBody {
            issuer_url: "ftp://nope.example.com".into(),
            client_id: "x".into(),
            client_secret: "x".into(),
            default_role: "viewer".into(),
        };
        let err = validate_oidc(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("http"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_oidc_rejects_empty_fields() {
        let body = PutOidcBody {
            issuer_url: "https://example.com/".into(),
            client_id: "  ".into(),
            client_secret: "x".into(),
            default_role: "viewer".into(),
        };
        let err = validate_oidc(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("client_id"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_oidc_rejects_bad_role() {
        let body = PutOidcBody {
            issuer_url: "https://example.com/".into(),
            client_id: "x".into(),
            client_secret: "x".into(),
            default_role: "superuser".into(),
        };
        let err = validate_oidc(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("default_role"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn wrap_pem_breaks_at_64_chars() {
        let body = "A".repeat(130);
        let pem = wrap_pem(&body);
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(pem.ends_with("-----END CERTIFICATE-----\n"));
        let lines: Vec<&str> = pem.lines().collect();
        // header + 3 body lines (64 + 64 + 2) + footer
        assert_eq!(lines.len(), 5, "lines were: {lines:?}");
        assert_eq!(lines[1].len(), 64);
        assert_eq!(lines[2].len(), 64);
        assert_eq!(lines[3].len(), 2);
    }

    #[test]
    fn parse_saml_metadata_extracts_entity_sso_and_cert() {
        // Minimal but realistic IdP metadata document.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  xmlns:ds="http://www.w3.org/2000/09/xmldsig#"
                  entityID="https://idp.example.com/saml2/idp/metadata.php">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <ds:KeyInfo>
        <ds:X509Data>
          <ds:X509Certificate>MIIBszCCARygAwIBAgIJAKxQfake</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
                         Location="https://idp.example.com/saml2/idp/SSOService.php"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;
        let body = PutSamlBody {
            metadata_xml: xml.into(),
            default_role: "viewer".into(),
            sp_entity_id: None,
        };
        let parsed = parse_saml_metadata(&body).expect("parses");
        assert_eq!(
            parsed.idp_entity_id,
            "https://idp.example.com/saml2/idp/metadata.php"
        );
        assert_eq!(
            parsed.idp_sso_url,
            "https://idp.example.com/saml2/idp/SSOService.php"
        );
        assert!(parsed.idp_x509_cert_pem.contains("-----BEGIN CERTIFICATE-----"));
        assert!(parsed.idp_x509_cert_pem.contains("MIIBszCCARygAwIBAgIJAKxQfake"));
        assert!(parsed.idp_x509_cert_pem.contains("-----END CERTIFICATE-----"));
        assert!(parsed.sp_entity_id.is_none());
    }

    #[test]
    fn parse_saml_metadata_rejects_garbage_xml() {
        let body = PutSamlBody {
            metadata_xml: "not-xml-at-all".into(),
            default_role: "viewer".into(),
            sp_entity_id: None,
        };
        let err = parse_saml_metadata(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("metadata_xml"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_saml_metadata_rejects_sp_descriptor_only() {
        // Has SPSSODescriptor (not IDPSSODescriptor) — common mistake
        // where admins paste the wrong half of a federation file.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  entityID="https://sp.example.com/">
  <SPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <AssertionConsumerService index="0"
                              Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
                              Location="https://sp.example.com/acs"/>
  </SPSSODescriptor>
</EntityDescriptor>"#;
        let body = PutSamlBody {
            metadata_xml: xml.into(),
            default_role: "viewer".into(),
            sp_entity_id: None,
        };
        let err = parse_saml_metadata(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(
                m.contains("IDPSSODescriptor"),
                "expected helpful error, got: {m}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_saml_metadata_rejects_bad_role() {
        let body = PutSamlBody {
            metadata_xml: "<EntityDescriptor/>".into(),
            default_role: "operator".into(),
            sp_entity_id: None,
        };
        let err = parse_saml_metadata(&body).unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("default_role"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
