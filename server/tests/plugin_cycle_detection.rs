//! Plugin → plugin cycle detection in the bootstrap resolver (#36).
//!
//! Plugin A's manifest declares `plugins: [{"slug": "b"}]`; plugin B's
//! manifest declares `plugins: [{"slug": "a"}]`. A project pins plugin A
//! as a `kind="plugin"` item; bootstrap walks A → B → A and must reject
//! with a 422 + `{"error":"plugin_cycle","cycle":[...]}` whose cycle path
//! is normalised (smallest slug leads).
//!
//! The cycle lives in the loose `manifest.plugins[]` JSON passthrough —
//! the publish handler doesn't validate the nested-plugin slugs (they
//! may legitimately forward-reference plugins published later), so we
//! don't need any tricks to seed the cycle.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

struct Harness {
    base: String,
    acme_token: String,
    acme_admin_token: String,
    _pool: sqlx::PgPool,
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

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "publisher", "skills:read skills:publish")
        .await?
        .raw_token;
    let acme_admin_token =
        admin::create_token(&pool, "acme", "admin", "tenant:admin skills:read skills:publish")
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
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_token,
        acme_admin_token,
        _pool: pool,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn c() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(
    cl: &reqwest::Client,
    m: reqwest::Method,
    base: &str,
    p: &str,
    tenant: &str,
) -> reqwest::RequestBuilder {
    cl.request(m, format!("{base}{p}"))
        .header("x-skill-pool-tenant", tenant)
}

fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

async fn publish_plugin_with_nested(
    cl: &reqwest::Client,
    h: &Harness,
    slug: &str,
    nested_slugs: &[&str],
) -> Result<()> {
    let body = json!({
        "slug": slug,
        "manifest": {
            "name": slug,
            "version": "1.0.0",
            "description": format!("Cycle-detection fixture {slug}"),
            // Loose passthrough — the publish handler stores `extra`
            // fields verbatim (#30). The bootstrap-tier resolver reads
            // `manifest.plugins[]` to enqueue transitive plugins.
            "plugins": nested_slugs.iter().map(|s| json!({"slug": *s})).collect::<Vec<_>>()
        },
        "contents": [],
        "sourcing_mode": "internal",
        "status": "published"
    });
    let resp = authed(
        req(cl, reqwest::Method::POST, &h.base, "/v1/plugins", "acme"),
        &h.acme_token,
    )
    .json(&body)
    .send()
    .await?;
    let status = resp.status().as_u16();
    if status != 201 {
        let txt = resp.text().await?;
        anyhow::bail!("publish plugin {slug} failed ({status}): {txt}");
    }
    Ok(())
}

#[tokio::test]
async fn bootstrap_returns_422_plugin_cycle_when_project_pins_cycle_root() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Seed the cycle: a → b → a. Both plugins are content-less; the
    // cycle lives entirely in the loose `manifest.plugins[]` field.
    publish_plugin_with_nested(&cl, &h, "a", &["b"]).await?;
    publish_plugin_with_nested(&cl, &h, "b", &["a"]).await?;

    // Create a project whose only item is plugin `a` — bootstrap will
    // BFS-walk into the cycle when it tries to expand the item list.
    let create = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({"slug": "cycle-proj", "name": "Cycle Proj"}))
    .send()
    .await?;
    assert_eq!(create.status().as_u16(), 201, "{}", create.text().await?);

    let put_items = authed(
        req(
            &cl,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/projects/cycle-proj/items",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!([{"slug": "a", "kind": "plugin"}]))
    .send()
    .await?;
    assert_eq!(
        put_items.status().as_u16(),
        204,
        "put items: {}",
        put_items.text().await?
    );

    let resp = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/bootstrap?project=cycle-proj",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?;

    // 422 — cycles are a manifest-correctness error, not a 500 or 400.
    // The dedicated status + JSON shape lets the web admin render a
    // specific "this project pulls in a plugin cycle" message.
    assert_eq!(
        resp.status().as_u16(),
        422,
        "expected 422 plugin_cycle, got {} — body: {}",
        resp.status().as_u16(),
        resp.text().await?
    );
    let body: Value = resp.json().await?;

    assert_eq!(body["error"], "plugin_cycle", "{body:#}");
    assert_eq!(
        body["message"], "plugin dependency cycle detected",
        "{body:#}"
    );

    // Cycle path is normalised: smallest slug leads, repeats at end.
    // a < b, so the expected reading is `["a", "b", "a"]`.
    let cycle: Vec<String> = body["cycle"]
        .as_array()
        .expect("cycle must be array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(cycle, vec!["a", "b", "a"], "cycle was: {cycle:?}");

    Ok(())
}
