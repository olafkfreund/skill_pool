//! Stack detection from project files.
//!
//! Phase 3 ships the **fingerprint** tier — fast (<100ms typical),
//! deterministic, no network. Catches ~90% of common stacks. Tiers 2
//! (deep manifest parsing) and 3 (LLM fallback) layer on later.
//!
//! Output is a deduped sorted list of lower-case tag strings that the
//! server's `/v1/bootstrap` endpoint matches against `tenant_stack_mappings`.
//!
//! ## Cache layer
//!
//! Detection is cheap (~100ms), but on a hot path — `direnv` triggers
//! `skill-pool ensure` which calls back into bootstrap on a fresh dir.
//! To stay snappy on repeated invocations within a session we persist
//! the last detection result to `<project_root>/.skill-pool/detected.json`
//! along with the mtime of every source file we read. On the next call:
//!
//! - if every recorded mtime is unchanged AND no new source file appeared,
//!   we return the cached `stack` without re-reading any file;
//! - otherwise we run fresh detection and rewrite the cache atomically.
//!
//! Callers should use `detect_cached(root)` by default; `detect(root)`
//! is the cache-miss path and stays public for tests and `--no-cache`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Filename inside `.skill-pool/` where the detection cache lives.
pub const CACHE_REL: &str = ".skill-pool/detected.json";

#[derive(Debug, Serialize)]
pub struct Detection {
    pub stack: Vec<String>,
    /// Files that contributed to detection — useful for `--verbose` output.
    pub signals: Vec<String>,
}

/// Single-file fingerprint rules — relative path → tags to insert when present.
const FILE_RULES: &[(&str, &[&str])] = &[
    ("flake.nix", &["nix"]),
    ("Cargo.toml", &["rust"]),
    ("go.mod", &["go"]),
    ("Gemfile", &["ruby"]),
    ("pyproject.toml", &["python"]),
    ("requirements.txt", &["python"]),
    ("Pipfile", &["python"]),
    ("composer.json", &["php"]),
    ("pom.xml", &["java", "maven"]),
    ("build.gradle", &["java", "gradle"]),
    ("build.gradle.kts", &["kotlin", "gradle"]),
    ("Package.swift", &["swift"]),
    ("mix.exs", &["elixir"]),
    ("docker-compose.yml", &["docker", "compose"]),
    ("docker-compose.yaml", &["docker", "compose"]),
    ("Dockerfile", &["docker"]),
    ("Makefile", &["make"]),
    ("justfile", &["just"]),
    ("CMakeLists.txt", &["cmake", "c", "cpp"]),
    ("tsconfig.json", &["typescript"]),
    (".terraform.lock.hcl", &["terraform"]),
];

/// Directory-presence rules — relative path → tags to insert when present.
const DIR_RULES: &[(&str, &[&str])] = &[
    (".github/workflows", &["ci-github"]),
    (".gitlab", &["ci-gitlab"]),
    ("k8s", &["kubernetes"]),
    ("kustomize", &["kubernetes", "kustomize"]),
    ("helm", &["kubernetes", "helm"]),
    ("terraform", &["terraform"]),
];

/// Files we read for deep-ish content parsing (in addition to mere presence
/// checks). Keep in sync with `detect()` — adding a new content parse here
/// without updating the function (or vice versa) breaks cache invalidation.
const CONTENT_FILES: &[&str] = &["package.json", "Cargo.toml", "pyproject.toml"];

/// Every relative path `detect()` might look at. Used by the cache layer
/// to decide whether the cached result is still valid.
pub fn candidate_source_paths() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (name, _) in FILE_RULES {
        out.push(name);
    }
    for (name, _) in DIR_RULES {
        out.push(name);
    }
    for name in CONTENT_FILES {
        out.push(name);
    }
    // Dedup while preserving deterministic order.
    let mut seen = BTreeSet::new();
    out.retain(|p| seen.insert(*p));
    out
}

