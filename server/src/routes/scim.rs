//! SCIM 2.0 (RFC 7643/7644) — provisioning endpoints for IdPs.
//!
//! Scope: the bare minimum that Okta and Azure AD actually exercise:
//!   - ServiceProviderConfig + Schemas + ResourceTypes
//!   - Users list with `?filter=userName eq "..."` (the provisioning lookup
//!     IdPs perform before deciding whether to POST or PATCH)
//!   - Users POST (create)
//!   - Users GET by id
//!   - Users PATCH — only the `replace active` operation (deprovisioning)
//!   - Users DELETE — same effect as PATCH active=false
//!
//! Auth: bearer token with the `scim:provision` scope. Mint via:
//!   skill-pool-server admin token-create --tenant acme --name okta \
//!     --scope 'scim:provision'

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const SCHEMA_USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const SCHEMA_LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const SCHEMA_PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const SCHEMA_ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const SCHEMA_SPC: &str = "urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig";

// --- SCIM resource shapes ------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScimUser {
    pub schemas: Vec<String>,
    pub id: String,
    #[serde(rename = "userName")]
    pub user_name: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ScimName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emails: Vec<ScimEmail>,
    pub meta: ScimMeta,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ScimName {
    #[serde(rename = "formatted", default, skip_serializing_if = "Option::is_none")]
    pub formatted: Option<String>,
    #[serde(rename = "givenName", default, skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,
    #[serde(
        rename = "familyName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub family_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScimEmail {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScimMeta {
    #[serde(rename = "resourceType")]
    pub resource_type: String,
    pub created: DateTime<Utc>,
    #[serde(rename = "lastModified")]
    pub last_modified: DateTime<Utc>,
    pub location: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // `emails` accepted from clients but we canonicalise on userName
pub struct CreateUserRequest {
    #[serde(rename = "userName")]
    pub user_name: String,
    #[serde(default = "default_active")]
    pub active: bool,
    #[serde(default)]
    pub name: Option<ScimName>,
    #[serde(default)]
    pub emails: Vec<ScimEmail>,
}

fn default_active() -> bool {
    true
}

#[derive(Deserialize)]
pub struct PatchRequest {
    #[serde(default)]
    pub schemas: Vec<String>,
    #[serde(rename = "Operations", default)]
    pub operations: Vec<PatchOp>,
}

#[derive(Deserialize)]
pub struct PatchOp {
    pub op: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

// --- Auth gate ------------------------------------------------------------

fn require_scim_scope(caller: &AuthedCaller) -> AppResult<()> {
    if caller
        .scope
        .split_whitespace()
        .any(|s| s == "scim:provision" || s == "tenant:admin" || s == "*")
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// --- Discovery: ServiceProviderConfig + Schemas + ResourceTypes ----------

pub async fn service_provider_config() -> Json<Value> {
    Json(json!({
        "schemas": [SCHEMA_SPC],
        "documentationUri": "https://github.com/olafkfreund/skill_pool/blob/main/docs/scim.md",
        "patch": { "supported": true },
        "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
        "filter": { "supported": true, "maxResults": 200 },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "OAuth Bearer Token",
            "description": "Authentication via an API token with `scim:provision` scope.",
            "primary": true
        }]
    }))
}

pub async fn resource_types() -> Json<Value> {
    Json(json!({
        "schemas": [SCHEMA_LIST],
        "totalResults": 1,
        "Resources": [{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
            "id": "User",
            "name": "User",
            "endpoint": "/scim/v2/Users",
            "schema": SCHEMA_USER
        }]
    }))
}

pub async fn schemas() -> Json<Value> {
    Json(json!({
        "schemas": [SCHEMA_LIST],
        "totalResults": 1,
        "Resources": [{
            "id": SCHEMA_USER,
            "name": "User",
            "description": "skill-pool tenant membership exposed as a SCIM User.",
            "attributes": [
                { "name": "userName", "type": "string", "required": true, "mutability": "readWrite" },
                { "name": "active", "type": "boolean", "required": false, "mutability": "readWrite" },
                { "name": "emails", "type": "complex", "multiValued": true, "required": false }
            ]
        }]
    }))
}

// --- Users -----------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default)]
    pub count: Option<i64>,
}

