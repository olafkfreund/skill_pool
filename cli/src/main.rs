use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod client;
mod cmd;
mod config;
mod detect;
mod install;
mod manifest;

#[derive(Parser)]
#[command(
    name = "skill-pool",
    version,
    about = "Install, search, and publish Claude Code skills."
)]
struct Cli {
    /// Path to config file (defaults to ~/.skill-pool/config.toml).
    #[arg(long, global = true, env = "SKILL_POOL_CONFIG")]
    config: Option<std::path::PathBuf>,

    /// Override the registry URL for this invocation.
    #[arg(long, global = true, env = "SKILL_POOL_REGISTRY")]
    registry: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter .skill-pool/manifest.toml in the current directory.
    Init,
    /// Authenticate against a registry and persist the token.
    Login {
        #[arg(long)]
        registry: String,
        #[arg(long)]
        tenant: String,
    },
    /// Install everything in the project manifest into .claude/skills/.
    Ensure {
        /// Suppress per-skill progress lines. Errors still surface.
        /// Used by the direnv hook to stay silent on the happy path.
        #[arg(long)]
        quiet: bool,
    },
    /// Add a skill to the manifest and install it.
    Add { slug: String },
    /// Search the registry. With no query, lists all skills.
    Search {
        /// Optional substring matched against slug and description (ILIKE).
        query: Option<String>,
        /// Comma-separated tags; ALL must be present on a result.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Limit results (1..200).
        #[arg(long)]
        limit: Option<u32>,
        /// Emit JSON instead of a table — useful in scripts.
        #[arg(long)]
        json: bool,
    },
    /// Publish a local skill directory to the registry.
    Publish {
        #[arg(value_name = "DIR")]
        dir: std::path::PathBuf,
        /// Override the slug. Defaults to the frontmatter `name`, then the directory name.
        #[arg(long)]
        slug: Option<String>,
        /// Required. Semver string for this publish (e.g. 1.0.0).
        #[arg(long)]
        version: String,
    },
    /// Diagnose: list loaded skills, dangling symlinks, drift.
    Doctor,
    /// Detect the current project's stack from filesystem fingerprints.
    Detect {
        /// Emit JSON instead of a human-friendly summary.
        #[arg(long)]
        json: bool,
    },
    /// Install the direnv helper into ~/.config/direnv/lib so .envrc files
    /// can use `use skill_pool`. Embedded at compile time — no network.
    DirenvInstall {
        /// Overwrite if a different version is already present.
        #[arg(long)]
        force: bool,
    },
    /// Detect the stack, ask the registry which skills it recommends, then
    /// (with confirmation) add them to the manifest and install. The
    /// canonical "onboard a new project" command.
    Bootstrap {
        /// Skip the Y/n confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Re-run detection even if the manifest already has a stack.
        #[arg(long)]
        detect: bool,
        /// Show what would be added without saving the manifest or calling ensure.
        #[arg(long)]
        dry_run: bool,
    },
    /// Install a Claude Code SessionStart hook that runs `skill-pool ensure
    /// --quiet` on every session start. Writes to .claude/settings.json in
    /// the current project. Preserves all other settings.
    HookInstall {
        /// Remove the hook instead of installing it.
        #[arg(long)]
        remove: bool,
        /// Print the merged settings.json content to stdout; don't write.
        #[arg(long)]
        print: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,skill_pool=info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load(cli.config.as_deref(), cli.registry.as_deref())?;

    match cli.command {
        Cmd::Init => cmd::init::run(&cfg),
        Cmd::Login { registry, tenant } => cmd::login::run(&cfg, &registry, &tenant).await,
        Cmd::Ensure { quiet } => cmd::ensure::run_with_quiet(&cfg, quiet).await,
        Cmd::Add { slug } => cmd::add::run(&cfg, &slug).await,
        Cmd::Search {
            query,
            tags,
            limit,
            json,
        } => cmd::search::run(&cfg, query.as_deref(), &tags, limit, json).await,
        Cmd::Publish { dir, slug, version } => {
            cmd::publish::run(&cfg, &dir, slug.as_deref(), &version).await
        }
        Cmd::Doctor => cmd::doctor::run(&cfg),
        Cmd::Detect { json } => cmd::detect::run(json),
        Cmd::Bootstrap {
            yes,
            detect,
            dry_run,
        } => cmd::bootstrap::run(&cfg, detect, yes, dry_run).await,
        Cmd::DirenvInstall { force } => cmd::direnv_install::run(force),
        Cmd::HookInstall { remove, print } => cmd::hook_install::run(remove, print),
    }
}
