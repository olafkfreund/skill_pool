//! Dedicated-mode end-to-end smoke.
//!
//! Locks down the deploy contract for `SKILL_POOL_TENANCY_MODE=dedicated`:
//!
//!   1. The server boots with `tenancy_mode.mode = "dedicated"` and a
//!      pinned `tenant_slug`.
//!   2. A request to `/v1/skills` with **no** `X-Skill-Pool-Tenant` header
//!      and **no** recognisable Host subdomain still resolves to the pinned
//!      tenant — i.e. the request goes through the dedicated short-circuit
//!      in `tenant::slug_from_request`.
//!   3. The catalog returned belongs to that tenant (we publish one skill
//!      under "acme" and assert the GET sees it).
//!
//! In shared mode the same request would 400 with `tenant_resolution_failed`,
//! so a passing test is sufficient proof that the dedicated path is wired.
//!
//! Requires a working Docker socket (testcontainers). Run with:
//! `cargo test --test dedicated_mode`.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

fn build_bundle(skill_md: &str) -> Bytes {
    let mut tar = tar::Builder::new(Vec::new());
    let body = skill_md.as_bytes();
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
async fn dedicated_mode_pins_tenant_without_header() -> Result<()> {
    // 1. Postgres
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let pg_port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

    // 2. Migrations
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // 3. Tenant + publish token. We seed one tenant ("acme"); the pinned
    //    slug below must match this row or the extractor will 401.
    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    // 4. Build a Config with tenancy_mode = dedicated. We build it directly
    //    (not via env vars) because Config's fields are public and figment
    //    plumbing is verified separately by config unit tests.
    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw {
            mode: "dedicated".into(),
            tenant_slug: Some("acme".into()),
        },
        database_url: db_url,
        database_read_url: None,
        redis_url: None,
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "https://acme-skill-pool.example.test".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
        decay_check_interval_secs: 0,
    };
    // Sanity: the resolver maps the raw config onto the enum variant we expect.
    match cfg.resolved_tenancy() {
        config::TenancyMode::Dedicated { tenant_slug } => assert_eq!(tenant_slug, "acme"),
        config::TenancyMode::Shared => panic!("expected Dedicated, got Shared"),
    }

    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");

    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    // 5. Publish one skill — uses the same dedicated short-circuit; we
    //    deliberately do NOT send X-Skill-Pool-Tenant.
    let bundle = build_bundle(
        "---\nname: hello\ndescription: Says hello when greeted.\ntags: [smoke]\n---\n\nBody.\n",
    );
    let form = Form::new()
        .text("metadata", r#"{"slug":"hello","version":"1.0.0"}"#)
        .part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name("hello.tar.gz")
                .mime_str("application/gzip")?,
        );
    let resp = c
        .post(format!("{base}/v1/skills"))
        .bearer_auth(&acme_token)
        .multipart(form)
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        201,
        "publish in dedicated mode (no tenant header) should 201, got: {}",
        resp.text().await?
    );

    // 6. List skills — NO tenant header, NO subdomain. The dedicated path
    //    is the only thing that can make this succeed.
    let resp = c
        .get(format!("{base}/v1/skills"))
        .bearer_auth(&acme_token)
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "list in dedicated mode (no tenant header) should 200, got: {}",
        resp.text().await?
    );
    let list: Vec<Value> = resp.json().await?;
    assert!(
        list.iter().any(|s| s["slug"] == "hello"),
        "expected to see acme's `hello` skill in dedicated-mode catalog: {list:?}"
    );

    // Cleanup
    drop(pg);
    drop(storage_dir);
    Ok(())
}
