//! Issue #31 — publishing the same plugin twice produces a single commit
//! on `refs/heads/main` and one row in `plugin_marketplace_entries`.
//!
//! Why this test exists: the post-publish hook in `routes/plugins.rs`
//! runs in three independently-failable phases (DB commit → git tree
//! write → marketplace upsert). To recover from a partial failure, a
//! retried publish must converge — never accumulate stale commits or
//! duplicate marketplace rows. This test exercises that contract
//! end-to-end via the public API.

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

fn build_bundle() -> Bytes {
    const SKILL_MD: &str = "---\nname: rust-fmt\ndescription: fmt\n---\nHello\n";
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

async fn count_commits_on_main(repo_path: &std::path::Path) -> Result<usize> {
    let path = repo_path.to_path_buf();
    let n = tokio::task::spawn_blocking(move || -> Result<usize> {
        let repo = git2::Repository::open_bare(&path)?;
        let head = repo.head()?.peel_to_commit()?;
        let mut walk = repo.revwalk()?;
        walk.push(head.id())?;
        Ok(walk.count())
    })
    .await??;
    Ok(n)
}

#[tokio::test]
async fn republish_with_same_input_does_not_add_a_new_commit() -> Result<()> {
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
    let client = reqwest::Client::new();

    // Upload a real bundle so the materialiser has something to extract.
    let metadata = json!({
        "slug": "rust-fmt",
        "version": "1.0.0",
        "kind": "skill",
        "tags": [],
    });
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .part(
            "bundle",
            reqwest::multipart::Part::bytes(build_bundle().to_vec())
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
    assert_eq!(resp.status().as_u16(), 201);

    let publish_body = json!({
        "slug": "rust-toolkit",
        "manifest": {
            "name": "rust-toolkit",
            "version": "1.0.0",
            "description": "first try",
        },
        "contents": [
            { "kind": "skill", "slug": "rust-fmt", "version": "1.0.0" },
        ],
        "sourcing_mode": "internal",
    });

    // ----- First publish: should create a single commit ---------------
    let resp = client
        .post(format!("{base}/v1/plugins"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {token}"))
        .json(&publish_body)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 201);

    // Locate the repo on disk. Tenant UUID is the slug-keyed lookup.
    let tenant_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = 'acme'")
        .fetch_one(&pool)
        .await?;
    let repo_path = storage_dir
        .path()
        .join(tenant_id.to_string())
        .join("plugins")
        .join("rust-toolkit.git");

    assert!(repo_path.exists(), "bare repo not created on first publish");
    let commits_after_first = count_commits_on_main(&repo_path).await?;
    assert_eq!(
        commits_after_first, 1,
        "first publish should create exactly one commit"
    );

    let entries_after_first: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugin_marketplace_entries WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(entries_after_first, 1);

    // ----- Republish: identical inputs except `version` must bump in
    // the manifest, but the API insert uniqueness key is (tenant, slug,
    // version) — so a true byte-identical republish would 409. The
    // idempotency contract we care about is at the git-write level: if
    // the **tree** matches, we don't churn HEAD. To test that, archive
    // the existing version first so the next publish is accepted by
    // the API, then publish a row whose content + manifest produce an
    // identical tree (same name, same contents). Compare commits.
    //
    // Note: this exercises the materialise path's short-circuit. The
    // marketplace UPSERT side is exercised by counting rows after the
    // second publish — must stay at 1.
    let resp = client
        .delete(format!("{base}/v1/plugins/rust-toolkit/versions/1.0.0"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    let second_body = json!({
        "slug": "rust-toolkit",
        "manifest": {
            "name": "rust-toolkit",
            "version": "1.0.0",
            "description": "first try",  // identical content
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
        .json(&second_body)
        .send()
        .await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    assert_eq!(status.as_u16(), 201, "republish failed: {payload}");

    let commits_after_second = count_commits_on_main(&repo_path).await?;
    assert_eq!(
        commits_after_second, 1,
        "republish with identical tree should not add a new commit (got {commits_after_second})"
    );

    let entries_after_second: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plugin_marketplace_entries WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        entries_after_second, 1,
        "marketplace UPSERT must not duplicate rows on republish"
    );

    drop(pool);
    Ok(())
}
