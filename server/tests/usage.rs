//! Phase 5 integration test: usage telemetry.
//!
//! Covers:
//!   1. Publish + download → event row appears, timeline reflects it,
//!      top endpoint lists the skill.
//!   2. View (GET skill-md) records as `view` not `download`.
//!   3. Multi-day backdating via raw SQL — timeline correctly buckets
//!      per day with zero-filled gaps.
//!   4. Tenant isolation — globex's downloads never appear in acme's
//!      timeline or top results.
//!   5. Non-admin caller → 403.

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
    acme_admin: String,
    acme_reader: String,
    globex_admin: String,
    db: sqlx::PgPool,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
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

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex", "team").await?;
    let acme_admin = admin::create_token(
        &pool,
        "acme",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
    .await?
    .raw_token;
    let acme_reader = admin::create_token(&pool, "acme", "reader", "skills:read")
        .await?
        .raw_token;
    let globex_admin = admin::create_token(
        &pool,
        "globex",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
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
        queue_enabled: None,
        decay_check_interval_secs: 0,
        git_repo_path: None,
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
        acme_admin,
        acme_reader,
        globex_admin,
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

fn req(c: &reqwest::Client, m: reqwest::Method, base: &str, p: &str, t: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{base}{p}")).header("x-skill-pool-tenant", t)
}
fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

async fn publish(c: &reqwest::Client, h: &Harness, tenant: &str, token: &str, slug: &str) -> Result<()> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: Pattern about {slug}.\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(req(c, reqwest::Method::POST, &h.base, "/v1/skills", tenant), token)
        .multipart(form)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 201, "{}", resp.text().await?);
    Ok(())
}

#[tokio::test]
async fn usage_telemetry_end_to_end() -> Result<()> {
    let h = boot().await?;
    let c = client();

    publish(&c, &h, "acme", &h.acme_admin, "alpha").await?;
    publish(&c, &h, "acme", &h.acme_admin, "beta").await?;
    publish(&c, &h, "globex", &h.globex_admin, "gamma").await?;

    // 1. Download alpha twice + view it once.
    for _ in 0..2 {
        let r = authed(
            req(&c, reqwest::Method::GET, &h.base, "/v1/skills/alpha/bundle.tar.gz", "acme"),
            &h.acme_admin,
        ).send().await?;
        assert_eq!(r.status().as_u16(), 200);
    }
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/alpha/skill-md", "acme"),
        &h.acme_admin,
    ).send().await?;
    assert_eq!(r.status().as_u16(), 200);
    // Beta downloaded once.
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/beta/bundle.tar.gz", "acme"),
        &h.acme_admin,
    ).send().await?;
    assert_eq!(r.status().as_u16(), 200);
    // Globex downloads gamma — must NOT leak into acme telemetry.
    for _ in 0..5 {
        let r = authed(
            req(&c, reqwest::Method::GET, &h.base, "/v1/skills/gamma/bundle.tar.gz", "globex"),
            &h.globex_admin,
        ).send().await?;
        assert_eq!(r.status().as_u16(), 200);
    }

    // 2. The events table reflects what happened.
    let (acme_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skill_usage_events e \
         JOIN tenants t ON t.id = e.tenant_id WHERE t.slug = 'acme'"
    ).fetch_one(&h.db).await?;
    // 2 alpha downloads + 1 alpha view + 1 beta download = 4 events.
    assert_eq!(acme_count, 4, "expected 4 acme events, got {acme_count}");

    // 3. Top endpoint surfaces alpha first (3 events), then beta (1).
    let top: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/usage/top?days=7&limit=10", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(top[0]["slug"], "alpha", "{top:?}");
    assert_eq!(top[0]["downloads"], 2);
    assert_eq!(top[0]["views"], 1);
    assert_eq!(top[0]["total"], 3);
    assert_eq!(top[1]["slug"], "beta");
    assert_eq!(top[1]["total"], 1);
    // Gamma (globex) must NOT appear.
    assert!(top.iter().all(|r| r["slug"] != "gamma"), "tenant leak: {top:?}");

    // 4. Timeline reflects today's totals + zero-fills past days.
    let timeline: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/usage/timeline?days=7", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(timeline.len(), 7, "expected 7 daily buckets: {timeline:?}");
    let today = timeline.last().unwrap();
    assert_eq!(today["downloads"], 3);
    assert_eq!(today["views"], 1);
    assert_eq!(today["unique_skills"], 2);
    // Yesterday should be all zeros.
    let yesterday = &timeline[timeline.len() - 2];
    assert_eq!(yesterday["downloads"], 0);
    assert_eq!(yesterday["views"], 0);
    assert_eq!(yesterday["unique_skills"], 0);

    // 5. Backdate an event by 3 days — verify it lands in the right bucket.
    sqlx::query(
        "UPDATE skill_usage_events SET ts = now() - INTERVAL '3 days' \
         WHERE id = (SELECT id FROM skill_usage_events \
                     WHERE tenant_id = (SELECT id FROM tenants WHERE slug = 'acme') \
                     AND event_kind = 'view' \
                     LIMIT 1)"
    ).execute(&h.db).await?;
    let timeline: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/usage/timeline?days=7", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    // The 4th-from-end bucket (3 days ago) should now have views=1.
    let three_days_ago = &timeline[timeline.len() - 4];
    assert_eq!(three_days_ago["views"], 1, "{timeline:?}");
    // And today's views dropped to 0.
    let today = timeline.last().unwrap();
    assert_eq!(today["views"], 0);

    // 6. Non-admin caller → 403.
    let resp = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/usage/timeline", "acme"),
        &h.acme_reader,
    ).send().await?;
    assert_eq!(resp.status().as_u16(), 403);

    // 7. CLI-driven `POST /v1/usage` lands a `view` event in the same
    //    table as server-side download/view records. The reader token
    //    has `skills:read` → can post (no admin scope required).
    let before: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skill_usage_events e \
         JOIN tenants t ON t.id = e.tenant_id \
         WHERE t.slug = 'acme' AND e.event_kind = 'view'",
    ).fetch_one(&h.db).await?;
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/usage", "acme"),
        &h.acme_reader,
    )
    .json(&serde_json::json!({
        "skill_id": "alpha",
        "kind": "skill",
        "event": "view",
        "project_hash": "deadbeefcafebabe",
    }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 202, "{}", resp.text().await?);
    let after: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skill_usage_events e \
         JOIN tenants t ON t.id = e.tenant_id \
         WHERE t.slug = 'acme' AND e.event_kind = 'view'",
    ).fetch_one(&h.db).await?;
    assert_eq!(after.0, before.0 + 1, "CLI usage POST must add one view row");

    // 8. Unknown slug → 404 so a stale manifest entry surfaces.
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/usage", "acme"),
        &h.acme_reader,
    )
    .json(&serde_json::json!({
        "skill_id": "never-existed",
        "kind": "skill",
        "event": "view",
    }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // 9. Bad event kind → 400 (CHECK constraint can't be bypassed via API).
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/usage", "acme"),
        &h.acme_reader,
    )
    .json(&serde_json::json!({
        "skill_id": "alpha",
        "event": "click",
    }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400);

    Ok(())
}
