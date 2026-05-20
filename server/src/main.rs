use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use skill_pool_server::{admin, config, notify, routes, state, telemetry, tracing_setup, worker};

#[derive(Parser)]
#[command(
    name = "skill-pool-server",
    version,
    about = "skill-pool registry HTTP server"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the HTTP server (default if no subcommand given).
    Serve,
    /// Ops actions: create tenants, mint tokens. Run server-side; no network exposure.
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    /// Create a tenant.
    TenantCreate {
        #[arg(long)]
        slug: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "team")]
        plan: String,
    },
    /// Set or clear a tenant's session idle-timeout policy. Applies to
    /// the web portal's session cookie at next login. Range: 1 minute
    /// to 30 days (CHECK-enforced). Use `--clear` to revert to the
    /// 14-day default. See `docs/enterprise/session-policy.md`.
    TenantSessionPolicy {
        #[arg(long)]
        slug: String,
        /// Set the maximum session age in days (1..=30). Conflicts with `--clear`.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=30))]
        max_age_days: Option<u32>,
        /// Clear any custom policy on this tenant; revert to system default.
        #[arg(long, conflicts_with = "max_age_days")]
        clear: bool,
    },
    /// Set or clear a tenant's data-residency fields (region tag,
    /// per-tenant bundle storage URI override). Either or both may be
    /// passed per call; omitted fields are unchanged. To clear a value,
    /// pass an empty string. The storage URI is validated synchronously.
    /// See `docs/enterprise/data-residency.md`.
    TenantResidency {
        #[arg(long)]
        slug: String,
        #[arg(long)]
        region: Option<String>,
        #[arg(long)]
        storage_uri: Option<String>,
    },
    /// Set, update, or clear a tenant's CLI startup banner (#9). The CLI
    /// fetches this once per shell session and prints `text` + optional
    /// `url` to stderr before running the user's subcommand. Constraints:
    /// `text` ≤240 chars; `url` must be `https://` with no whitespace.
    /// Use `--clear` to wipe both columns. See
    /// `docs/enterprise/branded-cli-banner.md`.
    TenantBannerSet {
        #[arg(long)]
        slug: String,
        /// One-line greeting (≤240 chars). Pass an empty string to
        /// clear just this column while leaving `--url` alone.
        #[arg(long, conflicts_with = "clear")]
        text: Option<String>,
        /// Optional `https://` link printed below the greeting. Pass
        /// empty string to clear just this column.
        #[arg(long, conflicts_with = "clear")]
        url: Option<String>,
        /// Clear both banner_text and banner_url for this tenant.
        #[arg(long, conflicts_with_all = ["text", "url"])]
        clear: bool,
    },
    /// Set or clear a tenant's per-tenant rate limits (#8 §L20). Plan
    /// defaults apply when both columns are NULL: team (600/60),
    /// business (3000/300), enterprise (30000/1000). Range: rpm
    /// 1..=100000, burst 1..=10000 (DB CHECK enforced). Use `--clear`
    /// to revert to the plan default. See
    /// `docs/enterprise/rate-limits.md`.
    TenantRateLimits {
        #[arg(long)]
        slug: String,
        /// Requests per 60-second window (1..=100000). Conflicts with `--clear`.
        #[arg(long, conflicts_with = "clear")]
        rpm: Option<u32>,
        /// Requests per 1-second window (1..=10000). Conflicts with `--clear`.
        #[arg(long, conflicts_with = "clear")]
        burst: Option<u32>,
        /// Clear both overrides; revert to plan defaults.
        #[arg(long, conflicts_with_all = ["rpm", "burst"])]
        clear: bool,
    },
    /// Hard-delete a tenant and all its data via ON DELETE CASCADE.
    /// Bundle storage is NOT swept — the command prints the storage prefix
    /// for a separate operator sweep (or retention). Pair with SIEM export
    /// before running if you need to preserve `audit_events`.
    TenantDelete {
        #[arg(long)]
        slug: String,
        /// Skip the typed-slug confirmation prompt. Use only in scripted /
        /// migration contexts where the caller is sure.
        #[arg(long)]
        confirm: bool,
    },
    /// Mint a new API token for a tenant. Prints the raw token once.
    TokenCreate {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "skills:read skills:publish")]
        scope: String,
    },
    /// Configure (or update) SAML 2.0 SSO for a tenant.
    SamlSet {
        #[arg(long)]
        tenant: String,
        /// IdP entity ID (usually a URI).
        #[arg(long)]
        idp_entity_id: String,
        /// IdP SSO URL — where we send the user for sign-in.
        #[arg(long)]
        idp_sso_url: String,
        /// Path to a PEM file containing the IdP signing certificate.
        #[arg(long)]
        idp_cert_path: std::path::PathBuf,
        /// Optional SP entity ID override (defaults to `urn:skill-pool:tenant:<slug>`).
        #[arg(long)]
        sp_entity_id: Option<String>,
        #[arg(long, default_value = "viewer")]
        default_role: String,
    },
    /// Configure (or update) OIDC SSO for a tenant.
    SsoSet {
        #[arg(long)]
        tenant: String,
        /// OIDC issuer URL, e.g. https://acme.okta.com/oauth2/default
        #[arg(long)]
        issuer: String,
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        client_secret: String,
        /// Role granted to first-time signers (viewer|publisher|curator|admin).
        #[arg(long, default_value = "viewer")]
        default_role: String,
    },
    /// Map an IdP group to a tenant role. Re-evaluated on every sign-in.
    GroupMapSet {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        group: String,
        #[arg(long)]
        role: String,
    },
    /// List configured IdP group → role mappings for a tenant.
    GroupMapList {
        #[arg(long)]
        tenant: String,
    },
    /// Remove an IdP group → role mapping.
    GroupMapRemove {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        group: String,
    },
    /// Map a stack tag (e.g. "rust") to a skill slug. Phase-3 bootstrap.
    StackMapSet {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        stack: String,
        #[arg(long)]
        skill: String,
    },
    /// List stack-tag → skill mappings for a tenant.
    StackMapList {
        #[arg(long)]
        tenant: String,
    },
    /// Remove a stack-tag → skill mapping.
    StackMapRemove {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        stack: String,
        #[arg(long)]
        skill: String,
    },
    /// Configure (or update) per-tenant branded transactional email
    /// (#9). Sets the From line, optional Reply-To / footer, and the
    /// dedicated SMTP relay URL. The SMTP password is read from stdin
    /// (so it doesn't show up in shell history) and stored encrypted
    /// at rest under the `SKILL_POOL_EMAIL_SECRET_KEY` env. See
    /// `docs/enterprise/branded-emails.md`.
    EmailBrandingSet {
        #[arg(long)]
        tenant: String,
        /// From address, e.g. `noreply@acme.example.com`. May include
        /// a display name like `"Acme" <noreply@acme.example.com>` but
        /// `--from-name` is preferred.
        #[arg(long)]
        from_addr: String,
        /// Optional display name. Combined with from_addr at send time.
        #[arg(long)]
        from_name: Option<String>,
        /// Optional Reply-To address.
        #[arg(long)]
        reply_to: Option<String>,
        /// SMTP URL with no password baked in, e.g.
        /// `smtps://user@smtp.eu.example.com:465`. The scheme must be
        /// `smtp://` or `smtps://`.
        #[arg(long)]
        smtp_url: String,
        /// Optional plain-text footer appended to each outbound mail.
        #[arg(long)]
        footer_html: Option<String>,
    },
    /// Send a probe email through the tenant's branded SMTP transport.
    /// Useful for verifying configuration before relying on it for
    /// real notifications.
    EmailBrandingTest {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        to: String,
    },
    /// Per-tenant custom domain admin. Lets a tenant pin
    /// `skills.acme.com` at this backend; cert issuance is the reverse
    /// proxy's job (Caddy `on_demand_tls` / Traefik HTTP-01). See
    /// `docs/enterprise/custom-domains.md`.
    CustomDomain {
        #[arg(long)]
        tenant: String,
        #[command(subcommand)]
        action: CustomDomainAction,
    },
    /// Backfill description_embedding for skills that pre-date Phase 5.
    /// Walks `skills` rows with NULL embedding, computes one via the
    /// configured Embedder, and updates the column. Skipped silently on
    /// rows the embedder declines (None return).
    BackfillEmbeddings {
        /// Restrict to one tenant. Default: all tenants.
        #[arg(long)]
        tenant: Option<String>,
        /// Stop after processing this many rows (cost cap).
        #[arg(long, default_value_t = 500)]
        limit: usize,
        /// Show what would happen without writing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum CustomDomainAction {
    /// Claim a hostname on behalf of a tenant. Prints the DNS TXT
    /// record the tenant admin needs to add.
    Add {
        #[arg(long)]
        hostname: String,
    },
    /// List all custom domains for a tenant with their status.
    List,
    /// Print the verify command for a pending domain. Actual DNS lookup
    /// happens via the HTTP endpoint (single code path).
    Verify {
        #[arg(long)]
        id: uuid::Uuid,
    },
    /// Operator override: skip DNS, flip status straight to `active`.
    /// Use for private CAs / air-gapped deploys.
    Activate {
        #[arg(long)]
        id: uuid::Uuid,
    },
    /// Withdraw a custom-domain claim. Removes the row; the next cache
    /// refresh will drop it from the in-process map.
    Remove {
        #[arg(long)]
        id: uuid::Uuid,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // OTel SDK first (so the tracer provider exists), then subscriber.
    // `telemetry::init` is a no-op without the `otlp` feature.
    telemetry::init()?;
    tracing_setup::init();

    let cli = Cli::parse();
    let cfg = config::Config::load()?;

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve(cfg).await,
        Cmd::Admin { action } => {
            match action {
                AdminAction::TenantCreate { slug, name, plan } => {
                    let db = admin::connect(&cfg).await?;
                    admin::create_tenant(&db, &slug, &name, &plan).await?;
                    println!("\nnext: skill-pool-server admin token-create --tenant {slug} --name bootstrap");
                    Ok(())
                }
                AdminAction::TenantSessionPolicy {
                    slug,
                    max_age_days,
                    clear,
                } => {
                    let db = admin::connect(&cfg).await?;
                    let secs = if clear {
                        None
                    } else {
                        Some(
                            max_age_days
                                .ok_or_else(|| {
                                    anyhow::anyhow!("pass --max-age-days N or --clear")
                                })? as i32
                                * 24
                                * 60
                                * 60,
                        )
                    };
                    admin::set_session_max_age(&db, &slug, secs).await
                }
                AdminAction::TenantRateLimits {
                    slug,
                    rpm,
                    burst,
                    clear,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::set_tenant_rate_limits(
                        &db,
                        &slug,
                        rpm.map(|v| v as i32),
                        burst.map(|v| v as i32),
                        clear,
                    )
                    .await
                }
                AdminAction::TenantBannerSet {
                    slug,
                    text,
                    url,
                    clear,
                } => {
                    if !clear && text.is_none() && url.is_none() {
                        return Err(anyhow::anyhow!(
                            "pass --text, --url, or --clear (at least one required)"
                        ));
                    }
                    let db = admin::connect(&cfg).await?;
                    admin::set_tenant_banner(
                        &db,
                        &slug,
                        text.as_deref(),
                        url.as_deref(),
                        clear,
                    )
                    .await
                }
                AdminAction::TenantResidency {
                    slug,
                    region,
                    storage_uri,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::set_tenant_residency(
                        &db,
                        &slug,
                        region.as_deref(),
                        storage_uri.as_deref(),
                    )
                    .await
                }
                AdminAction::TenantDelete { slug, confirm } => {
                    if !confirm {
                        use std::io::{BufRead, Write};
                        print!(
                            "This will DELETE tenant `{slug}` and ALL associated rows\n\
                             (skills, drafts, tokens, audit_events, theme, sso config, …)\n\
                             via ON DELETE CASCADE. Bundle storage is NOT touched.\n\
                             Type the slug to confirm: "
                        );
                        std::io::stdout().flush().ok();
                        let stdin = std::io::stdin();
                        let mut line = String::new();
                        stdin.lock().read_line(&mut line)?;
                        if line.trim() != slug {
                            return Err(anyhow::anyhow!("confirmation mismatch; nothing deleted"));
                        }
                    }
                    let db = admin::connect(&cfg).await?;
                    let deleted = admin::delete_tenant(&db, &slug).await?;
                    println!("tenant deleted");
                    println!("  id:   {}", deleted.id);
                    println!("  slug: {}", deleted.slug);
                    println!();
                    println!("Bundle storage was NOT swept. To reclaim space, run:");
                    println!("  # fs://    rm -rf <storage_root>/{}", deleted.id);
                    println!("  # s3://    aws s3 rm s3://<bucket>/{}/ --recursive", deleted.id);
                    Ok(())
                }
                AdminAction::TokenCreate {
                    tenant,
                    name,
                    scope,
                } => {
                    let db = admin::connect(&cfg).await?;
                    let created = admin::create_token(&db, &tenant, &name, &scope).await?;
                    println!("token created");
                    println!("  id:     {}", created.id);
                    println!("  tenant: {tenant}");
                    println!("  scope:  {scope}");
                    println!();
                    println!("RAW TOKEN (shown once — copy now):");
                    println!("  {}", created.raw_token);
                    Ok(())
                }
                AdminAction::SsoSet {
                    tenant,
                    issuer,
                    client_id,
                    client_secret,
                    default_role,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::set_sso(
                        &db,
                        &tenant,
                        &issuer,
                        &client_id,
                        &client_secret,
                        &default_role,
                    )
                    .await
                }
                AdminAction::SamlSet {
                    tenant,
                    idp_entity_id,
                    idp_sso_url,
                    idp_cert_path,
                    sp_entity_id,
                    default_role,
                } => {
                    let cert = std::fs::read_to_string(&idp_cert_path)
                        .map_err(|e| anyhow::anyhow!("read {}: {e}", idp_cert_path.display()))?;
                    let db = admin::connect(&cfg).await?;
                    admin::set_saml(
                        &db,
                        &tenant,
                        &idp_entity_id,
                        &idp_sso_url,
                        &cert,
                        sp_entity_id.as_deref(),
                        &default_role,
                    )
                    .await
                }
                AdminAction::GroupMapSet {
                    tenant,
                    group,
                    role,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::set_role_mapping(&db, &tenant, &group, &role).await
                }
                AdminAction::GroupMapList { tenant } => {
                    let db = admin::connect(&cfg).await?;
                    admin::list_role_mappings(&db, &tenant).await
                }
                AdminAction::GroupMapRemove { tenant, group } => {
                    let db = admin::connect(&cfg).await?;
                    admin::remove_role_mapping(&db, &tenant, &group).await
                }
                AdminAction::StackMapSet {
                    tenant,
                    stack,
                    skill,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::set_stack_mapping(&db, &tenant, &stack, &skill).await
                }
                AdminAction::StackMapList { tenant } => {
                    let db = admin::connect(&cfg).await?;
                    admin::list_stack_mappings(&db, &tenant).await
                }
                AdminAction::StackMapRemove {
                    tenant,
                    stack,
                    skill,
                } => {
                    let db = admin::connect(&cfg).await?;
                    admin::remove_stack_mapping(&db, &tenant, &stack, &skill).await
                }
                AdminAction::EmailBrandingSet {
                    tenant,
                    from_addr,
                    from_name,
                    reply_to,
                    smtp_url,
                    footer_html,
                } => {
                    use std::io::{BufRead, Write};
                    eprint!("SMTP password (will be encrypted at rest): ");
                    std::io::stderr().flush().ok();
                    let stdin = std::io::stdin();
                    let mut line = String::new();
                    stdin.lock().read_line(&mut line)?;
                    let smtp_password = line.trim_end_matches(['\r', '\n']).to_string();
                    if smtp_password.is_empty() {
                        return Err(anyhow::anyhow!("SMTP password must not be empty"));
                    }
                    let db = admin::connect(&cfg).await?;
                    admin::set_email_branding(
                        &db,
                        &tenant,
                        admin::EmailBrandingArgs {
                            from_addr: &from_addr,
                            from_name: from_name.as_deref(),
                            reply_to: reply_to.as_deref(),
                            smtp_url: &smtp_url,
                            smtp_password: &smtp_password,
                            footer_html: footer_html.as_deref(),
                        },
                    )
                    .await
                }
                AdminAction::EmailBrandingTest { tenant, to } => {
                    let db = admin::connect(&cfg).await?;
                    admin::email_branding_test(&db, &tenant, &to).await
                }
                AdminAction::CustomDomain { tenant, action } => {
                    let db = admin::connect(&cfg).await?;
                    match action {
                        CustomDomainAction::Add { hostname } => {
                            admin::add_custom_domain(&db, &tenant, &hostname).await
                        }
                        CustomDomainAction::List => {
                            admin::list_custom_domains(&db, &tenant).await
                        }
                        CustomDomainAction::Verify { id } => {
                            admin::verify_custom_domain(&db, &tenant, id).await
                        }
                        CustomDomainAction::Activate { id } => {
                            admin::activate_custom_domain(&db, &tenant, id).await
                        }
                        CustomDomainAction::Remove { id } => {
                            admin::remove_custom_domain(&db, &tenant, id).await
                        }
                    }
                }
                AdminAction::BackfillEmbeddings {
                    tenant,
                    limit,
                    dry_run,
                } => {
                    let db = admin::connect(&cfg).await?;
                    let embedder = skill_pool_server::embedding::from_config(&cfg.embedding)?;
                    admin::backfill_embeddings(&db, embedder.as_ref(), tenant.as_deref(), limit, dry_run)
                        .await
                }
            }
        }
    }
}

