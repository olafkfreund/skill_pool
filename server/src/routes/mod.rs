use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod health;
mod skills;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/healthz", get(health::healthz))
        .route("/v1/skills", get(skills::list).post(skills::publish))
        .route("/v1/skills/{slug}", get(skills::get_one))
        .route("/v1/skills/{slug}/bundle.tar.gz", get(skills::get_bundle))
        .route("/v1/skills/validate", post(skills::validate))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
