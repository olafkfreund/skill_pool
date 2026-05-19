use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod anthropic;
mod capturer;
mod client;
mod cmd;
mod config;
mod detect;
mod install;
mod manifest;
mod scorer;
mod secret_scan;

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
        /// Semantic search: rank by cosine similarity of `description_embedding`
        /// to this query. Requires the server to be built with
        /// `--features fastembed`.
        #[arg(long, value_name = "TEXT")]
        semantic: Option<String>,
        /// Minimum similarity (0.0..1.0) when `--semantic` is set. Default 0.0.
        #[arg(long, value_name = "FLOAT")]
        min_similarity: Option<f32>,
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
        /// Catalog kind. Defaults to `skill`. Use `agent` or `command` to
        /// publish into the parallel catalog surfaces.
        #[arg(long, value_parser = ["skill", "agent", "command"], default_value = "skill")]
        kind: String,
    },
    /// Capture a local skill directory as a draft (Phase 4). Drafts land in
    /// the curator inbox; a reviewer assigns a version at publish time.
    Capture {
        #[arg(value_name = "DIR")]
        dir: std::path::PathBuf,
        /// Override the slug. Defaults to the frontmatter `name`, then the directory name.
        #[arg(long)]
        slug: Option<String>,
        /// Free-form note for the reviewer (why this matters, session context).
        #[arg(long)]
        notes: Option<String>,
        /// Extra tags to attach (comma-separated). Merged with frontmatter tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Skip the secret-scan quality gate. Findings are logged but the
        /// capture proceeds. Use when the skill is *about* secret handling
        /// (e.g. rotation runbook) and a regex false-positive would block it.
        #[arg(long)]
        allow_secret: bool,
    },
    /// Score a session for "this was worth capturing" signals (Phase 4.5).
    /// Designed to run as the Claude Code Stop hook — reads the hook payload
    /// from stdin, runs cheap deterministic rules (no LLM), persists the
    /// score under ~/.skill-pool/sessions/. Exits 0 on any error so the
    /// hook never blocks the user.
    CaptureScore {
        /// Read the hook payload from a file instead of stdin.
        #[arg(long, value_name = "PATH")]
        from_file: Option<std::path::PathBuf>,
    },
    /// List persisted session scores, ranked. Star marks draft-worthy
    /// sessions. `--json` dumps the raw records.
    CaptureStatus {
        #[arg(long)]
        json: bool,
    },
    /// Run the Phase 4.6 LLM capturer over draft-worthy sessions. Two-stage
    /// pipeline: Haiku extractor → Sonnet drafter → POST /v1/drafts.
    /// Idempotent: a session whose `capture_state` is set is skipped.
    /// Designed to be invoked by a systemd user timer (or cron).
    CaptureRun {
        /// Maximum sessions to process this pass (cost cap).
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Show which sessions would be processed without calling the LLM.
        #[arg(long)]
        dry_run: bool,
        /// Override the Stage 1 (extractor) model.
        #[arg(long)]
        stage1_model: Option<String>,
        /// Override the Stage 2 (drafter) model.
        #[arg(long)]
        stage2_model: Option<String>,
        /// Skip the secret-scan quality gate. Findings are logged as
        /// warnings but the pipeline proceeds. Use only when triaging a
        /// regex false-positive — the server runs its own scan too.
        #[arg(long)]
        allow_secret: bool,
    },
    /// Diagnose: list loaded skills, dangling symlinks, drift.
    Doctor {
        /// Emit JSON instead of a human-friendly summary.
        #[arg(long)]
        json: bool,
    },
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
    /// Install Claude Code hooks into .claude/settings.json. Installs the
    /// SessionStart hook (`skill-pool ensure --quiet`). With `--with-scorer`,
    /// also installs the Stop hook (`skill-pool capture-score`) for Phase
    /// 4.5 signal scoring. Preserves all other settings.
    HookInstall {
        /// Remove our hooks (both SessionStart and Stop) instead of installing.
        #[arg(long)]
        remove: bool,
        /// Print the merged settings.json content to stdout; don't write.
        #[arg(long)]
        print: bool,
        /// Also install the Stop hook that scores each session.
        #[arg(long)]
        with_scorer: bool,
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
            semantic,
            min_similarity,
        } => {
            cmd::search::run(
                &cfg,
                query.as_deref(),
                &tags,
                limit,
                json,
                semantic.as_deref(),
                min_similarity,
            )
            .await
        }
        Cmd::Publish {
            dir,
            slug,
            version,
            kind,
        } => cmd::publish::run(&cfg, &dir, slug.as_deref(), &version, &kind).await,
        Cmd::Capture {
            dir,
            slug,
            notes,
            tags,
            allow_secret,
        } => {
            cmd::capture::run(
                &cfg,
                &dir,
                slug.as_deref(),
                notes.as_deref(),
                &tags,
                allow_secret,
            )
            .await
        }
        Cmd::CaptureScore { from_file } => match from_file {
            Some(p) => cmd::capture_score::run_from_file(&p),
            None => cmd::capture_score::run(),
        },
        Cmd::CaptureStatus { json } => cmd::capture_status::run(json),
        Cmd::CaptureRun {
            limit,
            dry_run,
            stage1_model,
            stage2_model,
            allow_secret,
        } => {
            cmd::capture_run::run(
                &cfg,
                limit,
                dry_run,
                stage1_model.as_deref(),
                stage2_model.as_deref(),
                allow_secret,
            )
            .await
        }
        Cmd::Doctor { json } => cmd::doctor::run(&cfg, json).await,
        Cmd::Detect { json } => cmd::detect::run(json),
        Cmd::Bootstrap {
            yes,
            detect,
            dry_run,
        } => cmd::bootstrap::run(&cfg, detect, yes, dry_run).await,
        Cmd::DirenvInstall { force } => cmd::direnv_install::run(force),
        Cmd::HookInstall {
            remove,
            print,
            with_scorer,
        } => cmd::hook_install::run(remove, print, with_scorer),
    }
}
