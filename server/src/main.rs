use std::net::SocketAddr;

use anyhow::Result;
use clap::{Parser, Subcommand};

use skill_pool_server::{admin, config, routes, state, telemetry, tracing_setup};

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
    // Start the background custom-domain cache refresher so admins see
    // verified/active domains flow into request routing without manual
    // server reloads. Detached: the JoinHandle is dropped, the task
    // ends when the runtime shuts down.
    let _refresher = state.spawn_custom_domain_refresher();
    let app = routes::router(state);

    let addr: SocketAddr = cfg.bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    telemetry::shutdown();
    Ok(())
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
