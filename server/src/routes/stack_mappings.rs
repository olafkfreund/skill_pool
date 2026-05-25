//! Tenant stack mappings (Phase 3 finish-up).
//!
//! Admins curate `stack_tag → skill_slug` recommendations that drive
//! `skill-pool bootstrap`. The CLI surface (`skill-pool-server admin
//! stack-map-{set,list,remove}`) has existed since Phase 3; this layer
//! lets curators do the same in the portal.
//!
//! - `GET    /v1/tenant/stack-mappings`        — list (admin)
//! - `POST   /v1/tenant/stack-mappings`        — upsert (admin)
//! - `DELETE /v1/tenant/stack-mappings`        — remove (admin)
//!
//! No row-id is exposed because the natural key (`stack`, `skill`)
//! already uniquely identifies a row (composite PK). Forward refs are
//! allowed: a mapping can name a skill that doesn't exist yet.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct StackMapping {
    pub stack: String,
    pub skill: String,
}

pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<Vec<StackMapping>>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let rows = sqlx::query!(
        "SELECT stack_tag AS stack, skill_slug AS skill \
         FROM tenant_stack_mappings \
         WHERE tenant_id = $1 \
         ORDER BY stack_tag ASC, skill_slug ASC",
        caller.tenant.tenant_id,
    )
    .fetch_all(state.db())
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| StackMapping {
                stack: r.stack,
                skill: r.skill,
            })
            .collect::<Vec<_>>(),
    ))
}

#[derive(Deserialize)]
pub struct MutateBody {
    pub stack: String,
    pub skill: String,
}

fn validate_pair(body: &MutateBody) -> AppResult<(&str, &str)> {
    let stack = body.stack.trim();
    let skill = body.skill.trim();
    if stack.is_empty() {
        return Err(AppError::BadRequest("stack is required".into()));
    }
    if skill.is_empty() {
        return Err(AppError::BadRequest("skill is required".into()));
    }
    Ok((stack, skill))
}

pub async fn upsert(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<MutateBody>,
) -> AppResult<(StatusCode, Json<StackMapping>)> {
    require_scope(&caller.scope, "tenant:admin")?;
    let (stack, skill) = validate_pair(&body)?;

    // Idempotent insert: re-adding the same (stack, skill) is a no-op
    // thanks to the composite PK + ON CONFLICT.
    let r = sqlx::query!(
        "INSERT INTO tenant_stack_mappings (tenant_id, stack_tag, skill_slug) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, stack_tag, skill_slug) DO UPDATE SET stack_tag = EXCLUDED.stack_tag \
         RETURNING stack_tag AS stack, skill_slug AS skill",
        caller.tenant.tenant_id,
        stack,
        skill,
    )
    .fetch_one(state.db())
    .await?;
    let row = StackMapping {
        stack: r.stack,
        skill: r.skill,
    };

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "stack_mapping.set",
            target_kind: "stack_mapping",
            target_id: Some(stack),
            metadata: serde_json::json!({ "stack": stack, "skill": skill }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok((StatusCode::OK, Json(row)))
}

pub async fn remove(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<MutateBody>,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "tenant:admin")?;
    let (stack, skill) = validate_pair(&body)?;

    let result = sqlx::query!(
        "DELETE FROM tenant_stack_mappings \
         WHERE tenant_id = $1 AND stack_tag = $2 AND skill_slug = $3",
        caller.tenant.tenant_id,
        stack,
        skill,
    )
    .execute(state.db())
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "stack_mapping.remove",
            target_kind: "stack_mapping",
            target_id: Some(stack),
            metadata: serde_json::json!({ "stack": stack, "skill": skill }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
