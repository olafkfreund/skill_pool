//! Tenant member management.
//!
//! Distinct from SCIM (which is the IdP-facing provisioning shape). This
//! endpoint serves the admin portal — human-readable rows with role + joined
//! date, plus mutations to change role and remove members. Refuses to leave a
//! tenant with zero admins.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize, sqlx::FromRow)]
pub struct Member {
    /// `tenant_users.id` — primary key for member-scoped mutations.
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
    pub active: bool,
}

// JUSTIFIED runtime-checked: `SELECT_MEMBER_COLS` is a `&str` const that is
// used with `format!()` to build WHERE-clause variants at runtime. The `query!`
// macro requires a single string literal; const-fragment concatenation is not
// supported. All queries that use this const include `tu.tenant_id = $1`.
const SELECT_MEMBER_COLS: &str = "
    SELECT tu.id          AS id,
           u.email        AS email,
           u.display_name AS display_name,
           tu.role        AS role,
           tu.created_at  AS joined_at,
           u.active       AS active
    FROM tenant_users tu
    JOIN users u ON u.id = tu.user_id
";

pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<Vec<Member>>> {
    let sql = format!("{SELECT_MEMBER_COLS} WHERE tu.tenant_id = $1 ORDER BY tu.created_at");
    let rows: Vec<Member> = sqlx::query_as(&sql)
        .bind(caller.tenant.tenant_id)
        .fetch_all(state.db())
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct PatchBody {
    pub role: String,
}

pub async fn patch_role(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(member_id): Path<Uuid>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<Member>> {
    require_admin(&caller)?;
    validate_role(&body.role)?;

    let mut tx = state.db().begin().await?;

    let target = sqlx::query!(
        "SELECT user_id, role FROM tenant_users \
         WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
        caller.tenant.tenant_id,
        member_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let t = target.ok_or(AppError::NotFound)?;
    let user_id = t.user_id;
    let current_role = t.role;

    if current_role == "admin" && body.role != "admin" {
        let other_admins = count_other_admins(&mut tx, caller.tenant.tenant_id, member_id).await?;
        if other_admins == 0 {
            return Err(AppError::BadRequest(
                "refusing to demote the last admin of this tenant".into(),
            ));
        }
    }

    sqlx::query!(
        "UPDATE tenant_users SET role = $1 \
         WHERE tenant_id = $2 AND id = $3",
        body.role,
        caller.tenant.tenant_id,
        member_id,
    )
    .execute(&mut *tx)
    .await?;

    audit::record(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "member.role_changed",
            target_kind: "member",
            target_id: Some(&member_id.to_string()),
            metadata: serde_json::json!({
                "user_id": user_id,
                "old_role": current_role,
                "new_role": body.role,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await
    .ok();

    tx.commit().await?;

    let sql = format!("{SELECT_MEMBER_COLS} WHERE tu.tenant_id = $1 AND tu.id = $2 LIMIT 1");
    let row: Option<Member> = sqlx::query_as(&sql)
        .bind(caller.tenant.tenant_id)
        .bind(member_id)
        .fetch_optional(state.db())
        .await?;
    row.map(Json).ok_or(AppError::NotFound)
}

pub async fn remove(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(member_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    require_admin(&caller)?;

    let mut tx = state.db().begin().await?;
    let target = sqlx::query!(
        "SELECT user_id, role FROM tenant_users \
         WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
        caller.tenant.tenant_id,
        member_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let t = target.ok_or(AppError::NotFound)?;
    let user_id = t.user_id;
    let role = t.role;

    if role == "admin" {
        let other_admins = count_other_admins(&mut tx, caller.tenant.tenant_id, member_id).await?;
        if other_admins == 0 {
            return Err(AppError::BadRequest(
                "refusing to remove the last admin of this tenant".into(),
            ));
        }
    }

    sqlx::query!(
        "DELETE FROM tenant_users WHERE tenant_id = $1 AND id = $2",
        caller.tenant.tenant_id,
        member_id,
    )
    .execute(&mut *tx)
    .await?;

    // Revoke any active sessions this user holds against this tenant.
    sqlx::query!(
        "UPDATE user_sessions SET revoked_at = now() \
         WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
        caller.tenant.tenant_id,
        user_id,
    )
    .execute(&mut *tx)
    .await?;

    audit::record(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "member.removed",
            target_kind: "member",
            target_id: Some(&member_id.to_string()),
            metadata: serde_json::json!({ "user_id": user_id, "former_role": role }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await
    .ok();

    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- helpers --------------------------------------------------------------

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
    if matches!(role, "viewer" | "publisher" | "curator" | "admin") {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "invalid role `{role}`; expected viewer/publisher/curator/admin"
        )))
    }
}

async fn count_other_admins(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    except_member_id: Uuid,
) -> AppResult<i64> {
    let n = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM tenant_users \
         WHERE tenant_id = $1 AND role = 'admin' AND id <> $2",
        tenant_id,
        except_member_id,
    )
    .fetch_one(&mut **tx)
    .await?
    // COUNT(*) is always non-null; unwrap is safe.
    .unwrap_or(0);
    Ok(n)
}
