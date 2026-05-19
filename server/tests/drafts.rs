//! Phase 4 integration test: drafts inbox round-trip.
//!
//! 1. POST a draft as acme → 201, draft appears in inbox.
//! 2. GET /v1/drafts/{id}/skill-md returns the embedded SKILL.md.
//! 3. Tenant isolation: globex sees an empty inbox.
//! 4. POST /v1/drafts/{id}/publish → promotes to skills, draft flips to `published`.
//! 5. Same publish call a second time → 400 (already published).
//! 6. POST /v1/drafts/{id}/discard on a separate pending draft → 204; inbox
//!    no longer surfaces it under `status=pending`.

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
    acme_token: String,
    globex_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
    // pgvector/pgvector ships pgvector pre-installed; we need it for the
    // 0009 embeddings migration. Strict superset of postgres:11-alpine.
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
    };
    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_token,
        globex_token,
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

async fn create_draft(
    c: &reqwest::Client,
    h: &Harness,
    slug: &str,
    notes: Option<&str>,
) -> Result<Value> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: A captured insight from a real session.\ntags: [captured]\n---\n\n# {slug}\n\nbody.\n",
    ));
    let meta = json!({
        "slug": slug,
        "origin": "cli",
        "notes": notes,
        "tags": ["from-cli"],
    });
    let form = Form::new()
        .text("metadata", meta.to_string())
        .part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name(format!("{slug}.tar.gz"))
                .mime_str("application/gzip")?,
        );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/drafts", "acme"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 201, "draft create failed: {body}");
    Ok(body)
}

#[tokio::test]
async fn draft_inbox_publish_and_discard_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Create a draft as acme.
    let draft_a = create_draft(&c, &h, "axum-handler-tip", Some("from oncall PR")).await?;
    assert_eq!(draft_a["slug"], "axum-handler-tip");
    assert_eq!(draft_a["status"], "pending");
    assert_eq!(draft_a["origin"], "cli");
    let tags = draft_a["tags"].as_array().unwrap();
    assert!(tags.iter().any(|t| t == "captured"));
    assert!(tags.iter().any(|t| t == "from-cli"));
    let draft_a_id = draft_a["id"].as_str().unwrap().to_string();

    // 2. Inbox lists it under pending.
    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/drafts", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(list.len(), 1, "acme inbox: {list:?}");
    assert_eq!(list[0]["id"], draft_a_id);

    // 3. Render SKILL.md.
    let md = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            &format!("/v1/drafts/{draft_a_id}/skill-md"),
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .text()
    .await?;
    assert!(md.contains("A captured insight"), "SKILL.md = {md}");
    assert!(md.contains("# axum-handler-tip"));

    // 4. Tenant isolation — globex sees nothing.
    let globex_list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/drafts", "globex"),
        &h.globex_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        globex_list.is_empty(),
        "globex inbox should be empty, got {globex_list:?}"
    );

    // 5. Publish the draft.
    let publish_resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            &format!("/v1/drafts/{draft_a_id}/publish"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "version": "1.0.0" }))
    .send()
    .await?;
    let publish_status = publish_resp.status().as_u16();
    let publish_body: Value = publish_resp.json().await?;
    assert_eq!(publish_status, 200, "publish failed: {publish_body}");
    assert_eq!(publish_body["slug"], "axum-handler-tip");
    assert_eq!(publish_body["version"], "1.0.0");
    assert!(publish_body["skill_id"].is_string());

    // 6. Draft now reports status=published.
    let one: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            &format!("/v1/drafts/{draft_a_id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(one["status"], "published");
    assert_eq!(one["published_version"], "1.0.0");

    // 7. /v1/skills surfaces the promoted skill.
    let skills: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        skills.iter().any(|s| s["slug"] == "axum-handler-tip"),
        "promoted skill missing from /v1/skills: {skills:?}"
    );

    // 8. Re-publishing the same draft 400s.
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            &format!("/v1/drafts/{draft_a_id}/publish"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "version": "1.0.1" }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400, "{}", resp.text().await?);

    // 9. Create a second draft and discard it.
    let draft_b = create_draft(&c, &h, "tossable-tip", None).await?;
    let draft_b_id = draft_b["id"].as_str().unwrap().to_string();

    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            &format!("/v1/drafts/{draft_b_id}/discard"),
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 204);

    // 10. Pending inbox no longer shows it.
    let list: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/drafts?status=pending",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        !list.iter().any(|d| d["id"] == draft_b_id),
        "discarded draft still pending: {list:?}"
    );

    // 11. status=discarded surfaces it.
    let list: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/drafts?status=discarded",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        list.iter().any(|d| d["id"] == draft_b_id),
        "discarded inbox missing: {list:?}"
    );

    Ok(())
}
