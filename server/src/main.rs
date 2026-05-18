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
