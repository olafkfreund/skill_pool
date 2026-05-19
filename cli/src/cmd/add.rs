use anyhow::{Context, Result};

use crate::client::Client;
use crate::cmd::ensure;
use crate::config::Config;
use crate::manifest;

/// Default entry point: add a skill (kind=`skill`) to the manifest and
/// install everything reachable through ensure. Kept as a thin shim so
/// the call site in `main.rs` (`Cmd::Add`) stays unchanged.
pub async fn run(cfg: &Config, slug: &str) -> Result<()> {
    run_with_kind(cfg, slug, "skill").await
}

/// Kind-aware add: validates the slug exists on the registry, appends
/// it to the right manifest array (`skills`, `agents`, or `commands`),
/// then runs `ensure` to install everything.
///
/// Callers in `main.rs` are the three subcommands `Add`, `AddAgent`,
/// `AddCommand`, which pre-translate to one of the three supported
/// kind strings. Any other value is rejected by `add_to_manifest`.
pub async fn run_with_kind(cfg: &Config, slug: &str, kind: &str) -> Result<()> {
    let project_root = manifest::find_project_root().context("locate project root")?;
    let mut mf = manifest::load_in(&project_root).unwrap_or_default();

    // Validate the catalog item exists on the registry before mutating
    // local files. The kind defaults to `skill` on the server too, so
    // we forward whichever kind the user asked for.
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let meta = client
        .get_skill_with_kind(slug, kind)
        .await
        .with_context(|| {
            format!(
                "verify `{slug}` ({kind}) exists on {} for tenant {}",
                reg.url, reg.tenant
            )
        })?;

    let inserted = manifest::add_to_manifest(&mut mf, slug, kind)?;
    if inserted {
        manifest::save_in(&project_root, &mf)?;
        println!("added: {slug}@{} [{kind}] (manifest updated)", meta.version);
    } else {
        println!("(already in manifest: {slug}@{} [{kind}])", meta.version);
    }

    ensure::run(cfg).await
}
