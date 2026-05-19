//! `skill-pool doctor` — local diagnostics.
//!
//! Answers the developer's questions in priority order:
//!   1. "Is my registry config sane?"
//!   2. "Do the skills in my manifest actually resolve to a working symlink?"
//!   3. "Are there symlinks in `.claude/skills/` that aren't in my manifest?"
//!      (orphans — likely from manual `ln -s`, or a manifest entry that was
//!      removed without `skill-pool` cleaning up)
//!   4. "Anything in the catalog headed for the graveyard?"
//!      (decay candidates — admin-scope only; soft-skipped without auth)

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use crate::client::Client;
use crate::config::Config;
use crate::install;
use crate::manifest;

#[derive(Serialize)]
struct DoctorReport {
    registry: RegistryReport,
    manifest: ManifestReport,
    orphans: Vec<Orphan>,
    decay: DecayReport,
}

#[derive(Serialize)]
struct RegistryReport {
    configured: bool,
    url: Option<String>,
    tenant: Option<String>,
    token_set: bool,
}

#[derive(Serialize)]
struct ManifestReport {
    /// None when no `.skill-pool/manifest.toml` was found from the cwd.
    path: Option<PathBuf>,
    project_root: Option<PathBuf>,
    entries: Vec<EntryReport>,
}

#[derive(Serialize)]
struct EntryReport {
    kind: &'static str,
    slug: String,
    version: String,
    scope: String,
    /// `ok` — symlink resolves to a populated library entry.
    /// `missing` — no symlink at the expected target.
    /// `dangling` — symlink exists but points at a non-existent path.
    /// `wrong-target` — symlink exists and points somewhere else than the library entry.
    /// `not-a-symlink` — a non-symlink path occupies the slot.
    /// `library-empty` — symlink ok but the library entry has no SKILL.md.
    status: &'static str,
    target: PathBuf,
    library_entry: Option<PathBuf>,
    detail: Option<String>,
}

#[derive(Serialize)]
struct Orphan {
    scope: &'static str,
    path: PathBuf,
    target: Option<PathBuf>,
}

#[derive(Serialize)]
struct DecayReport {
    /// Why decay was skipped, if it was.
    skipped: Option<String>,
    candidates: Vec<DecayMatch>,
}

#[derive(Serialize)]
struct DecayMatch {
    slug: String,
    version: String,
    use_count: i32,
    last_used_at: Option<String>,
    installed_locally: bool,
}

