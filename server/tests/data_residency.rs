//! Data-residency end-to-end smoke.
//!
//! Locks down the deploy contract for per-tenant bundle-storage override
//! (`tenants.storage_uri` from migration 0018):
//!
//!   1. Two tenants, one with a `storage_uri` override pointing at a
//!      *second* tempdir (the "EU region").
//!   2. Each tenant publishes a bundle.
//!   3. The override tenant's bundle bytes land in the override dir.
//!   4. The default tenant's bundle bytes land in the global dir.
//!   5. Cross-check: the override dir does NOT contain the default
//!      tenant's bundle and vice versa.
//!
//! In shared deploys without overrides this test class doesn't apply;
//! the behaviour is "everything in the global backend" by default.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use walkdir::WalkDir;

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

fn count_files(dir: &std::path::Path) -> usize {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

#[tokio::test]
async fn per_tenant_storage_uri_overrides_global_default() -> Result<()> {
    // 1. Postgres
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let pg_port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // 2. Two tempdirs — the global default and the override.
    let default_dir = tempfile::tempdir()?;
    let override_dir = tempfile::tempdir()?;
    let default_uri = format!("fs://{}", default_dir.path().display());
    let override_uri = format!("fs://{}", override_dir.path().display());

    // 3. Two tenants. `eu-acme` gets the override; `us-acme` rides the default.
    admin::create_tenant(&pool, "eu-acme", "Acme EU", "enterprise").await?;
    admin::create_tenant(&pool, "us-acme", "Acme US", "team").await?;
    admin::set_tenant_residency(&pool, "eu-acme", Some("eu-west-1"), Some(&override_uri)).await?;

    let eu_token = admin::create_token(&pool, "eu-acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;
    let us_token = admin::create_token(&pool, "us-acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    // 4. Server with the *default* tenancy + the *default* storage URI.
    //    The eu-acme override is set per-tenant in the DB; no server-side
    //    config change required.
    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        redis_url: None,
        db_pool_size: 20,
        storage_uri: default_uri.clone(),
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
        decay_check_interval_secs: 0,
    };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
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

    // 5. Both tenants publish.
    for (tenant, token, name) in [
        ("eu-acme", &eu_token, "eu-skill"),
        ("us-acme", &us_token, "us-skill"),
    ] {
        let bundle = build_bundle(&format!(
            "---\nname: {name}\ndescription: a residency probe.\n---\n\n# {name}\n"
        ));
        let form = Form::new()
            .text(
                "metadata",
                format!(r#"{{"slug":"{name}","version":"1.0.0"}}"#),
            )
            .part(
                "bundle",
                Part::bytes(bundle.to_vec())
                    .file_name(format!("{name}.tar.gz"))
                    .mime_str("application/gzip")?,
            );
        let resp = c
            .post(format!("{base}/v1/skills"))
            .header("x-skill-pool-tenant", tenant)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await?;
        assert_eq!(
            resp.status().as_u16(),
            201,
            "publish to {tenant} failed: {}",
            resp.text().await?
        );
    }

    // 6. The override dir has eu-acme's bundle; the default dir has us-acme's.
    let default_files = count_files(default_dir.path());
    let override_files = count_files(override_dir.path());
    assert!(
        default_files >= 1,
        "default dir should hold us-acme's bundle, found {default_files} files"
    );
    assert!(
        override_files >= 1,
        "override dir should hold eu-acme's bundle, found {override_files} files"
    );

    // 7. Cross-check: each dir contains exactly one tenant's bundle.
    //    Bundle keys are `{tenant_id}/{slug}/{version}.tar.gz` so we can
    //    grep by file content (each contains a unique SKILL.md name).
    let mut default_contains_eu = false;
    let mut default_contains_us = false;
    for entry in WalkDir::new(default_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let bytes = std::fs::read(entry.path())?;
        let mut gz = flate2::read::GzDecoder::new(bytes.as_slice());
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut gz, &mut decoded).ok();
        let s = String::from_utf8_lossy(&decoded);
        if s.contains("eu-skill") {
            default_contains_eu = true;
        }
        if s.contains("us-skill") {
            default_contains_us = true;
        }
    }
    assert!(
        !default_contains_eu,
        "EU tenant's bundle leaked into the default storage dir"
    );
    assert!(
        default_contains_us,
        "US tenant's bundle should be in the default storage dir"
    );

    let mut override_contains_eu = false;
    let mut override_contains_us = false;
    for entry in WalkDir::new(override_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let bytes = std::fs::read(entry.path())?;
        let mut gz = flate2::read::GzDecoder::new(bytes.as_slice());
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut gz, &mut decoded).ok();
        let s = String::from_utf8_lossy(&decoded);
        if s.contains("eu-skill") {
            override_contains_eu = true;
        }
        if s.contains("us-skill") {
            override_contains_us = true;
        }
    }
    assert!(
        override_contains_eu,
        "EU tenant's bundle should be in the override (eu-acme) storage dir"
    );
    assert!(
        !override_contains_us,
        "US tenant's bundle leaked into the override storage dir"
    );

    // 8. GET round-trip: each tenant can still fetch their own bundle.
    for (tenant, token, name) in [
        ("eu-acme", &eu_token, "eu-skill"),
        ("us-acme", &us_token, "us-skill"),
    ] {
        let resp = c
            .get(format!("{base}/v1/skills/{name}/bundle.tar.gz?bytes=true"))
            .header("x-skill-pool-tenant", tenant)
            .bearer_auth(token)
            .send()
            .await?;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "GET bundle for {tenant} failed"
        );
        let bytes = resp.bytes().await?;
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "expected gzip magic for {tenant}");
    }

    Ok(())
}
