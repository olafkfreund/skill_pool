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