pub async fn run(cfg: &Config, json_out: bool) -> Result<()> {
    let registry = registry_report(cfg);
    let manifest = manifest_report(cfg)?;
    let orphans = orphan_report(&manifest);
    let decay = decay_report(cfg, &manifest).await;

    let report = DoctorReport {
        registry,
        manifest,
        orphans,
        decay,
    };

    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

fn registry_report(cfg: &Config) -> RegistryReport {
    match &cfg.registry {
        Some(r) => RegistryReport {
            configured: true,
            url: Some(r.url.clone()),
            tenant: Some(r.tenant.clone()),
            token_set: r.token.is_some(),
        },
        None => RegistryReport {
            configured: false,
            url: None,
            tenant: None,
            token_set: false,
        },
    }
}

fn manifest_report(cfg: &Config) -> Result<ManifestReport> {
    let project_root = match manifest::find_project_root() {
        Ok(p) if p.join(manifest::MANIFEST_REL).exists() => p,
        _ => {
            return Ok(ManifestReport {
                path: None,
                project_root: None,
                entries: vec![],
            });
        }
    };
    let path = manifest::manifest_path_in(&project_root);
    let manifest_obj = manifest::load_in(&project_root)?;
    let tenant = manifest_obj
        .project
        .tenant
        .clone()
        .or_else(|| cfg.registry.as_ref().map(|r| r.tenant.clone()))
        .unwrap_or_else(|| "default".into());

    let mut entries = Vec::new();
    for s in &manifest_obj.skills {
        entries.push(check_entry(&project_root, &tenant, "skill", s));
    }
    for a in &manifest_obj.agents {
        entries.push(check_entry(&project_root, &tenant, "agent", a));
    }
    for c in &manifest_obj.commands {
        entries.push(check_entry(&project_root, &tenant, "command", c));
    }

    Ok(ManifestReport {
        path: Some(path),
        project_root: Some(project_root),
        entries,
    })
}

fn check_entry(
    project_root: &std::path::Path,
    tenant: &str,
    kind: &'static str,
    s: &manifest::SkillRef,
) -> EntryReport {
    let lib = install::library_entry(tenant, &s.slug, &s.version).ok();
    let target_parent = match install::target_for_scope(project_root, &s.scope) {
        Ok(p) => p,
        Err(e) => {
            return EntryReport {
                kind,
                slug: s.slug.clone(),
                version: s.version.clone(),
                scope: s.scope.clone(),
                status: "missing",
                target: PathBuf::new(),
                library_entry: lib,
                detail: Some(e.to_string()),
            };
        }
    };
    let target = target_parent.join(&s.slug);

    let md = std::fs::symlink_metadata(&target).ok();
    let lib_path = lib.clone();

    match md {
        None => EntryReport {
            kind,
            slug: s.slug.clone(),
            version: s.version.clone(),
            scope: s.scope.clone(),
            status: "missing",
            target,
            library_entry: lib_path,
            detail: Some(
                "no symlink at expected target — run `skill-pool ensure` to install".into(),
            ),
        },
        Some(md) if md.file_type().is_symlink() => {
            let link = std::fs::read_link(&target).ok();
            // Dangling? Symlink exists but its destination doesn't.
            if !target.exists() {
                return EntryReport {
                    kind,
                    slug: s.slug.clone(),
                    version: s.version.clone(),
                    scope: s.scope.clone(),
                    status: "dangling",
                    target,
                    library_entry: lib_path,
                    detail: link.map(|p| format!("points at {} (does not exist)", p.display())),
                };
            }
            // Pointing at the expected library entry?
            let canon_target = target.canonicalize().ok();
            let canon_lib = lib_path.as_ref().and_then(|p| p.canonicalize().ok());
            if canon_target.is_some() && canon_target == canon_lib {
                // Sanity check: library entry actually has SKILL.md.
                if let Some(p) = canon_lib.as_ref() {
                    if !p.join("SKILL.md").exists() {
                        return EntryReport {
                            kind,
                            slug: s.slug.clone(),
                            version: s.version.clone(),
                            scope: s.scope.clone(),
                            status: "library-empty",
                            target,
                            library_entry: lib_path,
                            detail: Some("library entry has no SKILL.md".into()),
                        };
                    }
                }
                EntryReport {
                    kind,
                    slug: s.slug.clone(),
                    version: s.version.clone(),
                    scope: s.scope.clone(),
                    status: "ok",
                    target,
                    library_entry: lib_path,
                    detail: None,
                }
            } else {
                EntryReport {
                    kind,
                    slug: s.slug.clone(),
                    version: s.version.clone(),
                    scope: s.scope.clone(),
                    status: "wrong-target",
                    target,
                    library_entry: lib_path,
                    detail: link.map(|p| format!("points at {}", p.display())),
                }
            }
        }
        Some(_) => EntryReport {
            kind,
            slug: s.slug.clone(),
            version: s.version.clone(),
            scope: s.scope.clone(),
            status: "not-a-symlink",
            target,
            library_entry: lib_path,
            detail: Some(
                "non-symlink file/dir occupies the slot — remove it or rename, then re-ensure"
                    .into(),
            ),
        },
    }
}

fn orphan_report(m: &ManifestReport) -> Vec<Orphan> {
    let mut known: HashSet<PathBuf> = HashSet::new();
    for e in &m.entries {
        known.insert(e.target.clone());
    }

    let mut out = Vec::new();
    let Some(root) = &m.project_root else {
        return out;
    };

    for (scope_label, parent) in [
        ("project", root.join(".claude").join("skills")),
        ("personal", personal_skills_dir()),
    ] {
        if !parent.is_dir() {
            continue;
        }
        let read = match std::fs::read_dir(&parent) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let path = entry.path();
            let md = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !md.file_type().is_symlink() {
                continue;
            }
            if known.contains(&path) {
                continue;
            }
            let target = std::fs::read_link(&path).ok();
            out.push(Orphan {
                scope: scope_label,
                path,
                target,
            });
        }
    }
    out
}