/// Run fingerprint-tier detection rooted at `project_root`. This is the
/// cache-miss path; most callers want `detect_cached()`.
pub fn detect(project_root: &Path) -> Detection {
    let mut tags = BTreeSet::new();
    let mut signals = Vec::new();

    for (name, t) in FILE_RULES {
        if project_root.join(name).exists() {
            signals.push((*name).to_string());
            for tag in *t {
                tags.insert((*tag).to_string());
            }
        }
    }

    for (name, t) in DIR_RULES {
        if project_root.join(name).is_dir() {
            signals.push((*name).to_string());
            for tag in *t {
                tags.insert((*tag).to_string());
            }
        }
    }

    // ---- package.json deep-ish parse (JS framework names from deps) ----
    if let Ok(pkg) = std::fs::read_to_string(project_root.join("package.json")) {
        tags.insert("javascript".into());
        signals.push("package.json".into());
        let deps_tags: &[(&str, &str)] = &[
            ("next", "nextjs"),
            ("react", "react"),
            ("@sveltejs/kit", "sveltekit"),
            ("svelte", "svelte"),
            ("vue", "vue"),
            ("nuxt", "nuxt"),
            ("vite", "vite"),
            ("astro", "astro"),
            ("remix", "remix"),
            ("@angular/core", "angular"),
            ("solid-js", "solid"),
            ("tailwindcss", "tailwind"),
            ("@nestjs/core", "nestjs"),
            ("express", "express"),
            ("fastify", "fastify"),
            ("prisma", "prisma"),
        ];
        for (needle, tag) in deps_tags {
            if pkg.contains(&format!("\"{needle}\"")) {
                tags.insert((*tag).to_string());
            }
        }
    }

    // ---- Cargo.toml — pick out a few common framework crates ----
    if let Ok(cargo) = std::fs::read_to_string(project_root.join("Cargo.toml")) {
        let crate_tags: &[(&str, &str)] = &[
            ("axum", "axum"),
            ("actix-web", "actix"),
            ("rocket", "rocket"),
            ("tonic", "tonic"),
            ("sqlx", "sqlx"),
            ("diesel", "diesel"),
            ("tokio", "tokio"),
            ("leptos", "leptos"),
            ("yew", "yew"),
            ("bevy", "bevy"),
        ];
        for (needle, tag) in crate_tags {
            // Match `axum = ` or `axum=` or `axum = { ` at start of a line
            // (after optional whitespace), avoiding substring matches like
            // `axum-extra` for `axum` (handled by the trailing `=` / `{`).
            let pat1 = format!("\n{needle} =");
            let pat2 = format!("\n{needle}=");
            if cargo.contains(&pat1) || cargo.contains(&pat2) {
                tags.insert((*tag).to_string());
            }
        }
    }

    // ---- pyproject.toml — minimal framework hint ----
    if let Ok(py) = std::fs::read_to_string(project_root.join("pyproject.toml")) {
        let py_tags: &[(&str, &str)] = &[
            ("fastapi", "fastapi"),
            ("django", "django"),
            ("flask", "flask"),
        ];
        for (needle, tag) in py_tags {
            if py.contains(&format!("\"{needle}\"")) || py.contains(&format!("'{needle}'")) {
                tags.insert((*tag).to_string());
            }
        }
    }

    signals.sort();
    Detection {
        stack: tags.into_iter().collect(),
        signals,
    }
}

/// On-disk cache record. Lives at `<project_root>/.skill-pool/detected.json`.
#[derive(Debug, Serialize, Deserialize)]
struct CacheRecord {
    /// The detected stack tags, sorted.
    stack: Vec<String>,
    /// ISO-8601 (UTC) timestamp the cache was written.
    cached_at: String,
    /// Map of relative source path → mtime (seconds since UNIX epoch) at
    /// the time of detection. Files that didn't exist when we last ran
    /// are intentionally omitted; their later appearance is what triggers
    /// invalidation.
    sources: BTreeMap<String, i64>,
}

/// Run detection with a `<project_root>/.skill-pool/detected.json` cache
/// in front. Returns the cached stack iff every recorded mtime is
/// unchanged AND no new candidate source file has appeared since the last
/// run. Any mismatch triggers a fresh `detect()` and an atomic rewrite.
pub fn detect_cached(project_root: &Path) -> Result<Detection> {
    let cache_path = cache_path(project_root);
    if let Ok(record) = load_cache(&cache_path) {
        if cache_is_fresh(project_root, &record) {
            // Cache hit: return without re-walking the filesystem deeply.
            // We don't repopulate `signals` from the cache (it's a debug
            // surface that gets recomputed on the next miss).
            return Ok(Detection {
                stack: record.stack,
                signals: Vec::new(),
            });
        }
    }

    let detection = detect(project_root);
    if let Err(e) = write_cache(project_root, &cache_path, &detection) {
        // A failure to write the cache should never break detection.
        tracing::warn!(error = %e, "failed to write detection cache");
    }
    Ok(detection)
}

