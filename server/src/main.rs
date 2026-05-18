use std::net::SocketAddr;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod admin;
mod audit;
mod auth;
mod bundle;
mod config;
mod error;
mod routes;
mod state;
mod storage;
mod tenant;

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
        action: AdminCmd,
    },
}

#[derive(Subcommand)]
enum AdminCmd {
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load()?;

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve(cfg).await,
        Cmd::Admin { action } => admin::run(&cfg, action).await,
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