fn personal_skills_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_default()
        .join(".claude")
        .join("skills")
}

async fn decay_report(cfg: &Config, m: &ManifestReport) -> DecayReport {
    let Some(reg) = cfg.registry.as_ref() else {
        return DecayReport {
            skipped: Some("registry not configured".into()),
            candidates: vec![],
        };
    };
    if reg.token.is_none() {
        return DecayReport {
            skipped: Some("no token configured — run `skill-pool login`".into()),
            candidates: vec![],
        };
    }
    let client = match Client::new(reg) {
        Ok(c) => c,
        Err(e) => {
            return DecayReport {
                skipped: Some(format!("client init failed: {e}")),
                candidates: vec![],
            };
        }
    };
    let raw = match client.decay_candidates().await {
        Ok(v) => v,
        Err(e) => {
            return DecayReport {
                skipped: Some(format!("server returned error: {e}")),
                candidates: vec![],
            };
        }
    };

    let installed: HashSet<&str> = m.entries.iter().map(|e| e.slug.as_str()).collect();
    let candidates = raw
        .into_iter()
        .map(|c| DecayMatch {
            installed_locally: installed.contains(c.slug.as_str()),
            slug: c.slug,
            version: c.version,
            use_count: c.use_count,
            last_used_at: c.last_used_at,
        })
        .collect();
    DecayReport {
        skipped: None,
        candidates,
    }
}

fn print_human(r: &DoctorReport) {
    println!("skill-pool doctor — v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Registry");
    if r.registry.configured {
        println!("  url:    {}", r.registry.url.as_deref().unwrap_or("?"));
        println!("  tenant: {}", r.registry.tenant.as_deref().unwrap_or("?"));
        println!(
            "  token:  {}",
            if r.registry.token_set {
                "set"
            } else {
                "MISSING"
            }
        );
    } else {
        println!("  (not configured — run `skill-pool login`)");
    }

    println!();
    println!("Manifest");
    match &r.manifest.path {
        None => println!("  (no .skill-pool/manifest.toml found from cwd)"),
        Some(p) => {
            println!("  path: {}", p.display());
            if r.manifest.entries.is_empty() {
                println!("  (no entries)");
            }
            let mut ok = 0;
            let mut bad = 0;
            for e in &r.manifest.entries {
                let mark = match e.status {
                    "ok" => {
                        ok += 1;
                        "✓"
                    }
                    _ => {
                        bad += 1;
                        "✗"
                    }
                };
                println!(
                    "  {mark} [{kind}] {slug}@{version}  ({scope})  {status}",
                    kind = e.kind,
                    slug = e.slug,
                    version = e.version,
                    scope = e.scope,
                    status = e.status,
                );
                if let Some(d) = &e.detail {
                    println!("      {d}");
                }
            }
            println!("  → {ok} ok, {bad} broken");
        }
    }

    println!();
    println!("Orphan symlinks");
    if r.orphans.is_empty() {
        println!("  (none)");
    } else {
        for o in &r.orphans {
            print!("  ⚠ [{scope}] {}", o.path.display(), scope = o.scope);
            if let Some(t) = &o.target {
                print!("  → {}", t.display());
            }
            println!();
        }
    }

    println!();
    println!("Decay candidates");
    if let Some(reason) = &r.decay.skipped {
        println!("  (skipped: {reason})");
    } else if r.decay.candidates.is_empty() {
        println!("  (none)");
    } else {
        for c in &r.decay.candidates {
            let last = c
                .last_used_at
                .as_deref()
                .unwrap_or("never");
            println!(
                "  • {slug}@{version}  used {n}×  last_used={last}{installed}",
                slug = c.slug,
                version = c.version,
                n = c.use_count,
                last = last,
                installed = if c.installed_locally {
                    "  [installed locally]"
                } else {
                    ""
                },
            );
        }
    }
}
