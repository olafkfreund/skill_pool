//! `skill-pool project` subcommand — list, inspect, link, and unlink
//! curator-defined projects from the current workspace manifest.

use anyhow::{Context, Result};

use crate::client::Client;
use crate::config::Config;
use crate::manifest::{find_project_root, load_in, save_in};

#[derive(Debug, clap::Subcommand)]
pub enum ProjectCmd {
    /// List all projects registered for the tenant.
    List,
    /// Show metadata and items for one project.
    Show {
        /// The project slug (e.g. `acme-billing-service`).
        slug: String,
    },
    /// Pin this workspace to a project: writes `manifest.project.slug`.
    Link {
        /// The project slug to link.
        slug: String,
    },
    /// Remove the project pin from this workspace's manifest.
    Unlink,
    /// Curator-side: append a plugin to an existing project's item list
    /// (#36). The plugin slug must be published in the same tenant.
    ///
    /// Existing items are preserved; the plugin is appended at the end
    /// of the list, so it takes the lowest precedence among the project's
    /// items but stays ahead of stack-tier matches.
    AddPlugin {
        /// The plugin slug to add (e.g. `acme-base-bundle`).
        slug: String,
        /// Override the active workspace project pin. When omitted,
        /// the slug is read from `manifest.project.slug`.
        #[arg(long)]
        project: Option<String>,
    },
}

pub async fn run(cfg: &Config, cmd: ProjectCmd) -> Result<()> {
    match cmd {
        ProjectCmd::List => list(cfg).await,
        ProjectCmd::Show { slug } => show(cfg, &slug).await,
        ProjectCmd::Link { slug } => link(cfg, &slug).await,
        ProjectCmd::Unlink => unlink(cfg).await,
        ProjectCmd::AddPlugin { slug, project } => {
            add_plugin(cfg, &slug, project.as_deref()).await
        }
    }
}

// ── list ─────────────────────────────────────────────────────────────────────

async fn list(cfg: &Config) -> Result<()> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let projects = client.list_projects().await?;

    if projects.is_empty() {
        println!("(no projects registered for tenant `{}`)", reg.tenant);
        println!(
            "  create one: POST /v1/tenant/projects  {{\"slug\": \"...\", \"name\": \"...\"}}"
        );
        return Ok(());
    }

    // Table header
    println!("{:<30}  {:<30}  {:>6}  GIT REMOTE", "SLUG", "NAME", "ITEMS");
    println!("{}", "-".repeat(90));

    for p in &projects {
        let remote = p.git_remote.as_deref().unwrap_or("—");
        println!(
            "{:<30}  {:<30}  {:>6}  {}",
            p.slug, p.name, p.item_count, remote
        );
    }

    Ok(())
}

// ── show ─────────────────────────────────────────────────────────────────────

async fn show(cfg: &Config, slug: &str) -> Result<()> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let p = client.get_project(slug).await?;

    println!("Project: {} ({})", p.name, p.slug);
    if let Some(desc) = &p.description {
        println!("Description: {desc}");
    }
    if let Some(remote) = &p.git_remote {
        println!("Git remote:  {remote}");
    }
    if !p.stack_tags.is_empty() {
        println!("Stack tags:  {}", p.stack_tags.join(", "));
    }

    if p.items.is_empty() {
        println!("\n(no items curated yet)");
        return Ok(());
    }

    // Group by kind for readability.
    let mut skills: Vec<&str> = vec![];
    let mut agents: Vec<&str> = vec![];
    let mut commands: Vec<&str> = vec![];

    for item in &p.items {
        match item.kind.as_str() {
            "skill" => skills.push(&item.skill_slug),
            "agent" => agents.push(&item.skill_slug),
            "command" => commands.push(&item.skill_slug),
            _ => skills.push(&item.skill_slug),
        }
    }

    println!();
    if !skills.is_empty() {
        println!("Skills ({}):", skills.len());
        for s in &skills {
            println!("  {s}");
        }
    }
    if !agents.is_empty() {
        println!("Agents ({}):", agents.len());
        for a in &agents {
            println!("  {a}");
        }
    }
    if !commands.is_empty() {
        println!("Commands ({}):", commands.len());
        for c in &commands {
            println!("  {c}");
        }
    }

    Ok(())
}

// ── link ─────────────────────────────────────────────────────────────────────

