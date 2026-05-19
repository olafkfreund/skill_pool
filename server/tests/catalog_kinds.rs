//! Phase 5: agents + commands as parallel surfaces to skills.
//!
//! Slice 1 just adds the `kind` discriminator on `skills`. We verify
//! catalog isolation across the three kinds + default-skill semantics
//! preserved for existing clients.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

struct Harness {
    base: String,
    token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
    let pg = Postgres::default().with_name("pgvector/pgvector").with_tag("pg16").start().await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new().max_connections(4).connect(&db_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    let token = admin::create_token(&pool, "acme", "test", "tenant:admin skills:read skills:publish").await?.raw_token;

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
    };
    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness { base: format!("http://{addr}"), token, _pg: pg, _storage_dir: storage_dir })
}

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

fn cl() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap()
}
fn req(c: &reqwest::Client, m: reqwest::Method, b: &str, p: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{b}{p}")).header("x-skill-pool-tenant", "acme")
}
fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

async fn publish(c: &reqwest::Client, h: &Harness, slug: &str, kind: Option<&str>) -> Result<u16> {
    let body = format!(
        "---\nname: {slug}\ndescription: a {} called {slug}\ntags: [test]\n---\n\n# {slug}\n",
        kind.unwrap_or("skill")
    );
    let bundle = build_bundle(&body);
    let mut meta = json!({ "slug": slug, "version": "1.0.0" });
    if let Some(k) = kind {
        meta["kind"] = json!(k);
    }
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec()).file_name(format!("{slug}.tar.gz")).mime_str("application/gzip")?,
    );
    let r = authed(req(c, reqwest::Method::POST, &h.base, "/v1/skills"), &h.token).multipart(form).send().await?;
    Ok(r.status().as_u16())
}

#[tokio::test]
async fn catalog_kinds_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // 1. Publish one of each kind. No `kind` field on metadata = default skill.
    assert_eq!(publish(&c, &h, "my-skill", None).await?, 201);
    assert_eq!(publish(&c, &h, "my-skill-2", Some("skill")).await?, 201);
    assert_eq!(publish(&c, &h, "code-reviewer", Some("agent")).await?, 201);
    assert_eq!(publish(&c, &h, "deploy", Some("command")).await?, 201);

    // 2. Default list returns only skills.
    let list: Vec<Value> = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills"), &h.token)
        .send().await?.json().await?;
    let slugs: Vec<&str> = list.iter().map(|s| s["slug"].as_str().unwrap()).collect();
    assert!(slugs.contains(&"my-skill"), "{slugs:?}");
    assert!(slugs.contains(&"my-skill-2"));
    assert!(!slugs.contains(&"code-reviewer"), "agents must not show up in default list: {slugs:?}");
    assert!(!slugs.contains(&"deploy"), "commands must not show up in default list: {slugs:?}");

    // 3. ?kind=agent returns only agents.
    let list: Vec<Value> = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills?kind=agent"), &h.token)
        .send().await?.json().await?;
    let slugs: Vec<&str> = list.iter().map(|s| s["slug"].as_str().unwrap()).collect();
    assert_eq!(slugs, vec!["code-reviewer"], "{slugs:?}");

    // 4. ?kind=command returns only commands.
    let list: Vec<Value> = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills?kind=command"), &h.token)
        .send().await?.json().await?;
    let slugs: Vec<&str> = list.iter().map(|s| s["slug"].as_str().unwrap()).collect();
    assert_eq!(slugs, vec!["deploy"], "{slugs:?}");

    // 5. ?kind=garbage → 400.
    let r = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills?kind=garbage"), &h.token).send().await?;
    assert_eq!(r.status().as_u16(), 400);

    // 6. get_one with ?kind= follows the same rules. Default = skill.
    let r = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/code-reviewer"), &h.token).send().await?;
    // code-reviewer is an agent — looking it up as a skill must 404.
    assert_eq!(r.status().as_u16(), 404);
    let body: Value = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/code-reviewer?kind=agent"), &h.token)
        .send().await?.json().await?;
    assert_eq!(body["slug"], "code-reviewer");

    // 7. Bundle download obeys the same kind filter.
    let r = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/deploy/bundle.tar.gz"), &h.token).send().await?;
    assert_eq!(r.status().as_u16(), 404, "deploying as skill must 404");
    let r = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/deploy/bundle.tar.gz?kind=command"), &h.token)
        .send().await?;
    assert_eq!(r.status().as_u16(), 200);

    // 8. Detail endpoint also filters.
    let r = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/code-reviewer/detail"), &h.token).send().await?;
    assert_eq!(r.status().as_u16(), 404);
    let body: Value = authed(req(&c, reqwest::Method::GET, &h.base, "/v1/skills/code-reviewer/detail?kind=agent"), &h.token)
        .send().await?.json().await?;
    assert_eq!(body["slug"], "code-reviewer");

    // 9. Decay query is skills-only (an agent never shows even if stale).
    let candidates: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/skills/decay?days=1&max_uses=99"),
        &h.token,
    ).send().await?.json().await?;
    let slugs: Vec<&str> = candidates.iter().map(|s| s["slug"].as_str().unwrap()).collect();
    assert!(!slugs.contains(&"code-reviewer"));
    assert!(!slugs.contains(&"deploy"));

    // 10. Bogus kind on publish → 400.
    let bundle = build_bundle("---\nname: x\ndescription: x\n---\n\nbody\n");
    let form = Form::new().text("metadata", json!({"slug":"x","version":"1.0.0","kind":"plugin"}).to_string())
        .part("bundle", Part::bytes(bundle.to_vec()).file_name("x.tar.gz").mime_str("application/gzip")?);
    let r = authed(req(&c, reqwest::Method::POST, &h.base, "/v1/skills"), &h.token).multipart(form).send().await?;
    assert_eq!(r.status().as_u16(), 400);

    Ok(())
}
