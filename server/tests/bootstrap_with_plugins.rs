//! Bootstrap tier-0 plugin expansion (#36).
//!
//! Seeds: tenant `acme` with a published skill `standalone-skill`, a
//! published plugin `bundle-alpha` whose `plugin_contents` references two
//! published items (`skill-a`, `agent-reviewer`), and a project `proj1`
//! whose `tenant_project_items` list is:
//!
//!   1. (kind="skill",  slug="standalone-skill")
//!   2. (kind="plugin", slug="bundle-alpha")
//!
//! Expectations:
//!
//!   - `GET /v1/bootstrap?project=proj1` returns 200.
//!   - `skills: [...]` (the legacy field) lists only kind="skill"
//!     contributors, in BFS order: ["standalone-skill", "skill-a"].
//!   - `project_items: [...]` is the new full-provenance list and
//!     contains all three: standalone (source=direct), skill-a + agent
//!     (source="plugin:bundle-alpha").
//!   - `project: {slug, name}` echoes proj1.
//!
//! Regression coverage for the pre-#36 shape lives in `bootstrap.rs`;
//! those tests don't pin a project so they don't exercise tier 0.

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
    // `skills:publish` lets us seed published skills + plugins; the
    // bootstrap GET only needs an authenticated reader token.
    let acme_token = admin::create_token(&pool, "acme", "publisher", "skills:read skills:publish")
        .await?
        .raw_token;
    // The project routes (PUT /items) are admin-gated. Bootstrap reads
    // the project transparently via `admin::get_project` which doesn't
    // re-check scope, so the publisher token is enough for the GET
    // assertion.
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

fn build_bundle(name: &str) -> Bytes {
    let body = format!(
        "---\nname: {name}\ndescription: Test fixture for {name}\n---\n\n# {name}\n"
    );
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_path("SKILL.md").unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body.as_bytes()).unwrap();
    let tar_bytes = tar.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    Bytes::from(gz.finish().unwrap())
}

async fn publish_skill(
    cl: &reqwest::Client,
    h: &Harness,
    slug: &str,
    kind: Option<&str>,
) -> Result<()> {
    let bundle = build_bundle(slug);
    let mut meta = json!({ "slug": slug, "version": "1.0.0" });
    if let Some(k) = kind {
        meta["kind"] = json!(k);
    }
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(cl, reqwest::Method::POST, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    if status != 201 {
        let body = resp.text().await?;
        anyhow::bail!("publish {slug} ({kind:?}) failed ({status}): {body}");
    }
    Ok(())
}

async fn publish_plugin(
    cl: &reqwest::Client,
    h: &Harness,
    slug: &str,
    contents: Vec<(&str, &str, &str)>, // (kind, slug, version)
) -> Result<()> {
    let body = json!({
        "slug": slug,
        "manifest": {
            "name": slug,
            "version": "1.0.0",
            "description": format!("Test plugin {slug}")
        },
        "contents": contents.iter().map(|(k, s, v)| json!({
            "kind": k, "slug": s, "version": v
        })).collect::<Vec<_>>(),
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
async fn bootstrap_expands_plugin_kind_items_into_resolved_contents() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // 1. Publish the standalone skill (referenced directly by the project)
    //    plus the two skill-kind items the plugin will bundle.
    publish_skill(&cl, &h, "standalone-skill", None).await?;
    publish_skill(&cl, &h, "skill-a", None).await?;
    publish_skill(&cl, &h, "agent-reviewer", Some("agent")).await?;

    // 2. Publish the plugin bundling skill-a + agent-reviewer.
    publish_plugin(
        &cl,
        &h,
        "bundle-alpha",
        vec![
            ("skill", "skill-a", "1.0.0"),
            ("agent", "agent-reviewer", "1.0.0"),
        ],
    )
    .await?;

    // 3. Create the project + add both a direct skill and the plugin to
    //    its item list. The order is significant — the bootstrap response
    //    walks items in position order, then BFS-expands plugins.
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
    .json(&json!({
        "slug": "proj1",
        "name": "Proj One",
        "description": "Bootstrap-with-plugins test fixture"
    }))
    .send()
    .await?;
    assert_eq!(create.status().as_u16(), 201, "{}", create.text().await?);

    let put_items = authed(
        req(
            &cl,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/projects/proj1/items",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!([
        {"slug": "standalone-skill", "kind": "skill"},
        {"slug": "bundle-alpha", "kind": "plugin"}
    ]))
    .send()
    .await?;
    assert_eq!(
        put_items.status().as_u16(),
        204,
        "put items failed: {}",
        put_items.text().await?
    );

    // 4. Bootstrap with the project pin.
    let resp = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/bootstrap?project=proj1",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 200, "{}", resp.text().await?);
    let body: Value = resp.json().await?;

    // 4a. `project` echoes the resolved project metadata.
    assert_eq!(body["project"]["slug"], "proj1");
    assert_eq!(body["project"]["name"], "Proj One");

    // 4b. `skills: Vec<String>` — legacy field, carries ALL tier-0
    //     slugs regardless of kind (the field name predates per-kind
    //     awareness in the CLI), in BFS order: direct items first, then
    //     plugin-bundled in `position` order.
    let skills: Vec<String> = body["skills"]
        .as_array()
        .expect("skills must be array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        skills,
        vec!["standalone-skill", "skill-a", "agent-reviewer"],
        "skills (post-#36 expansion) was: {skills:?}"
    );

    // 4c. `project_items: Vec<{slug, kind, source}>` — new in #36;
    //     carries kind for agents/commands + provenance for debugging.
    let project_items = body["project_items"]
        .as_array()
        .expect("project_items must be present");
    assert_eq!(
        project_items.len(),
        3,
        "expected 3 resolved items: {project_items:#?}"
    );

    let by_slug: std::collections::HashMap<&str, (&str, &str)> = project_items
        .iter()
        .map(|v| {
            (
                v["slug"].as_str().unwrap(),
                (v["kind"].as_str().unwrap(), v["source"].as_str().unwrap()),
            )
        })
        .collect();
    assert_eq!(by_slug.get("standalone-skill"), Some(&("skill", "direct")));
    assert_eq!(
        by_slug.get("skill-a"),
        Some(&("skill", "plugin:bundle-alpha"))
    );
    assert_eq!(
        by_slug.get("agent-reviewer"),
        Some(&("agent", "plugin:bundle-alpha"))
    );

    Ok(())
}
