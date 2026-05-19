use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::metrics;
use crate::state::AppState;
use crate::tracing_setup;

mod audit_siem;
mod bootstrap;
mod custom_domains;
mod decay;
mod drafts;
mod email_branding;
mod enterprise;
mod health;
mod mcp;
mod members;
mod notifications;
mod oidc;
mod profile;
mod saml;
mod scim;
mod session_policy;
mod skills;
mod stack_mappings;
mod theme;
mod usage;

const MAX_BUNDLE_BYTES: usize = 5 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        // Prometheus scrape endpoint — no auth, no middleware overhead.
        .route("/metrics", get(metrics::handler))
        .route("/v1/healthz", get(health::healthz))
        .route("/v1/skills", get(skills::list).post(skills::publish))
        .route("/v1/skills/validate", post(skills::validate))
        .route("/v1/skills/{slug}", get(skills::get_one))
        .route("/v1/skills/{slug}/detail", get(skills::get_detail))
        .route("/v1/skills/{slug}/bundle.tar.gz", get(skills::get_bundle))
        .route("/v1/skills/{slug}/skill-md", get(skills::get_skill_md))
        .route("/v1/skills/{slug}/deps", get(skills::get_deps))
        .route("/v1/theme", get(theme::get_theme).put(theme::put_theme))
        .route(
            "/v1/theme/logo",
            get(theme::get_logo)
                .post(theme::post_logo)
                .delete(theme::delete_logo),
        )
        .route(
            "/v1/tenant/session-policy",
            get(session_policy::get_session_policy),
        )
        // Per-tenant CLI startup banner (#9 / Enterprise). No auth —
        // same model as `/v1/theme`. CLI fetches this once per shell
        // session and prints `text` + optional `url` to stderr.
        .route("/v1/tenant/profile/banner", get(profile::get_banner))
        // Bootstrap (Phase 3)
        .route("/v1/bootstrap", get(bootstrap::bootstrap))
        // Drafts (Phase 4 — retrospective capture)
        .route("/v1/drafts", get(drafts::list).post(drafts::create))
        .route(
            "/v1/drafts/{id}",
            get(drafts::get_one).patch(drafts::patch),
        )
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
        // Curator notifications (Phase 5)
        .route(
            "/v1/tenant/notifications",
            get(notifications::get_config).put(notifications::put_config),
        )
        .route(
            "/v1/tenant/notifications/pending-count",
            get(notifications::pending_count),
        )
        // Branded transactional email (#9) — per-tenant SMTP + From
        // override. GET/PUT/DELETE on the config row; POST on /test
        // sends a probe email so admins can verify before relying on
        // it for real notifications.
        .route(
            "/v1/tenant/email-branding",
            get(email_branding::get_config)
                .put(email_branding::put_config)
                .delete(email_branding::delete_config),
        )
        .route(
            "/v1/tenant/email-branding/test",
            post(email_branding::test_config),
        )
        // SIEM export (Phase 5) — fan out audit_events to Splunk HEC,
        // Datadog Logs, or any generic JSON POST receiver.
        .route(
            "/v1/tenant/audit-siem",
            get(audit_siem::get_config).put(audit_siem::put_config),
        )
        // Decay / archive (Phase 5)
        .route("/v1/tenant/skills/decay", get(decay::list_candidates))
        .route("/v1/skills/{slug}/archive", post(decay::archive))
        // Telemetry dashboards (Phase 5)
        .route("/v1/tenant/usage/timeline", get(usage::timeline))
        .route("/v1/tenant/usage/top", get(usage::top))
        // Custom domains (Phase 5 / Enterprise) — tenant-side admin flow
        // for mapping `skills.acme.com` at this backend with ACME
        // issuance handled by the reverse proxy.
        .route(
            "/v1/tenant/custom-domains",
            get(custom_domains::list).post(custom_domains::create),
        )
        .route(
            "/v1/tenant/custom-domains/{id}/verify",
            post(custom_domains::verify),
        )
        .route(
            "/v1/tenant/custom-domains/{id}",
            axum::routing::delete(custom_domains::remove),
        )
        // No auth, no tenant ctx — called by Caddy `on_demand_tls.ask`.
        // 200 = host is verified/active, 404 = unknown. See
        // `docs/enterprise/custom-domains.md`.
        .route(
            "/v1/tenant/custom-domains/{host}/cert-ok",
            get(custom_domains::cert_ok),
        )
        // Stack mappings — curated stack-tag → skill-slug recommendations
        // that drive `skill-pool bootstrap` (Phase 3 finish-up).
        .route(
            "/v1/tenant/stack-mappings",
            get(stack_mappings::list)
                .post(stack_mappings::upsert)
                .delete(stack_mappings::remove),
        )
        // MCP transport (Phase 5) — JSON-RPC adapter for skill search
        .route("/v1/mcp", post(mcp::handle))
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
        // Tenant span middleware: opens a tracing span with tenant.slug,
        // http.method, and http.path before the request enters TraceLayer.
        // This makes TraceLayer's HTTP events children of the tenant span,
        // so log aggregators can filter/group by tenant without parsing paths.
        .layer(middleware::from_fn(tracing_setup::tenant_span_layer))
        .layer(TraceLayer::new_for_http())
        // Prometheus instrumentation: records count, latency, and in-flight
        // for every request that enters the router. Applied after TraceLayer
        // so both observe the same request.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            metrics::track,
        ))
        .with_state(state)
}