async fn serve(cfg: config::Config) -> Result<()> {
    tracing::info!(addr = %cfg.bind, "skill-pool-server starting");

    let state = state::AppState::new(&cfg).await?;

    // Apply any pending migrations before we start serving traffic.
    // Today's failure mode without this: a fresh deploy connects to a
    // Postgres that's missing the latest migrations and every query
    // touching the new columns 500s. Running migrations at boot is the
    // idiomatic sqlx pattern and matches what the integration-test
    // harness does. Idempotent — sqlx's `_sqlx_migrations` table tracks
    // applied versions so re-runs are no-ops.
    sqlx::migrate!("./migrations")
        .run(state.db())
        .await
        .context("apply pending database migrations at boot")?;
    tracing::info!("database migrations up to date");

    // Start the background custom-domain cache refresher so admins see
    // verified/active domains flow into request routing without manual
    // server reloads. Detached: the JoinHandle is dropped, the task
    // ends when the runtime shuts down.
    let _refresher = state.spawn_custom_domain_refresher();

    // Spawn the job-queue worker when Redis + queue are both
    // available. The watch channel is shared between axum's graceful
    // shutdown signal and the worker's loop so a SIGTERM drains both
    // sides cleanly. When Redis isn't configured we skip the worker
    // and all consumers fall back to inline behaviour (notify.rs
    // branches on `state.queue()`).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_handle = if let Some(q) = state.queue() {
        let mut w = worker::Worker::new(q.clone(), shutdown_rx.clone());
        w.register("email", notify::EmailHandler::new(state.clone()));
        Some(tokio::spawn(w.run()))
    } else {
        None
    };

    // Background decay sweep (#7 lifecycle). Flips long-stale skills
    // to `status = 'archive_candidate'` so curators see them flagged
    // proactively. Configurable via SKILL_POOL_DECAY_CHECK_INTERVAL_SECS;
    // set to 0 to disable (the on-demand /v1/tenant/skills/decay endpoint
    // continues to work). Shares the same shutdown channel as the worker.
    let decay_handle = spawn_decay_sweep(state.db().clone(), cfg.decay_check_interval_secs, shutdown_rx.clone());

    // Background plan-refresh sweep (PL). Wakes every 60s, queries for
    // projects whose auto-refresh interval has elapsed, and calls
    // refresh_plan_from_source for each. Bounded concurrency: at most 4
    // projects per tick. Failures are persisted by the admin fn itself;
    // the sweep logs at warn and continues.
    let plan_refresh_handle =
        spawn_plan_refresh_sweep(state.db().clone(), state.http_client().clone(), shutdown_rx.clone());

    let app = routes::router(state);

    let addr: SocketAddr = cfg.bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // axum returned → user requested shutdown OR the listener died.
    // Tell the worker to drain. We give it 30s to finish whatever it's
    // doing; longer than that and we time out and log a warning rather
    // than block the process from exiting.
    let _ = shutdown_tx.send(true);
    if let Some(handle) = worker_handle {
        match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
            Ok(Ok(())) => tracing::info!("queue worker shut down cleanly"),
            Ok(Err(e)) => tracing::warn!(error = %e, "queue worker task panicked during shutdown"),
            Err(_) => tracing::warn!("queue worker shutdown timed out after 30s"),
        }
    }
    if let Some(handle) = decay_handle {
        // Decay sweep is purely periodic; 5s is plenty for the in-flight
        // UPDATE (if any) to finish.
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => tracing::info!("decay sweep shut down cleanly"),
            Ok(Err(e)) => tracing::warn!(error = %e, "decay sweep task panicked"),
            Err(_) => tracing::warn!("decay sweep shutdown timed out"),
        }
    }
    if let Some(handle) = plan_refresh_handle {
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => tracing::info!("plan refresh sweep shut down cleanly"),
            Ok(Err(e)) => tracing::warn!(error = %e, "plan refresh sweep task panicked"),
            Err(_) => tracing::warn!("plan refresh sweep shutdown timed out"),
        }
    }

    telemetry::shutdown();
    Ok(())
}

