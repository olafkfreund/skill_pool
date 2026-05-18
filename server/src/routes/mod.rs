use axum::routing::{get, post};
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod bootstrap;
mod drafts;
mod enterprise;
mod health;
mod members;
mod oidc;
mod saml;
mod scim;
mod skills;
mod theme;

const MAX_BUNDLE_BYTES: usize = 5 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/healthz", get(health::healthz))
        .route("/v1/skills", get(skills::list).post(skills::publish))
        .route("/v1/skills/validate", post(skills::validate))
        .route("/v1/skills/{slug}", get(skills::get_one))
        .route("/v1/skills/{slug}/bundle.tar.gz", get(skills::get_bundle))
        .route("/v1/skills/{slug}/skill-md", get(skills::get_skill_md))
        .route("/v1/theme", get(theme::get_theme).put(theme::put_theme))
        // Bootstrap (Phase 3)
        .route("/v1/bootstrap", get(bootstrap::bootstrap))
        // Drafts (Phase 4 — retrospective capture)
        .route("/v1/drafts", get(drafts::list).post(drafts::create))
        .route("/v1/drafts/{id}", get(drafts::get_one))
        .route("/v1/drafts/{id}/skill-md", get(drafts::get_skill_md))
        .route("/v1/drafts/{id}/publish", post(drafts::publish))
        .route("/v1/drafts/{id}/discard", post(drafts::discard))
        // Enterprise
        .route(
            "/v1/enterprise/managed-settings",
            get(enterprise::managed_settings),
        )
        // Members admin
        .route("/v1/tenant/members", get(members::list))
        .route(
            "/v1/tenant/members/{id}",
            axum::routing::patch(members::patch_role).delete(members::remove),
        )
        // OIDC
        .route("/v1/auth/oidc/discover", get(oidc::discover))
        .route("/v1/auth/oidc/{slug}/start", get(oidc::start))
        .route("/v1/auth/oidc/{slug}/callback", get(oidc::callback))
        .route("/v1/auth/whoami", get(oidc::whoami))
        .route("/v1/auth/logout", post(oidc::logout))
        // SAML
        .route("/v1/auth/saml/discover", get(saml::discover))
        .route("/v1/auth/saml/{slug}/metadata", get(saml::metadata))
        .route("/v1/auth/saml/{slug}/acs", post(saml::acs))
        // SCIM 2.0
        .route(
            "/scim/v2/ServiceProviderConfig",
            get(scim::service_provider_config),
        )
        .route("/scim/v2/ResourceTypes", get(scim::resource_types))
        .route("/scim/v2/Schemas", get(scim::schemas))
        .route(
            "/scim/v2/Users",
            get(scim::list_users).post(scim::create_user),
        )
        .route(
            "/scim/v2/Users/{id}",
            get(scim::get_user)
                .patch(scim::patch_user)
                .delete(scim::delete_user),
        )
        .layer(RequestBodyLimitLayer::new(MAX_BUNDLE_BYTES + 64 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
