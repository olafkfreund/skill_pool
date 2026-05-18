//! Phase 5 integration test: MCP transport.
//!
//! Drives the full JSON-RPC handshake against a live server:
//!  1. initialize → server-info + tools capability
//!  2. tools/list → both tools advertised
//!  3. tools/call search_skills → text + JSON content
//!  4. tools/call get_skill → SKILL.md content
//!  5. tools/call get_skill (missing slug) → isError:true tool result
//!  6. unknown method → JSON-RPC -32601
//!  7. unauthenticated request → 401

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
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
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

async fn publish(h: &Harness, c: &reqwest::Client, slug: &str, body: &str) -> Result<()> {
    let bundle = build_bundle(body);
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = c
        .post(format!("{}/v1/skills", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .multipart(form)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 201, "{}", resp.text().await?);
    Ok(())
}

async fn rpc(h: &Harness, c: &reqwest::Client, body: Value) -> Result<Value> {
    let resp = c
        .post(format!("{}/v1/mcp", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .json(&body)
        .send()
        .await?;
    let status = resp.status().as_u16();
    let payload: Value = resp.json().await?;
    assert_eq!(status, 200, "{payload}");
    Ok(payload)
}

#[tokio::test]
async fn mcp_protocol_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Seed the catalog with one skill.
    publish(
        &h,
        &c,
        "axum-handler",
        "---\nname: axum-handler\ndescription: Pattern for axum extractors.\ntags: [rust]\n---\n\n# axum-handler\n\nBody.\n",
    )
    .await?;

    // 1. initialize
    let init = rpc(
        &h,
        &c,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
    )
    .await?;
    assert_eq!(init["id"], 1);
    let server_info = &init["result"]["serverInfo"];
    assert_eq!(server_info["name"], "skill-pool");
    assert!(init["result"]["protocolVersion"].is_string());
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // 2. tools/list
    let list = rpc(
        &h,
        &c,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await?;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search_skills"));
    assert!(names.contains(&"get_skill"));

    // 3. tools/call search_skills
    let search = rpc(
        &h,
        &c,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "search_skills",
                "arguments": { "query": "axum", "limit": 5 }
            }
        }),
    )
    .await?;
    let content = search["result"]["content"].as_array().unwrap();
    assert!(content.iter().any(|c| {
        c["type"] == "text" && c["text"].as_str().unwrap_or("").contains("axum-handler")
    }));
    assert_eq!(search["result"]["isError"], false);

    // 4. tools/call get_skill
    let got = rpc(
        &h,
        &c,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "get_skill",
                "arguments": { "slug": "axum-handler" }
            }
        }),
    )
    .await?;
    let text = got["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("name: axum-handler"));
    assert!(text.contains("# axum-handler"));
    assert_eq!(got["result"]["isError"], false);

    // 5. tools/call get_skill on a missing slug — tool error, not RPC error.
    let missing = rpc(
        &h,
        &c,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "get_skill",
                "arguments": { "slug": "never-existed" }
            }
        }),
    )
    .await?;
    assert!(missing.get("error").is_none(), "should be tool error, got JSON-RPC error: {missing}");
    assert_eq!(missing["result"]["isError"], true);
    let msg = missing["result"]["content"][0]["text"].as_str().unwrap();
    assert!(msg.contains("never-existed"), "{msg}");

    // 6. Unknown method → JSON-RPC -32601
    let unknown = rpc(
        &h,
        &c,
        json!({"jsonrpc": "2.0", "id": 6, "method": "no/such/method"}),
    )
    .await?;
    assert_eq!(unknown["error"]["code"], -32601);

    // 7. Unauthenticated request → 401
    let resp = c
        .post(format!("{}/v1/mcp", h.base))
        .header("x-skill-pool-tenant", "acme")
        .json(&json!({"jsonrpc": "2.0", "id": 7, "method": "initialize"}))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 401, "{}", resp.text().await?);

    Ok(())
}
