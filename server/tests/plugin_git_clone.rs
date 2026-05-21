//! Issue #31 — `git clone` against skill-pool's dumb-HTTP smart git endpoint.
//!
//! Flow:
//!   1. Boot pgvector + the full router.
//!   2. Create tenant `acme`, mint a write token, then upload a real
//!      single-skill bundle via the existing multipart POST /v1/skills.
//!      We need a real bundle in storage so the materialiser can extract
//!      it into the plugin tree.
//!   3. Publish an `internal` plugin referencing that skill via POST
//!      /v1/plugins. The publish hook materialises the bare repo.
//!   4. `git2::Repository::clone(...)` against the in-process server.
//!   5. Assert the clone tree contains:
//!        * `.claude-plugin/plugin.json` with the published manifest.
//!        * `skills/<skill-slug>/SKILL.md` with the original bundle's content.
//!
//! This is the live wire-protocol test — pkt-line framing, capability
//! advertisement, packfile generation all exercised against an actual
//! git2 client.

use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

const SKILL_MD: &str = "---\nname: rust-fmt\ndescription: format rust\n---\nHello plugin world\n";

fn build_skill_bundle() -> Bytes {
    let mut tar = tar::Builder::new(Vec::new());
    let body = SKILL_MD.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_path("SKILL.md").unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body).unwrap();
    let tar_bytes = tar.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    Bytes::from(gz.finish().unwrap())
}

#[tokio::test]
async fn git_clone_yields_plugin_tree() -> Result<()> {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    let token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        redis_url: None,
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
        decay_check_interval_secs: 0,
        git_repo_path: None,
    };
    // SAFETY: leaf integration test, no other thread touches env.
    unsafe { std::env::remove_var("SKILL_POOL_REDIS_URL") };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");

    // 1. Upload a real bundle via POST /v1/skills (multipart).
    let client = reqwest::Client::new();
    let metadata = json!({
        "slug": "rust-fmt",
        "version": "1.0.0",
        "kind": "skill",
        "tags": [],
    });
    let bundle = build_skill_bundle();
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .part(
            "bundle",
            reqwest::multipart::Part::bytes(bundle.to_vec())
                .file_name("rust-fmt.tar.gz")
                .mime_str("application/gzip")?,
        );
    let resp = client
        .post(format!("{base}/v1/skills"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {token}"))
        .multipart(form)
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        201,
        "skill publish failed: {}",
        resp.text().await?
    );

    // 2. Publish an internal plugin referencing the skill.
    let body = json!({
        "slug": "rust-toolkit",
        "manifest": {
            "name": "rust-toolkit",
            "version": "1.2.0",
            "description": "Curated rust dev essentials",
            "keywords": ["rust"],
        },
        "contents": [
            { "kind": "skill", "slug": "rust-fmt", "version": "1.0.0" },
        ],
        "sourcing_mode": "internal",
    });
    let resp = client
        .post(format!("{base}/v1/plugins"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    assert_eq!(status.as_u16(), 201, "plugin publish failed: {payload}");

    // 3. git2 clone against the in-process server. The publish hook in
    //    routes/plugins.rs::publish already materialised the bare repo.
    let clone_url = format!(
        "http://127.0.0.1:{}/git/plugins/rust-toolkit.git",
        addr.port()
    );
    let clone_dir = tempfile::tempdir()?;
    // Override Host so the tenant resolver picks `acme`. git2 doesn't
    // expose a header-set API, so we use the x-skill-pool-tenant fallback
    // via remote callbacks isn't possible either — instead we point
    // clone at a URL whose Host is acme.localhost via reqwest, then
    // assert the clone via git2 separately. Actually git2 needs to
    // resolve the host, so we use the IP-based URL above and inject the
    // tenant header through libgit2's `remote_headers` API.
    //
    // Approach: open a transport, set custom headers via
    // RemoteCallbacks::transport. Simpler path: configure git2's
    // FetchOptions with proxy callbacks. The cleanest portable API is
    // `Repository::clone()` + `RemoteCallbacks` with a `transport`
    // factory — but for an HTTP-only smart endpoint behind libgit2's
    // built-in HTTP transport, we use the `http.extraHeader` config
    // override. libgit2 reads `http.extraHeader` from the repo config
    // before the clone runs; the easiest way to inject one for a clone
    // is via a temporary repo.
    let repo_path = clone_dir.path().join("rust-toolkit");
    let mut builder = git2::build::RepoBuilder::new();
    let mut fo = git2::FetchOptions::new();
    let mut cb = git2::RemoteCallbacks::new();
    cb.certificate_check(|_, _| Ok(git2::CertificateCheckStatus::CertificateOk));
    fo.remote_callbacks(cb);
    fo.custom_headers(&["x-skill-pool-tenant: acme"]);
    builder.fetch_options(fo);
    builder
        .clone(&clone_url, &repo_path)
        .map_err(|e| anyhow::anyhow!("git clone failed: {e}"))?;

    // 4. Assert tree contents.
    let manifest_path = repo_path.join(".claude-plugin").join("plugin.json");
    assert!(
        manifest_path.exists(),
        "missing .claude-plugin/plugin.json in cloned tree"
    );
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    assert_eq!(manifest["name"], "rust-toolkit");
    assert_eq!(manifest["version"], "1.2.0");

    let skill_md = repo_path
        .join("skills")
        .join("rust-fmt")
        .join("SKILL.md");
    assert!(
        skill_md.exists(),
        "missing skills/rust-fmt/SKILL.md in cloned tree"
    );
    let extracted = std::fs::read_to_string(&skill_md)?;
    assert_eq!(extracted, SKILL_MD);

    drop(pool);
    Ok(())
}
