//! End-to-end integration test for the skill-pool server.
//!
//! Brings up Postgres via testcontainers, uses a tempdir for FS storage,
//! spawns the router on an ephemeral port, then drives the full publish →
//! fetch → list flow over HTTP. Crucially asserts tenant isolation —
//! tenant B must not see tenant A's skills.
//!
//! Requires a working Docker socket; the test fails fast with a clear
//! message otherwise. Run with: `cargo test --test integration`.

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
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

/// Spin up Postgres + a server bound to a random port; return everything
/// the test needs to drive the system.
struct Harness {
    base: String,
    acme_token: String,
    globex_token: String,
    db: sqlx::PgPool,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
    // 1. Postgres
    let pg = Postgres::default().start().await?;
    let pg_port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

    // 2. Migrations
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // 3. Tenants + tokens
    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex Inc", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;
    let globex_token = admin::create_token(&pool, "globex", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    // 4. Server on ephemeral port
    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
    };
    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Tiny settle delay so the listener is accepting before reqwest hits it.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_token,
        globex_token,
        db: pool,
        _pg: pg,
        _storage_dir: storage_dir,
    })
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

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(
    c: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    path: &str,
    tenant: &str,
) -> reqwest::RequestBuilder {
    c.request(method, format!("{base}{path}"))
        .header("x-skill-pool-tenant", tenant)
}

fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

#[tokio::test]
async fn full_publish_install_and_isolation() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // healthz responds (no tenant required, but extractor for skills routes does)
    let healthz: Value = c
        .get(format!("{}/v1/healthz", h.base))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(healthz["status"], "ok");

    // -------- publish to acme --------
    let bundle = build_bundle(
        "---\nname: hello\ndescription: Says hello when the user asks for a greeting.\ntags: [test, greeting]\n---\n\n# hello\n\nGreet the user.\n",
    );
    let form = Form::new()
        .text(
            "metadata",
            r#"{"slug":"hello","version":"1.0.0","tags":["smoke"]}"#,
        )
        .part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name("hello.tar.gz")
                .mime_str("application/gzip")?,
        );
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(
        resp.status().as_u16(),
        201,
        "publish failed: {}",
        resp.text().await?
    );
    let published: Value = resp.json().await?;
    assert_eq!(published["slug"], "hello");
    assert_eq!(published["version"], "1.0.0");
    let tags = published["tags"].as_array().unwrap();
    assert!(tags.iter().any(|t| t == "test"));
    assert!(tags.iter().any(|t| t == "greeting"));
    assert!(tags.iter().any(|t| t == "smoke"));

    // -------- list as acme: present --------
    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        list.iter().any(|s| s["slug"] == "hello"),
        "acme should see hello: {list:?}"
    );

    // -------- list as globex: tenant isolation --------
    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "globex"),
        &h.globex_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(list.is_empty(), "globex should see no skills, got {list:?}");

    // -------- get one as acme --------
    let one: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/hello",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(
        one["description"],
        "Says hello when the user asks for a greeting."
    );

    // -------- get one as globex: 404 --------
    let resp = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/hello",
            "globex",
        ),
        &h.globex_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // -------- download bundle as acme --------
    let bytes = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/hello/bundle.tar.gz",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .bytes()
    .await?;
    assert!(!bytes.is_empty(), "bundle download should be non-empty");
    // Sanity: gzip magic bytes
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "expected gzip magic header");

    // -------- download as globex: 404 --------
    let resp = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/hello/bundle.tar.gz",
            "globex",
        ),
        &h.globex_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // -------- bogus token: 401 --------
    let resp = req(&c, reqwest::Method::POST, &h.base, "/v1/skills", "acme")
        .bearer_auth("spk_definitely_not_a_real_token")
        .multipart(
            Form::new()
                .text("metadata", r#"{"slug":"x","version":"0.0.1"}"#)
                .part("bundle", Part::bytes(vec![0u8]).file_name("x.tar.gz")),
        )
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 401);

    // -------- validate accepts a good bundle --------
    let good = build_bundle("---\nname: ok\ndescription: A good skill that does Y.\n---\nbody\n");
    let form = Form::new().part(
        "bundle",
        Part::bytes(good.to_vec())
            .file_name("ok.tar.gz")
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            "/v1/skills/validate",
            "acme",
        ),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert!(resp.status().is_success());
    let j: Value = resp.json().await?;
    assert_eq!(j["ok"], true);

    // -------- validate rejects a bundle with a secret --------
    let bad =
        build_bundle("---\nname: leaky\ndescription: bad.\n---\n\nAKIAIOSFODNN7EXAMPLE leak\n");
    let form = Form::new().part(
        "bundle",
        Part::bytes(bad.to_vec())
            .file_name("bad.tar.gz")
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            "/v1/skills/validate",
            "acme",
        ),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400);

    // -------- audit row written for the publish --------
    let audit: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_events \
         WHERE action = 'skill.publish' AND target_id = 'hello'",
    )
    .fetch_one(&h.db)
    .await?;
    assert_eq!(audit.0, 1, "expected exactly one audit row for the publish");

    // -------- duplicate publish: 400 --------
    let dup_bundle = build_bundle("---\nname: hello\ndescription: dup attempt.\n---\nbody\n");
    let form = Form::new()
        .text("metadata", r#"{"slug":"hello","version":"1.0.0"}"#)
        .part(
            "bundle",
            Part::bytes(dup_bundle.to_vec())
                .file_name("hello.tar.gz")
                .mime_str("application/gzip")?,
        );
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400);

    Ok(())
}
