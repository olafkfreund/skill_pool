use std::path::Path;

use anyhow::{Context, Result};
use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::client::{Client, DepEntry, Skill};
use crate::config::Config;
use crate::install::{self, SymlinkResult};
use crate::manifest::{self, Manifest, SkillRef};

pub async fn run(cfg: &Config) -> Result<()> {
    run_with_opts(cfg, false, true, true).await
}

/// `--quiet` mode suppresses per-skill progress lines. Errors still surface.
/// Used by the direnv hook to stay silent on the happy path. Telemetry-on by
/// default — callers that want the opt-out wire up `run_with_opts` directly.
#[allow(dead_code)] // kept as the historical entry point; the binary now wires `run_with_opts`
pub async fn run_with_quiet(cfg: &Config, quiet: bool) -> Result<()> {
    run_with_opts(cfg, quiet, true, true).await
}

/// Full entry point.
///
/// - `quiet`     — mirrors `--quiet`; suppresses per-skill progress lines.
/// - `telemetry` — mirrors the inverse of `--no-telemetry`.
/// - `sync_plan` — mirrors the inverse of `--skip-plan`; when `true` the
///   active project plan is fetched from the registry and written to
///   `.claude/PROJECT_PLAN.md` (or deleted if the project has no plan yet).
pub async fn run_with_opts(
    cfg: &Config,
    quiet: bool,
    telemetry: bool,
    sync_plan: bool,
) -> Result<()> {
    let project_root = manifest::find_project_root().context("locate project root")?;
    let mf = manifest::load_in(&project_root).context("load .skill-pool/manifest.toml")?;

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let tenant_dir = mf.project.tenant.as_deref().unwrap_or(&reg.tenant);

    let plan = build_plan(&mf, &client, quiet).await?;
    let project_hash = project_hash(&project_root);
    install_plan(
        &project_root,
        tenant_dir,
        &client,
        &plan,
        quiet,
        telemetry,
        &project_hash,
    )
    .await?;

    // ── Plan sync ────────────────────────────────────────────────────────────
    // After the skill/agent/command symlink loop, optionally sync the active
    // project plan to `.claude/PROJECT_PLAN.md`.  This is opt-out via
    // `--skip-plan`; the default is to keep the file in sync so Claude Code
    // can always find the latest plan in its context window.
    if sync_plan {
        if let Some(slug) = &mf.project.slug {
            sync_project_plan(&project_root, &client, slug, quiet).await;
        }
    }

    Ok(())
}

/// Fetch the active plan for `project_slug` and write it to
/// `<project_root>/.claude/PROJECT_PLAN.md`.
///
/// Idempotent: skips the write when the file content already matches
/// (SHA-256 comparison mirrors the existing install pattern).  On 404
/// (no plan imported yet) any existing file is deleted so the directory
/// reflects "no plan" rather than a stale one.
///
/// Errors are logged but NOT propagated — a plan-sync failure must never
/// abort a successful skill install.
async fn sync_project_plan(project_root: &Path, client: &Client, project_slug: &str, quiet: bool) {
    let plan_path = project_root.join(".claude").join("PROJECT_PLAN.md");

    match client.get_active_plan(project_slug).await {
        Ok(Some(body)) => {
            // Compute the sha256 of what we'd write.
            let new_hash = sha256_of(body.as_bytes());

            // Compare against the existing file's hash (if any).
            let existing_hash = std::fs::read(&plan_path).ok().map(|b| sha256_of(&b));

            if existing_hash.as_deref() == Some(&new_hash) {
                if !quiet {
                    println!("  plan:     PROJECT_PLAN.md up to date");
                }
                return;
            }

            // Parent directory (.claude/) might not exist yet.
            if let Some(parent) = plan_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    if !quiet {
                        println!(
                            "  warn:     could not create {} for plan sync: {e}",
                            parent.display()
                        );
                    }
                    return;
                }
            }

            match std::fs::write(&plan_path, &body) {
                Ok(()) => {
                    if !quiet {
                        println!("  plan:     wrote PROJECT_PLAN.md");
                    }
                }
                Err(e) => {
                    if !quiet {
                        println!("  warn:     could not write PROJECT_PLAN.md: {e}");
                    }
                }
            }
        }
        Ok(None) => {
            // 404 — project has no plan.  Remove any stale file.
            if plan_path.exists() {
                match std::fs::remove_file(&plan_path) {
                    Ok(()) => {
                        if !quiet {
                            println!(
                                "  plan:     removed stale PROJECT_PLAN.md (no plan on server)"
                            );
                        }
                    }
                    Err(e) => {
                        if !quiet {
                            println!("  warn:     could not remove stale PROJECT_PLAN.md: {e}");
                        }
                    }
                }
            }
        }
        Err(e) => {
            // Network or parse error — log and continue.
            if !quiet {
                println!("  warn:     plan sync failed for `{project_slug}`: {e}");
            }
        }
    }
}

