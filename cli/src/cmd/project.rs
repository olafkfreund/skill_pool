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
}

pub async fn run(cfg: &Config, cmd: ProjectCmd) -> Result<()> {
    match cmd {
        ProjectCmd::List => list(cfg).await,
        ProjectCmd::Show { slug } => show(cfg, &slug).await,
        ProjectCmd::Link { slug } => link(cfg, &slug).await,
        ProjectCmd::Unlink => unlink(cfg).await,
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