/// Spawn the background decay sweep task. Returns `None` when the
/// configured interval is 0 (disabled) so callers can skip the
/// shutdown join. The task listens on the shared shutdown channel so
/// SIGTERM drains it alongside the queue worker.
fn spawn_decay_sweep(
    db: sqlx::PgPool,
    interval_secs: u32,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    if interval_secs == 0 {
        tracing::info!("decay_check_interval_secs=0; background sweep disabled");
        return None;
    }
    let dur = std::time::Duration::from_secs(interval_secs as u64);
    Some(tokio::spawn(async move {
        tracing::info!(interval_secs, "decay sweep task starting");
        let mut tick = tokio::time::interval(dur);
        // First tick fires immediately. Consume it; otherwise a server
        // restart loop on a misconfigured deployment would hammer the
        // DB. The first real sweep runs one `interval` after startup.
        tick.tick().await;
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("decay sweep shutting down");
                        return;
                    }
                }
                _ = tick.tick() => {
                    match skill_pool_server::routes::decay::sweep(
                        &db,
                        skill_pool_server::routes::decay::DEFAULT_SWEEP_STALE_DAYS,
                        skill_pool_server::routes::decay::DEFAULT_SWEEP_MIN_USES,
                    ).await {
                        Ok(n) if n > 0 => tracing::info!(flipped = n, "decay sweep flipped rows to archive_candidate"),
                        Ok(_) => tracing::debug!("decay sweep: no stale skills"),
                        Err(e) => tracing::warn!(error = %e, "decay sweep failed; continuing"),
                    }
                }
            }
        }
    }))
}

