use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::metrics;
use crate::rate_limit;
use crate::state::AppState;
use crate::tracing_setup;

mod audit_siem;
mod bootstrap;
mod custom_domains;
pub mod decay;
mod drafts;
mod email_branding;
mod enterprise;
mod health;
mod mcp;
mod members;
mod notifications;
mod og;
mod oidc;
mod profile;
mod saml;
mod scim;
mod session_policy;
mod skills;
mod sso_admin;
mod projects;
mod project_plans;
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
        .route("/v1/skills/{slug}/versions", get(skills::get_versions))
        .route("/v1/theme", get(theme::get_theme).put(theme::put_theme))
        .route(
            "/v1/theme/logo",
            get(theme::get_logo)
                .post(theme::post_logo)
                .delete(theme::delete_logo),
        )
        .route(
            "/v1/theme/favicon",
            get(theme::get_favicon)
                .post(theme::post_favicon)
                .delete(theme::delete_favicon),
        )
        .route("/v1/theme/fonts", get(theme::get_fonts))
        // Per-tenant custom CSS overlay (#9). POST/DELETE require
        // `tenant:admin`; the GET is public (matches the rest of the
        // /v1/theme/* surface) and adds a strict CSP + cache headers.
        .route(
            "/v1/theme/custom-css",
            post(theme::post_custom_css).delete(theme::delete_custom_css),
        )
        .route("/v1/theme/custom.css", get(theme::get_custom_css))
        // Open Graph card generator (#9). Public, tenant-resolved —
        // social-platform crawlers don't carry auth. Returns SVG with a
        // 24h Cache-Control + ETag.
        .route("/v1/og", get(og::og_image))
        .route(
            "/v1/tenant/session-policy",
            get(session_policy::get_session_policy),
        )
        // Per-tenant CLI startup banner (#9 / Enterprise). No auth —
        // same model as `/v1/theme`. CLI fetches this once per shell
        // session and prints `text` + optional `url` to stderr.
        .route("/v1/tenant/profile/banner", get(profile::get_banner))
        // Personal API tokens (#4). Session-authenticated callers only —
        // bare API tokens cannot manage other tokens. POST returns the
        // raw token *once*; subsequent listings only carry the prefix.
        .route(
            "/v1/profile/tokens",
            get(profile::list_tokens).post(profile::create_token),
        )
        .route("/v1/profile/tokens/{id}", delete(profile::revoke_token))
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
        // Admin SSO config surface (#4) — separate from the runtime
        // `/v1/auth/oidc/*` and `/v1/auth/saml/*` endpoints, which are
        // unauthenticated browser-redirect flows. This surface is
        // tenant-admin JSON CRUD for the SvelteKit admin portal.
        .route(
            "/v1/tenant/sso",
            get(sso_admin::get_config).delete(sso_admin::delete_config),
        )
        .route("/v1/tenant/sso/oidc", axum::routing::put(sso_admin::put_oidc))
        .route("/v1/tenant/sso/saml", axum::routing::put(sso_admin::put_saml))
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
        // CLI-driven usage event (#7 lifecycle). `skill-pool ensure`
        // POSTs a `view` event per installed skill so the decay model
        // sees session-load activity, not just bundle downloads.
        .route("/v1/usage", post(usage::post_event))
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
        // Projects — named bundles of skills/agents/commands (Layer 2).
        // Write routes (create, update, delete, set-items) require tenant:admin.
        // The resolve endpoint only requires any authenticated tenant member.
        .route(
            "/v1/tenant/projects",
            get(projects::list).post(projects::create),
        )
        .route(
            "/v1/tenant/projects/{slug}",
            get(projects::detail)
                .patch(projects::patch)
                .delete(projects::delete),
        )
        .route(
            "/v1/tenant/projects/{slug}/items",
            axum::routing::put(projects::put_items),
        )
        .route("/v1/projects/resolve", get(projects::resolve))
        // Project Plans (PL) — per-project markdown documents.
        // Write routes (import, refresh, activate) require tenant:admin.
        // Read routes (get, list, get-version) require any authenticated member.
        .route(
            "/v1/tenant/projects/{slug}/plan",
            post(project_plans::import_plan).get(project_plans::get_plan),
        )
        .route(
            "/v1/tenant/projects/{slug}/plan/versions",
            get(project_plans::list_versions),
        )
        .route(
            "/v1/tenant/projects/{slug}/plan/versions/{v}",
            get(project_plans::get_version),
        )
        .route(
            "/v1/tenant/projects/{slug}/plan/refresh",
            post(project_plans::refresh_plan),
        )
        .route(
            "/v1/tenant/projects/{slug}/plan/activate",
            post(project_plans::activate_version),
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
        // Per-tenant rate limiter (#8 §L20). Sits between TraceLayer and
        // metrics::track so 429 responses still show up in Prometheus,
        // but spans/traces aren't opened for traffic we're throttling.
        // Fails open if Redis is unavailable.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::rate_limit_layer,
        ))
        // Prometheus instrumentation: records count, latency, and in-flight
        // for every request that enters the router. Applied after TraceLayer
        // so both observe the same request.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            metrics::track,
        ))
        .with_state(state)
}