pub async fn list_users(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_scim_scope(&caller)?;

    let mut rows = if let Some(filter) = &q.filter {
        // Only support `userName eq "..."` — the universal provisioning-lookup
        // shape. Anything else returns 400 with a clear error message.
        let email = parse_username_eq(filter)
            .ok_or_else(|| AppError::BadRequest(format!("unsupported filter: {filter}")))?;
        fetch_membership_by_email(&state, caller.tenant.tenant_id, &email).await?
    } else {
        fetch_all_memberships(&state, caller.tenant.tenant_id, q.count.unwrap_or(50)).await?
    };

    let resources: Vec<ScimUser> = rows
        .drain(..)
        .map(|r| row_to_scim_user(&r, &caller.tenant.tenant_slug))
        .collect();

    Ok(Json(json!({
        "schemas": [SCHEMA_LIST],
        "totalResults": resources.len(),
        "startIndex": q.start_index.unwrap_or(1),
        "itemsPerPage": resources.len(),
        "Resources": resources,
    })))
}

pub async fn create_user(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(req): Json<CreateUserRequest>,
) -> AppResult<(StatusCode, Json<ScimUser>)> {
    require_scim_scope(&caller)?;

    if req.user_name.trim().is_empty() {
        return Err(AppError::BadRequest("userName is required".into()));
    }
    let email = req.user_name.trim().to_lowercase();

    let display_name = req
        .name
        .as_ref()
        .and_then(|n| n.formatted.clone())
        .or_else(|| {
            req.name.as_ref().map(|n| {
                format!(
                    "{} {}",
                    n.given_name.as_deref().unwrap_or(""),
                    n.family_name.as_deref().unwrap_or("")
                )
                .trim()
                .to_string()
            })
        })
        .filter(|s| !s.is_empty());

    // Upsert user by email.
    let row = sqlx::query!(
        "INSERT INTO users (email, display_name, active) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (email) DO UPDATE SET \
           display_name = COALESCE(EXCLUDED.display_name, users.display_name), \
           active = EXCLUDED.active \
         RETURNING id",
        &email,
        display_name.as_deref(),
        req.active,
    )
    .fetch_one(state.db())
    .await?;
    let user_id = row.id;

    // Add membership at the default 'viewer' role; admins promote later.
    sqlx::query!(
        "INSERT INTO tenant_users (tenant_id, user_id, role) \
         VALUES ($1, $2, 'viewer') \
         ON CONFLICT (tenant_id, user_id) DO NOTHING",
        caller.tenant.tenant_id,
        user_id,
    )
    .execute(state.db())
    .await?;

    let row = fetch_membership_by_user(&state, caller.tenant.tenant_id, user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(row_to_scim_user(&row, &caller.tenant.tenant_slug)),
    ))
}