/// Spawn the background plan-refresh sweep task.
///
/// Wakes every 60 seconds. Queries `tenant_projects` for rows where
/// `plan_auto_refresh_interval_secs IS NOT NULL` and the last refresh is
/// past due. Processes at most 4 projects per tick (bounded concurrency).
/// Returns `None` immediately — the task runs until the shutdown channel fires.
fn spawn_plan_refresh_sweep(
    db: sqlx::PgPool,
    http: reqwest::Client,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    const SWEEP_INTERVAL_SECS: u64 = 60;
    const MAX_PER_TICK: usize = 4;

    Some(tokio::spawn(async move {
        tracing::info!("plan refresh sweep task starting");
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(SWEEP_INTERVAL_SECS));
        // Skip the immediate first tick — identical to the decay sweep pattern.
        tick.tick().await;
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("plan refresh sweep shutting down");
                        return;
                    }
                }
                _ = tick.tick() => {
                    // Query for projects whose refresh interval has elapsed.
                    // tenant_slug is needed by refresh_plan_from_source; project slug too.
                    let due: Vec<(String, String)> = match sqlx::query_as(
                        "SELECT t.slug, tp.slug::text \
                         FROM tenant_projects tp \
                         JOIN tenants t ON t.id = tp.tenant_id \
                         WHERE tp.plan_auto_refresh_interval_secs IS NOT NULL \
                           AND ( \
                             tp.last_plan_refresh_at IS NULL \
                             OR tp.last_plan_refresh_at + \
                                (tp.plan_auto_refresh_interval_secs || ' seconds')::interval < now() \
                           ) \
                         LIMIT $1",
                    )
                    .bind(MAX_PER_TICK as i64)
                    .fetch_all(&db)
                    .await
                    {
                        Ok(rows) => rows,
                        Err(e) => {
                            tracing::warn!(error = %e, "plan refresh sweep query failed; will retry");
                            continue;
                        }
                    };

                    for (tenant_slug, project_slug) in due {
                        match skill_pool_server::admin::refresh_plan_from_source(
                            &db,
                            &http,
                            &tenant_slug,
                            &project_slug,
                        )
                        .await
                        {
                            Ok(skill_pool_server::admin::RefreshOutcome::Updated(p)) => {
                                tracing::info!(
                                    tenant = %tenant_slug,
                                    project = %project_slug,
                                    version = p.version,
                                    "plan auto-refresh: new version created"
                                );
                            }

                            Ok(skill_pool_server::admin::RefreshOutcome::Unchanged) => {
                                tracing::debug!(
                                    tenant = %tenant_slug,
                                    project = %project_slug,
                                    "plan auto-refresh: content unchanged"
                                );
                            }
                            Ok(skill_pool_server::admin::RefreshOutcome::Failed(reason)) => {
                                tracing::warn!(
                                    tenant = %tenant_slug,
                                    project = %project_slug,
                                    error = %reason,
                                    "plan auto-refresh failed; last-good version retained"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    tenant = %tenant_slug,
                                    project = %project_slug,
                                    error = %e,
                                    "plan auto-refresh error"
                                );
                            }
                        }

                        // Update last_plan_refresh_at regardless of outcome.
                        let _ = sqlx::query(
                            "UPDATE tenant_projects \
                             SET last_plan_refresh_at = now() \
                             WHERE slug = $1 \
                               AND tenant_id = (SELECT id FROM tenants WHERE slug = $2)",
                        )
                        .bind(&project_slug)
                        .bind(&tenant_slug)
                        .execute(&db)
                        .await;
                    }
                }
            }
        }
    }))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; draining");
}
