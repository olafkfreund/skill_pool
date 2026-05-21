use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // scaffold; variants land as endpoints fill in (#3)
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("tenant resolution failed: {0}")]
    TenantResolution(String),

    #[error("not implemented")]
    NotImplemented,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    /// 422 with a structured field-error map. Serialised as
    /// `{"error":"unprocessable_entity","fields":<value>}` so callers can
    /// render per-field validation feedback. Used by the plugins publish
    /// handler; reusable for any future endpoint with field-level rejection.
    #[error("unprocessable entity")]
    Unprocessable(serde_json::Value),

    /// 422 with a plugin-resolution cycle path. Serialised as
    /// `{"error":"plugin_cycle","cycle":[...],"message":"..."}`.
    /// Emitted by `bootstrap` (and any other endpoint that walks
    /// `plugin_contents` transitively) when plugin A → … → A is
    /// detected. The cycle vector is normalised so its first slug is
    /// the lexicographically-smallest in the loop (matches the CLI
    /// resolver's diagnostic — see `cmd::ensure::PluginCycle`).
    #[error("plugin dependency cycle: {0:?}")]
    PluginCycle(Vec<String>),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::TenantResolution(_) => (StatusCode::BAD_REQUEST, "tenant_resolution_failed"),
            Self::NotImplemented => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            Self::Unprocessable(fields) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "error": "unprocessable_entity",
                        "fields": fields,
                    })),
                )
                    .into_response();
            }
            Self::PluginCycle(cycle) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "error": "plugin_cycle",
                        "message": "plugin dependency cycle detected",
                        "cycle": cycle,
                    })),
                )
                    .into_response();
            }
            Self::Sqlx(e) => {
                tracing::error!(error = ?e, "sqlx error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
            Self::Anyhow(e) => {
                tracing::error!(error = ?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (
            status,
            Json(json!({ "error": code, "message": self.to_string() })),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