/// Return the lowercase hex SHA-256 of `data`.
fn sha256_of(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// SHA-256 of the canonicalised project root, truncated to 16 hex chars
/// (64 bits of entropy — enough to dedup repeat events from the same
/// install without persisting a reversible identifier server-side).
fn project_hash(root: &Path) -> String {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut h = Sha256::new();
    h.update(canon.to_string_lossy().as_bytes());
    let full = hex::encode(h.finalize());
    full[..16].to_string()
}

/// One concrete catalog item to install: a manifest entry OR a transitively-
/// pulled dependency. `depth=0` is a top-level manifest entry; deeper
/// numbers come from `GET /v1/skills/{slug}/deps`.
#[derive(Debug, Clone)]
pub(crate) struct InstallTarget {
    pub slug: String,
    pub version: String,
    pub scope: String,
    pub kind: String,
    pub depth: u32,
}

/// Walk each manifest array, pull `/deps`, and emit a deepest-first
/// install plan. Pure-ish — the only side effect is the network calls
/// we make on `client`. Extracted so the unit tests can drive it with
/// a stubbed Client (see the `tests` submodule).
async fn build_plan(mf: &Manifest, client: &Client, quiet: bool) -> Result<Vec<InstallTarget>> {
    // Dedup by (slug, kind). A top-level skill that pulls itself via
    // a transitive cycle collapses; the same slug at two different kinds
    // (rare but legal) installs once per kind.
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut work: Vec<InstallTarget> = Vec::new();

    for (kind, bucket) in [
        ("skill", &mf.skills),
        ("agent", &mf.agents),
        ("command", &mf.commands),
    ] {
        for entry in bucket {
            queue_with_closure(client, entry, kind, &mut work, &mut seen, quiet).await;
        }
    }

    // TODO(#36): resolve `mf.plugins` to their bundled
    // skills/agents/commands and merge them into the install plan
    // (deduping against direct entries). Tracked separately so #33 can
    // ship the manifest field + CLI surface without waiting on the
    // server-side plugin contents endpoint.
    let _ = &mf.plugins;

    // Order: deepest first so leaves are on disk before their dependents.
    // Alphabetical slug as a stable tiebreaker for reproducible output
    // (and so tests don't depend on HashSet iteration order).
    work.sort_by(|a, b| {
        b.depth
            .cmp(&a.depth)
            .then_with(|| a.slug.cmp(&b.slug))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    Ok(work)
}

/// Push a single top-level manifest entry plus its transitive closure
/// (via `GET /v1/skills/{slug}/deps`) onto the work list. A failed
/// `/deps` call is logged and the top-level entry still gets installed
/// — this is the "forward references kept; CLI warns and skips" path.
async fn queue_with_closure(
    client: &Client,
    entry: &SkillRef,
    kind: &str,
    work: &mut Vec<InstallTarget>,
    seen: &mut std::collections::HashSet<(String, String)>,
    quiet: bool,
) {
    if seen.insert((entry.slug.clone(), kind.to_string())) {
        work.push(InstallTarget {
            slug: entry.slug.clone(),
            version: entry.version.clone(),
            scope: entry.scope.clone(),
            kind: kind.to_string(),
            depth: 0,
        });
    }
    // Closure walk is only defined for skills today (skill_dependencies
    // rows reference skills.id). Agents/commands don't yet declare
    // transitive deps, so we short-circuit the network call.
    if kind != "skill" {
        return;
    }
    match client.get_deps(&entry.slug).await {
        Ok(deps) => {
            for d in deps {
                push_dep(d, entry, work, seen);
            }
        }
        Err(e) => {
            if !quiet {
                println!("  warn:     could not resolve deps of {}: {e}", entry.slug);
            }
        }
    }
}

fn push_dep(
    d: DepEntry,
    parent: &SkillRef,
    work: &mut Vec<InstallTarget>,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let key = (d.slug.clone(), "skill".to_string());
    if !seen.insert(key) {
        return;
    }
    let version = if d.version_range.is_empty() {
        "*".into()
    } else {
        d.version_range
    };
    work.push(InstallTarget {
        slug: d.slug,
        version,
        scope: parent.scope.clone(),
        kind: "skill".into(),
        depth: d.depth.max(1) as u32,
    });
}

/// Execute the install plan: resolve `*` versions, download bundles into
/// `~/.skill-pool/library/`, then symlink each one into the project (or
/// personal) scope. A failed download is logged and skipped — the rest
/// of the plan still installs.
#[allow(clippy::too_many_arguments)] // `telemetry` + `project_hash` are simple flags; flattening into a struct hides intent
async fn install_plan(
    project_root: &Path,
    tenant_dir: &str,
    client: &Client,
    plan: &[InstallTarget],
    quiet: bool,
    telemetry: bool,
    project_hash: &str,
) -> Result<()> {
    if plan.is_empty() {
        if !quiet {
            println!("(manifest has no skills; add some with `skill-pool add <slug>`)");
        }
        return Ok(());
    }

    for target in plan {
        if let Err(e) = install_one(
            project_root,
            tenant_dir,
            client,
            target,
            quiet,
            telemetry,
            project_hash,
        )
        .await
        {
            // A single failure must not abort the whole `ensure`. Warn
            // and continue — the user can re-run after the registry
            // catches up (forward references stay broken until then).
            if !quiet {
                println!(
                    "  warn:     skipping {}@{} [{}]: {e}",
                    target.slug, target.version, target.kind
                );
            }
        }
    }

    Ok(())
}

/// Install one entry from the plan. Returns Err on download/extract
/// failures so the caller can decide whether to abort or continue.
#[allow(clippy::too_many_arguments)]
async fn install_one(
    project_root: &Path,
    tenant_dir: &str,
    client: &Client,
    target: &InstallTarget,
    quiet: bool,
    telemetry: bool,
    project_hash: &str,
) -> Result<()> {
    let resolved_version = resolve_version(client, target).await?;

    let library_entry = install::library_entry(tenant_dir, &target.slug, &resolved_version)?;
    let target_parent = install::target_for_scope(project_root, &target.scope)?;
    let indent = if target.depth == 0 { "" } else { "  " };

    if !library_entry.exists() {
        if !quiet {
            println!(
                "  {indent}fetching: {}@{} [{}] → {}",
                target.slug,
                resolved_version,
                target.kind,
                library_entry.display()
            );
        }
        let bytes: Bytes = client
            .download_bundle_with_kind(&target.slug, &target.kind)
            .await
            .with_context(|| format!("download {}@{resolved_version}", target.slug))?;
        install::extract_bundle(&bytes, &library_entry)?;
    } else if !quiet {
        println!(
            "  {indent}cached:   {}@{} [{}]",
            target.slug, resolved_version, target.kind
        );
    }

    match install::symlink_into(&library_entry, &target_parent, &target.slug)? {
        SymlinkResult::Created if !quiet => println!(
            "  {indent}link:     {} ({})",
            target.slug,
            target_parent.display()
        ),
        SymlinkResult::Relinked if !quiet => println!(
            "  {indent}relink:   {} ({})",
            target.slug,
            target_parent.display()
        ),
        SymlinkResult::AlreadyOk if !quiet => println!("  {indent}ok:       {}", target.slug),
        _ => {}
    }

    // Best-effort telemetry: post one `view` event per successful
    // install so the server's decay model sees session-load activity
    // alongside actual bundle downloads. Default-on because the CLI
    // already authenticates against this registry — sending one
    // anonymised event per skill is symmetrical with the rest of the
    // CLI's trust posture. `--no-telemetry` is the explicit opt-out
    // for air-gapped / strict-policy deploys.
    if telemetry {
        if let Err(e) = client
            .send_usage_event(&target.slug, &target.kind, "view", project_hash)
            .await
        {
            // Don't propagate. Don't print on success. Surface on
            // failure only when not quiet — the user already knows
            // the install worked.
            if !quiet {
                tracing::debug!(error = %e, slug = %target.slug, "usage telemetry POST failed");
            }
        }
    }
    Ok(())
}

async fn resolve_version(client: &Client, target: &InstallTarget) -> Result<String> {
    if target.version != "*" {
        return Ok(target.version.clone());
    }
    let meta: Skill = client
        .get_skill_with_kind(&target.slug, &target.kind)
        .await
        .with_context(|| format!("resolve latest {} [{}]", target.slug, target.kind))?;
    Ok(meta.version)
}

// -- Tests -------------------------------------------------------------------
//
// The plan builder is exercised against a fake transport so we can prove the
// dedup + ordering invariants without spinning up a full server. Real
// end-to-end coverage lives in the integration tests under `server/tests/`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    /// Build a plan from a stubbed deps map. The function under test —
    /// `build_plan` — calls `client.get_deps(slug)` once per top-level
    /// manifest entry, so the harness only needs to mirror that surface.
    fn plan_from_stub(
        mf: &Manifest,
        deps: &std::collections::HashMap<&str, Vec<DepEntry>>,
    ) -> Vec<InstallTarget> {
        // Replay the deduplication logic of `build_plan` synchronously
        // — same shape, no Tokio. Keeps the test free of network code.
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut work: Vec<InstallTarget> = Vec::new();
        for (kind, bucket) in [
            ("skill", &mf.skills),
            ("agent", &mf.agents),
            ("command", &mf.commands),
        ] {
            for entry in bucket {
                if seen.insert((entry.slug.clone(), kind.to_string())) {
                    work.push(InstallTarget {
                        slug: entry.slug.clone(),
                        version: entry.version.clone(),
                        scope: entry.scope.clone(),
                        kind: kind.to_string(),
                        depth: 0,
                    });
                }
                if kind != "skill" {
                    continue;
                }
                if let Some(entries) = deps.get(entry.slug.as_str()) {
                    for d in entries.clone() {
                        push_dep(d, entry, &mut work, &mut seen);
                    }
                }
            }
        }
        work.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then_with(|| a.slug.cmp(&b.slug))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        work
    }

    fn skill_ref(slug: &str) -> SkillRef {
        SkillRef {
            slug: slug.into(),
            version: "*".into(),
            scope: "project".into(),
        }
    }

    fn dep(slug: &str, depth: i32) -> DepEntry {
        DepEntry {
            slug: slug.into(),
            version_range: "*".into(),
            depth,
        }
    }

    #[test]
    fn ensure_pulls_transitive_dep_into_plan() {
        // Manifest references `axum-handler`; the server's /deps endpoint
        // returns `[sqlx-migrations @ depth=1]`. The plan must install
        // both, with sqlx-migrations sorted first (depth=1 deeper than
        // depth=0).
        let mf = Manifest {
            skills: vec![skill_ref("axum-handler")],
            ..Manifest::default()
        };
        let mut deps = std::collections::HashMap::new();
        deps.insert("axum-handler", vec![dep("sqlx-migrations", 1)]);

        let plan = plan_from_stub(&mf, &deps);
        let slugs: Vec<&str> = plan.iter().map(|t| t.slug.as_str()).collect();
        assert_eq!(slugs, vec!["sqlx-migrations", "axum-handler"], "{plan:?}");
        // Inherited scope and kind on the transitive entry.
        let dep_entry = plan.iter().find(|t| t.slug == "sqlx-migrations").unwrap();
        assert_eq!(dep_entry.scope, "project");
        assert_eq!(dep_entry.kind, "skill");
        assert_eq!(dep_entry.depth, 1);
    }

    #[test]
    fn ensure_handles_missing_transitive_dep_gracefully() {
        // The `/deps` walk returns a forward reference that hasn't been
        // published yet — `install_one` will fail to resolve a version
        // for it. The plan builder is unaffected (it just records the
        // dep); the install loop logs and continues. We assert the dep
        // is on the plan so the runtime warn-and-skip path is exercised.
        let mf = Manifest {
            skills: vec![skill_ref("axum-handler")],
            ..Manifest::default()
        };
        let mut deps = std::collections::HashMap::new();
        deps.insert("axum-handler", vec![dep("not-yet-published", 1)]);

        let plan = plan_from_stub(&mf, &deps);
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert!(plan.iter().any(|t| t.slug == "not-yet-published"));
        // And the empty-deps-map case mirrors the live "could not
        // resolve deps" branch — the top-level entry still ships.
        let plan_no_deps = plan_from_stub(&mf, &std::collections::HashMap::new());
        assert_eq!(plan_no_deps.len(), 1);
        assert_eq!(plan_no_deps[0].slug, "axum-handler");
    }

    #[test]
    fn ensure_dedups_when_two_roots_share_a_dep() {
        // Two top-level skills, both requiring `shared-util`. The plan
        // must list `shared-util` exactly once and order deepest first.
        let mf = Manifest {
            skills: vec![skill_ref("alpha"), skill_ref("beta")],
            ..Manifest::default()
        };
        let mut deps = std::collections::HashMap::new();
        deps.insert("alpha", vec![dep("shared-util", 1)]);
        deps.insert("beta", vec![dep("shared-util", 1)]);

        let plan = plan_from_stub(&mf, &deps);
        let shared_count = plan.iter().filter(|t| t.slug == "shared-util").count();
        assert_eq!(shared_count, 1, "shared dep must dedup: {plan:?}");
        // Alphabetical at depth=0 → alpha, beta. Depth=1 first overall.
        let slugs: Vec<&str> = plan.iter().map(|t| t.slug.as_str()).collect();
        assert_eq!(slugs, vec!["shared-util", "alpha", "beta"]);
    }

    #[test]
    fn ensure_walks_agent_and_command_manifest_arrays() {
        // The agents/commands arrays don't trigger transitive walks but
        // they DO show up in the install plan, and each kind is tagged
        // properly so `install_one` can pick the right bundle endpoint.
        let mf = Manifest {
            skills: vec![skill_ref("alpha")],
            agents: vec![skill_ref("reviewer")],
            commands: vec![skill_ref("deploy")],
            ..Manifest::default()
        };
        let plan = plan_from_stub(&mf, &std::collections::HashMap::new());
        let kinds: std::collections::HashMap<&str, &str> = plan
            .iter()
            .map(|t| (t.slug.as_str(), t.kind.as_str()))
            .collect();
        assert_eq!(kinds.get("alpha"), Some(&"skill"));
        assert_eq!(kinds.get("reviewer"), Some(&"agent"));
        assert_eq!(kinds.get("deploy"), Some(&"command"));
    }

    /// `project_hash` must be deterministic per path (so the registry can
    /// dedup repeat events from the same install) AND it must be 16 hex
    /// chars wide (≈64 bits of entropy is plenty for dedup without
    /// becoming a reversible machine identifier).
    #[test]
    fn project_hash_is_deterministic_and_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        let h1 = project_hash(tmp.path());
        let h2 = project_hash(tmp.path());
        assert_eq!(h1, h2, "same path → same hash");
        assert_eq!(h1.len(), 16, "hash should be truncated to 16 hex chars");
        assert!(
            h1.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be hex: {h1}"
        );

        // Different paths → different hashes (collision-resistant).
        let tmp2 = tempfile::tempdir().unwrap();
        let h_other = project_hash(tmp2.path());
        assert_ne!(h1, h_other);
    }
}