async fn link(cfg: &Config, slug: &str) -> Result<()> {
    let project_root = find_project_root().context("locate project root")?;
    let mut mf = load_in(&project_root).unwrap_or_default();

    // Optionally validate that the slug exists server-side before writing.
    // We do a best-effort check: if no registry is configured, skip.
    if let Ok(reg) = cfg.require_registry() {
        let client = Client::new(reg)?;
        // get_project returns an error on 404 — propagate to the user.
        let p = client
            .get_project(slug)
            .await
            .with_context(|| format!("project `{slug}` not found on registry"))?;
        println!("Linking to project: {} ({})", p.name, p.slug);
    }

    mf.project.slug = Some(slug.to_string());
    save_in(&project_root, &mf)?;
    println!(
        "wrote manifest.project.slug = {:?}",
        mf.project.slug.as_deref().unwrap_or("")
    );
    println!("run `skill-pool bootstrap` to install the project's curated skills.");
    Ok(())
}

// ── unlink ───────────────────────────────────────────────────────────────────

async fn unlink(_cfg: &Config) -> Result<()> {
    let project_root = find_project_root().context("locate project root")?;
    let mut mf = load_in(&project_root).unwrap_or_default();

    if mf.project.slug.is_none() {
        println!("(manifest already has no project.slug; nothing to do)");
        return Ok(());
    }

    let old = mf.project.slug.take();
    save_in(&project_root, &mf)?;
    println!(
        "cleared manifest.project.slug (was {:?})",
        old.as_deref().unwrap_or("")
    );
    Ok(())
}

// ── add-plugin ───────────────────────────────────────────────────────────────

/// Append a plugin to a project's curated item list (server-side curation).
///
/// The implementation reads the existing items via `GET /v1/tenant/projects/{slug}`,
/// appends `(plugin_slug, "plugin")`, then atomically replaces via
/// `PUT /v1/tenant/projects/{slug}/items`. This is the cleanest path
/// today because the server-side API only exposes the full-replace
/// `PUT` endpoint; a future append-one route would let us skip the
/// roundtrip but is out of scope for #36.
///
/// We also verify the plugin slug exists in the tenant catalog (via
/// `GET /v1/plugins/{slug}`) before mutating the project so the user
/// gets an immediate, specific "no such plugin" error instead of
/// discovering it later when `ensure` falls over on a stale pin.
async fn add_plugin(cfg: &Config, plugin_slug: &str, project_override: Option<&str>) -> Result<()> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    // Resolve the target project slug: explicit `--project` override
    // takes priority over the workspace manifest's pin.
    let project_slug = match project_override {
        Some(s) => s.to_string(),
        None => {
            let project_root = find_project_root().context("locate project root")?;
            let mf = load_in(&project_root).unwrap_or_default();
            mf.project.slug.ok_or_else(|| {
                anyhow::anyhow!(
                    "no project pinned in manifest; pass --project <slug> or run `skill-pool project link <slug>` first"
                )
            })?
        }
    };

    // Verify the plugin exists in the tenant catalog so we fail fast
    // with a precise error message.
    use crate::client::PluginEndpointOutcome;
    match client.get_plugin(plugin_slug).await? {
        PluginEndpointOutcome::Ok(_) => {}
        PluginEndpointOutcome::Unavailable { issue } => {
            anyhow::bail!(
                "plugin `{plugin_slug}` not found on registry \
                 (publish it first with `skill-pool plugin publish <dir>`; tracking: issue #{issue})"
            );
        }
    }

    let existing = client
        .get_project(&project_slug)
        .await
        .with_context(|| format!("fetch project `{project_slug}` from registry"))?;

    // Idempotency: if the plugin is already pinned, surface the no-op
    // rather than re-PUTing the same list.
    if existing
        .items
        .iter()
        .any(|i| i.kind == "plugin" && i.skill_slug == plugin_slug)
    {
        println!(
            "(plugin `{plugin_slug}` is already pinned in project `{project_slug}`; nothing to do)"
        );
        return Ok(());
    }

    // Preserve existing items in their curator-defined order, then
    // append the new plugin.
    let mut items: Vec<(String, String)> = existing
        .items
        .iter()
        .map(|i| (i.skill_slug.clone(), i.kind.clone()))
        .collect();
    items.push((plugin_slug.to_string(), "plugin".into()));

    client
        .set_project_items(&project_slug, &items)
        .await
        .with_context(|| {
            format!("update items for project `{project_slug}` on registry")
        })?;

    println!(
        "added plugin `{plugin_slug}` to project `{project_slug}` ({} items total)",
        items.len()
    );
    println!("  run `skill-pool ensure` in a workspace pinned to `{project_slug}` to install the bundled contents.");
    Ok(())
}
