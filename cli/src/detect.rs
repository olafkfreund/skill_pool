//! Stack detection from project files.
//!
//! Phase 3 ships the **fingerprint** tier — fast (<100ms typical),
//! deterministic, no network. Catches ~90% of common stacks. Tiers 2
//! (deep manifest parsing) and 3 (LLM fallback) layer on later.
//!
//! Output is a deduped sorted list of lower-case tag strings that the
//! server's `/v1/bootstrap` endpoint matches against `tenant_stack_mappings`.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Detection {
    pub stack: Vec<String>,
    /// Files that contributed to detection — useful for `--verbose` output.
    pub signals: Vec<String>,
}

/// Run fingerprint-tier detection rooted at `project_root`.
pub fn detect(project_root: &Path) -> Detection {
    let mut tags = BTreeSet::new();
    let mut signals = Vec::new();

    // ---- single-file presence ----
    let file_rules: &[(&str, &[&str])] = &[
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
    for (name, t) in file_rules {
        if project_root.join(name).exists() {
            signals.push((*name).to_string());
            for tag in *t {
                tags.insert((*tag).to_string());
            }
        }
    }

    // ---- directory presence ----
    let dir_rules: &[(&str, &[&str])] = &[
        (".github/workflows", &["ci-github"]),
        (".gitlab", &["ci-gitlab"]),
        ("k8s", &["kubernetes"]),
        ("kustomize", &["kubernetes", "kustomize"]),
        ("helm", &["kubernetes", "helm"]),
        ("terraform", &["terraform"]),
    ];
    for (name, t) in dir_rules {
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
}