fn cache_path(project_root: &Path) -> PathBuf {
    project_root.join(CACHE_REL)
}

fn load_cache(path: &Path) -> Result<CacheRecord> {
    let raw = std::fs::read_to_string(path).context("read detection cache")?;
    let rec: CacheRecord = serde_json::from_str(&raw).context("parse detection cache")?;
    Ok(rec)
}

/// True when every recorded mtime matches AND no NEW candidate source
/// has appeared since the cache was written.
fn cache_is_fresh(project_root: &Path, record: &CacheRecord) -> bool {
    // 1. Every recorded source must exist with the same mtime.
    for (rel, recorded) in &record.sources {
        match file_mtime(&project_root.join(rel)) {
            Some(now) if now == *recorded => {}
            _ => return false,
        }
    }
    // 2. No candidate source path may exist on disk while being absent
    //    from `record.sources` — that would mean a new file appeared.
    for rel in candidate_source_paths() {
        if record.sources.contains_key(rel) {
            continue;
        }
        if project_root.join(rel).exists() {
            return false;
        }
    }
    true
}

fn write_cache(project_root: &Path, cache_path: &Path, detection: &Detection) -> Result<()> {
    let mut sources = BTreeMap::new();
    for rel in candidate_source_paths() {
        if let Some(mt) = file_mtime(&project_root.join(rel)) {
            sources.insert(rel.to_string(), mt);
        }
    }

    let cached_at = chrono::Utc::now().to_rfc3339();
    let record = CacheRecord {
        stack: detection.stack.clone(),
        cached_at,
        sources,
    };

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).context("create .skill-pool/ dir")?;
    }

    // Atomic write: serialize → write to sibling tmp → rename.
    // Hand-rolled (avoids pulling tempfile out of dev-deps).
    let parent = cache_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(".detected.json.{pid}.{nanos}.tmp"));
    let json = serde_json::to_vec_pretty(&record).context("serialize cache record")?;
    std::fs::write(&tmp_path, &json).context("write cache tempfile")?;
    std::fs::rename(&tmp_path, cache_path).context("rename cache tempfile")?;
    Ok(())
}

