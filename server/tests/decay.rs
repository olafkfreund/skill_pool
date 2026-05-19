//! Phase 5 integration test: decay/graveyard lifecycle.
//!
//! 1. Publish a skill → it's NOT a decay candidate (just created).
//! 2. Download the bundle → `use_count` bumps, `last_used_at` updates.
//! 3. Backdate `last_used_at` directly via SQL to simulate a stale
//!    skill → it now appears in the decay list.
//! 4. Archive endpoint flips status → list endpoint stops returning it
//!    AND decay list stops returning it.

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
    acme_admin_token: String,
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
    let acme_admin_token = admin::create_token(
        &pool,
        "acme",
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
        acme_admin_token,
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

async fn publish(h: &Harness, c: &reqwest::Client, slug: &str) -> Result<()> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: Pattern about {slug}.\ntags: [test]\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/skills", "acme"),
        &h.acme_admin_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 201, "{}", resp.text().await?);
    Ok(())
}

#[tokio::test]
async fn decay_lifecycle_end_to_end() -> Result<()> {
    let h = boot().await?;
    let c = client();

    publish(&h, &c, "old-skill").await?;
    publish(&h, &c, "active-skill").await?;

    // 1. Fresh publish → not a decay candidate (last_used_at = now()).
    let list: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/skills/decay?days=1",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(list.is_empty(), "fresh publishes shouldn't decay: {list:?}");

    // 2. Download active-skill's bundle → use_count bumps.
    for _ in 0..3 {
        let resp = authed(
            req(
                &c,
                reqwest::Method::GET,
                &h.base,
                "/v1/skills/active-skill/bundle.tar.gz",
                "acme",
            ),
            &h.acme_admin_token,
        )
        .send()
        .await?;
        assert_eq!(resp.status().as_u16(), 200);
    }
    let (active_count,): (i32,) = sqlx::query_as(
        "SELECT use_count FROM skills WHERE slug = 'active-skill'",
    )
    .fetch_one(&h.db)
    .await?;
    assert_eq!(active_count, 3, "use_count should bump on bundle download");

    // 3. Backdate old-skill's last_used_at to 200 days ago. Simulates a
    //    skill nobody has touched in months.
    sqlx::query(
        "UPDATE skills SET last_used_at = now() - INTERVAL '200 days' \
         WHERE slug = 'old-skill'",
    )
    .execute(&h.db)
    .await?;

    let candidates: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/skills/decay?days=180&max_uses=3",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0]["slug"], "old-skill");
    assert_eq!(candidates[0]["use_count"], 0);

    // active-skill has 3 uses, so even if we backdate it, max_uses=3
    // means it's NOT a candidate (use_count < 3 is the filter).
    sqlx::query(
        "UPDATE skills SET last_used_at = now() - INTERVAL '300 days' \
         WHERE slug = 'active-skill'",
    )
    .execute(&h.db)
    .await?;
    let candidates: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/skills/decay?days=180&max_uses=3",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(candidates.len(), 1, "active-skill must not decay: {candidates:?}");

    // 4. Archive old-skill → it disappears from both the catalog list
    //    AND the decay candidates.
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            "/v1/skills/old-skill/archive",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let archived: Value = resp.json().await?;
    assert_eq!(archived["slug"], "old-skill");
    assert_eq!(archived["version"], "1.0.0");

    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "acme"),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        list.iter().all(|s| s["slug"] != "old-skill"),
        "archived skill must vanish from catalog: {list:?}"
    );

    let candidates: Vec<Value> = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/skills/decay?days=180&max_uses=3",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        candidates.iter().all(|c| c["slug"] != "old-skill"),
        "archived skill must vanish from decay list: {candidates:?}"
    );

    // 5. Archiving a non-existent slug → 404.
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            "/v1/skills/never-existed/archive",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}

/// Background sweep: a stale, low-usage skill gets flipped to
/// `archive_candidate` so curators see it flagged proactively (#7
/// lifecycle). We exercise the extracted `decay::sweep` directly so
/// the test doesn't have to wait out the periodic interval.
#[tokio::test]
async fn decay_sweep_flips_archive_candidate() -> anyhow::Result<()> {
    use skill_pool_server::routes::decay::{
        sweep, DEFAULT_SWEEP_MIN_USES, DEFAULT_SWEEP_STALE_DAYS,
    };

    let h = boot().await?;
    let c = client();

    publish(&h, &c, "stale-skill").await?;
    publish(&h, &c, "active-skill").await?;

    // Make active-skill safe by bumping its use_count above the threshold.
    sqlx::query("UPDATE skills SET use_count = $1 WHERE slug = 'active-skill'")
        .bind(DEFAULT_SWEEP_MIN_USES + 1)
        .execute(&h.db)
        .await?;

    // Backdate stale-skill to 200 days; use_count stays 0.
    sqlx::query(
        "UPDATE skills SET last_used_at = now() - INTERVAL '200 days' \
         WHERE slug = 'stale-skill'",
    )
    .execute(&h.db)
    .await?;

    let flipped = sweep(&h.db, DEFAULT_SWEEP_STALE_DAYS, DEFAULT_SWEEP_MIN_USES).await?;
    assert!(flipped >= 1, "expected at least one row flipped, got {flipped}");

    // Verify the stale skill's status.
    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM skills WHERE slug = 'stale-skill'")
            .fetch_one(&h.db)
            .await?;
    assert_eq!(status, "archive_candidate");

    // active-skill stays published (use_count beats the threshold).
    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM skills WHERE slug = 'active-skill'")
            .fetch_one(&h.db)
            .await?;
    assert_eq!(status, "published");

    // Catalog list filters published → flagged skill drops out.
    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "acme"),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        list.iter().all(|s| s["slug"] != "stale-skill"),
        "archive_candidate must hide from catalog: {list:?}"
    );

    // Idempotency: running the sweep again finds no published rows in
    // the stale window, so it flips zero.
    let again = sweep(&h.db, DEFAULT_SWEEP_STALE_DAYS, DEFAULT_SWEEP_MIN_USES).await?;
    assert_eq!(again, 0, "second sweep should be a no-op");

    Ok(())
}