pub async fn get_user(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ScimUser>> {
    require_scim_scope(&caller)?;
    let row = fetch_membership_by_id(&state, caller.tenant.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(row_to_scim_user(&row, &caller.tenant.tenant_slug)))
}

pub async fn patch_user(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchRequest>,
) -> AppResult<Json<ScimUser>> {
    require_scim_scope(&caller)?;

    if !req.schemas.iter().any(|s| s == SCHEMA_PATCH) {
        return Err(AppError::BadRequest(format!(
            "expected schemas to include {SCHEMA_PATCH}"
        )));
    }

    let row = fetch_membership_by_id(&state, caller.tenant.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // We only support `replace active`. Everything else: 400.
    for op in &req.operations {
        let op_kind = op.op.to_ascii_lowercase();
        if op_kind != "replace" {
            return Err(AppError::BadRequest(format!(
                "only `replace` op supported; got `{}`",
                op.op
            )));
        }
        let path = op.path.as_deref().unwrap_or("");
        if path != "active" {
            return Err(AppError::BadRequest(format!(
                "only `replace active` is supported; got path `{path}`"
            )));
        }
        let active = op
            .value
            .as_ref()
            .and_then(|v| v.as_bool())
            .ok_or_else(|| AppError::BadRequest("replace active value must be bool".into()))?;

        if !active {
            // Deprovisioning: revoke membership + active flag. Keep the user
            // row for audit. Future logins will need re-provision.
            sqlx::query!("UPDATE users SET active = false WHERE id = $1", row.user_id)
                .execute(state.db())
                .await?;
            sqlx::query!("DELETE FROM tenant_users WHERE id = $1", row.tenant_user_id)
                .execute(state.db())
                .await?;
            // Best-effort: revoke active sessions.
            let _ = sqlx::query!(
                "UPDATE user_sessions SET revoked_at = now() \
                 WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
                caller.tenant.tenant_id,
                row.user_id,
            )
            .execute(state.db())
            .await;
            // Return a synthetic representation reflecting the post-deprovision state.
            return Ok(Json(ScimUser {
                schemas: vec![SCHEMA_USER.to_string()],
                id: id.to_string(),
                user_name: row.email,
                active: false,
                name: row.display_name.map(|d| ScimName {
                    formatted: Some(d),
                    ..Default::default()
                }),
                emails: vec![],
                meta: ScimMeta {
                    resource_type: "User".into(),
                    created: row.created_at,
                    last_modified: Utc::now(),
                    location: location_for(&caller.tenant.tenant_slug, id),
                },
            }));
        } else {
            // Re-activation toggle on a still-existing membership row.
            sqlx::query!("UPDATE users SET active = true WHERE id = $1", row.user_id)
                .execute(state.db())
                .await?;
        }
    }

    let refreshed = fetch_membership_by_id(&state, caller.tenant.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(row_to_scim_user(
        &refreshed,
        &caller.tenant.tenant_slug,
    )))
}

pub async fn delete_user(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    require_scim_scope(&caller)?;
    let row = fetch_membership_by_id(&state, caller.tenant.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;

    sqlx::query!("DELETE FROM tenant_users WHERE id = $1", row.tenant_user_id)
        .execute(state.db())
        .await?;
    sqlx::query!("UPDATE users SET active = false WHERE id = $1", row.user_id)
        .execute(state.db())
        .await?;
    let _ = sqlx::query!(
        "UPDATE user_sessions SET revoked_at = now() \
         WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
        caller.tenant.tenant_id,
        row.user_id,
    )
    .execute(state.db())
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- Filter parsing -------------------------------------------------------

/// Parse `userName eq "..."`. Tolerates surrounding whitespace and either
/// single or double quotes. Returns the value lowered for email comparison.
pub(crate) fn parse_username_eq(filter: &str) -> Option<String> {
    let s = filter.trim();
    let mut rest = s.strip_prefix("userName")?.trim_start();
    rest = rest.strip_prefix("eq")?.trim_start();
    if let Some(after_quote) = rest.strip_prefix('"') {
        let end = after_quote.find('"')?;
        return Some(after_quote[..end].to_lowercase());
    }
    if let Some(after_quote) = rest.strip_prefix('\'') {
        let end = after_quote.find('\'')?;
        return Some(after_quote[..end].to_lowercase());
    }
    None
}

// --- Membership row mapper ------------------------------------------------

#[derive(sqlx::FromRow)]
struct MembershipRow {
    tenant_user_id: Uuid,
    user_id: Uuid,
    email: String,
    display_name: Option<String>,
    active: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const SELECT_MEMBERSHIP: &str = "
    SELECT tu.id          AS tenant_user_id,
           u.id           AS user_id,
           u.email        AS email,
           u.display_name AS display_name,
           u.active       AS active,
           tu.created_at  AS created_at,
           u.updated_at   AS updated_at
    FROM tenant_users tu
    JOIN users u ON u.id = tu.user_id
    WHERE tu.tenant_id = $1
";

// JUSTIFIED: These four helpers share SELECT_MEMBERSHIP base SQL via format!() to
// avoid duplicating a 7-column JOIN. Each appends a different WHERE/LIMIT clause
// selected at call-site — query! cannot compose SQL from a runtime `const` string.
async fn fetch_membership_by_id(
    state: &AppState,
    tenant_id: Uuid,
    tu_id: Uuid,
) -> AppResult<Option<MembershipRow>> {
    let sql = format!("{SELECT_MEMBERSHIP} AND tu.id = $2");
    Ok(sqlx::query_as(&sql)
        .bind(tenant_id)
        .bind(tu_id)
        .fetch_optional(state.db())
        .await?)
}

async fn fetch_membership_by_email(
    state: &AppState,
    tenant_id: Uuid,
    email: &str,
) -> AppResult<Vec<MembershipRow>> {
    let sql = format!("{SELECT_MEMBERSHIP} AND lower(u.email) = $2 LIMIT 1");
    Ok(sqlx::query_as(&sql)
        .bind(tenant_id)
        .bind(email)
        .fetch_all(state.db())
        .await?)
}

async fn fetch_membership_by_user(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
) -> AppResult<Option<MembershipRow>> {
    let sql = format!("{SELECT_MEMBERSHIP} AND u.id = $2");
    Ok(sqlx::query_as(&sql)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(state.db())
        .await?)
}

async fn fetch_all_memberships(
    state: &AppState,
    tenant_id: Uuid,
    limit: i64,
) -> AppResult<Vec<MembershipRow>> {
    let sql = format!("{SELECT_MEMBERSHIP} ORDER BY tu.created_at LIMIT $2");
    Ok(sqlx::query_as(&sql)
        .bind(tenant_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(state.db())
        .await?)
}

fn row_to_scim_user(r: &MembershipRow, tenant_slug: &str) -> ScimUser {
    ScimUser {
        schemas: vec![SCHEMA_USER.to_string()],
        id: r.tenant_user_id.to_string(),
        user_name: r.email.clone(),
        active: r.active,
        name: r.display_name.as_ref().map(|d| ScimName {
            formatted: Some(d.clone()),
            ..Default::default()
        }),
        emails: vec![ScimEmail {
            value: r.email.clone(),
            primary: Some(true),
            kind: Some("work".into()),
        }],
        meta: ScimMeta {
            resource_type: "User".into(),
            created: r.created_at,
            last_modified: r.updated_at,
            location: location_for(tenant_slug, r.tenant_user_id),
        },
    }
}

fn location_for(_tenant_slug: &str, id: Uuid) -> String {
    // SCIM clients use this for resource navigation; it's a relative path.
    format!("/scim/v2/Users/{id}")
}

// --- Error response helper for SCIM-style errors (not currently wired) --

#[allow(dead_code)]
pub fn scim_error(status: StatusCode, detail: &str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/scim+json"),
    );
    (
        status,
        headers,
        Json(json!({
            "schemas": [SCHEMA_ERROR],
            "status": status.as_u16().to_string(),
            "detail": detail
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_username_eq() {
        assert_eq!(
            parse_username_eq(r#"userName eq "x@y.test""#).as_deref(),
            Some("x@y.test")
        );
        assert_eq!(
            parse_username_eq(r#"userName eq 'x@y.test'"#).as_deref(),
            Some("x@y.test")
        );
        assert_eq!(
            parse_username_eq(r#"  userName  eq  "X@Y" "#).as_deref(),
            Some("x@y")
        );
    }

    #[test]
    fn rejects_other_filters() {
        assert!(parse_username_eq(r#"displayName eq "x""#).is_none());
        assert!(parse_username_eq(r#"userName co "x""#).is_none());
        assert!(parse_username_eq(r#"userName eq x"#).is_none());
    }
}