fn file_mtime(path: &Path) -> Option<i64> {
    let md = std::fs::metadata(path).ok()?;
    let mt = md.modified().ok()?;
    let dur = mt.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn empty_dir_detects_nothing() {
        let dir = td();
        let d = detect(dir.path());
        assert!(d.stack.is_empty());
        assert!(d.signals.is_empty());
    }

    #[test]
    fn rust_axum_with_postgres() {
        let dir = td();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"foo\"\n\n[dependencies]\naxum = \"0.7\"\nsqlx = \"0.8\"\ntokio = { version = \"1\" }\n",
        )
        .unwrap();
        let d = detect(dir.path());
        assert!(d.stack.contains(&"rust".to_string()), "tags: {:?}", d.stack);
        assert!(d.stack.contains(&"axum".to_string()), "tags: {:?}", d.stack);
        assert!(d.stack.contains(&"sqlx".to_string()));
        assert!(d.stack.contains(&"tokio".to_string()));
    }

    #[test]
    fn nextjs_with_tailwind() {
        let dir = td();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","dependencies":{"next":"^14","react":"^18","tailwindcss":"^4"}}"#,
        )
        .unwrap();
        let d = detect(dir.path());
        assert!(d.stack.contains(&"javascript".to_string()));
        assert!(d.stack.contains(&"nextjs".to_string()));
        assert!(d.stack.contains(&"react".to_string()));
        assert!(d.stack.contains(&"tailwind".to_string()));
    }

    #[test]
    fn cargo_substring_match_doesnt_pollute() {
        // axum-extra should not match the bare-`axum` rule.
        let dir = td();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"foo\"\n\n[dependencies]\naxum-extra = \"0.7\"\n",
        )
        .unwrap();
        let d = detect(dir.path());
        assert!(d.stack.contains(&"rust".to_string()));
        assert!(
            !d.stack.contains(&"axum".to_string()),
            "tags: {:?}",
            d.stack
        );
    }

    #[test]
    fn ci_dirs_detected() {
        let dir = td();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        std::fs::create_dir_all(dir.path().join("k8s")).unwrap();
        let d = detect(dir.path());
        assert!(d.stack.contains(&"ci-github".to_string()));
        assert!(d.stack.contains(&"kubernetes".to_string()));
    }

    #[test]
    fn nix_and_just() {
        let dir = td();
        std::fs::write(dir.path().join("flake.nix"), "{}").unwrap();
        std::fs::write(dir.path().join("justfile"), "build:\n\tcargo build").unwrap();
        let d = detect(dir.path());
        assert!(d.stack.contains(&"nix".to_string()));
        assert!(d.stack.contains(&"just".to_string()));
    }

    // ---------- cache layer ----------

    #[test]
    fn cache_hit_returns_cached_stack_without_re_reading() {
        // Write a fake cache that disagrees with what `detect()` would
        // produce. If `detect_cached` honours the cache on the happy path,
        // we'll see the *cached* (fake) stack, not the live one.
        let dir = td();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\n\n[dependencies]\naxum=\"0.7\"\n",
        )
        .unwrap();
        // Confirm a fresh detect returns the live stack first.
        let live = detect(dir.path());
        assert!(live.stack.contains(&"rust".to_string()));
        assert!(live.stack.contains(&"axum".to_string()));

        // Hand-craft a cache claiming a totally different stack.
        let cargo_mtime = file_mtime(&dir.path().join("Cargo.toml")).unwrap();
        let mut sources = BTreeMap::new();
        sources.insert("Cargo.toml".to_string(), cargo_mtime);
        let record = CacheRecord {
            stack: vec!["sentinel-from-cache".into()],
            cached_at: "1970-01-01T00:00:00Z".into(),
            sources,
        };
        std::fs::create_dir_all(dir.path().join(".skill-pool")).unwrap();
        std::fs::write(
            dir.path().join(CACHE_REL),
            serde_json::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let cached = detect_cached(dir.path()).unwrap();
        assert_eq!(
            cached.stack,
            vec!["sentinel-from-cache".to_string()],
            "cache hit must short-circuit detection"
        );
    }

    #[test]
    fn mtime_change_on_source_invalidates_cache() {
        let dir = td();
        // Initial: just a Cargo.toml with axum.
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\n\n[dependencies]\naxum=\"0.7\"\n",
        )
        .unwrap();
        let first = detect_cached(dir.path()).unwrap();
        assert!(first.stack.contains(&"axum".to_string()));
        assert!(dir.path().join(CACHE_REL).exists());

        // Bump the mtime forward by 2s and change content (axum → rocket).
        let cargo = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo,
            "[package]\nname=\"x\"\n\n[dependencies]\nrocket=\"0.5\"\n",
        )
        .unwrap();
        // Force a different mtime even on filesystems with low resolution.
        let new_mt = SystemTime::now() + std::time::Duration::from_secs(5);
        let f = std::fs::File::open(&cargo).unwrap();
        f.set_modified(new_mt).unwrap();

        let second = detect_cached(dir.path()).unwrap();
        assert!(
            !second.stack.contains(&"axum".to_string()),
            "stale tag survived invalidation: {:?}",
            second.stack
        );
        assert!(
            second.stack.contains(&"rocket".to_string()),
            "new tag missing after invalidation: {:?}",
            second.stack
        );
    }

    #[test]
    fn new_source_file_invalidates_cache() {
        let dir = td();
        // Initial: only flake.nix.
        std::fs::write(dir.path().join("flake.nix"), "{}").unwrap();
        let first = detect_cached(dir.path()).unwrap();
        assert!(first.stack.contains(&"nix".to_string()));
        assert!(
            !first.stack.contains(&"rust".to_string()),
            "no Cargo.toml yet, so no rust tag: {:?}",
            first.stack
        );

        // Drop a NEW candidate source file the cache doesn't know about.
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\n\n[dependencies]\n",
        )
        .unwrap();

        let second = detect_cached(dir.path()).unwrap();
        assert!(
            second.stack.contains(&"rust".to_string()),
            "new file failed to invalidate cache: {:?}",
            second.stack
        );
    }
}
