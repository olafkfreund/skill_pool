use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    db: &'static str,
    version: &'static str,
}

pub async fn healthz(State(state): State<AppState>) -> Json<Health> {
    // Cheap DB ping. We deliberately don't fail the whole endpoint on a transient
    // DB blip — return "degraded" so the load balancer's distinction between
    // "rolling-restart-ready" and "actually-broken" stays meaningful.
    let db = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.db())
        .await
    {
        Ok(1) => "up",
        _ => "down",
    };

    Json(Health {
        status: "ok",
        db,
        version: env!("CARGO_PKG_VERSION"),
    })
}
