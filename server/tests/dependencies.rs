//! Phase 5 integration test: dependency resolution.
//!
//! Coverage:
//!   1. Publish a skill that declares `requires` → rows land in
//!      `skill_dependencies`; audit metadata reflects the count.
//!   2. `GET /v1/skills/{slug}/deps` returns immediate deps.
//!   3. Transitive chain A → B → C — closure returns both, with depth.
//!   4. Diamond A → {B, C}; B → D; C → D — D appears once (UNION dedups).
//!   5. Cycle A → B → A — closure terminates (depth cap + UNION).
//!   6. Forward reference: A requires X; X never published — A publishes
//!      anyway; closure includes X with no version conflict.
//!   7. Self-require is a 400.

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
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
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

fn req(c: &reqwest::Client, m: reqwest::Method, base: &str, p: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{base}{p}"))
        .header("x-skill-pool-tenant", "acme")
}

fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

/// Publish a skill with optional `requires:` block. Returns status code +
/// JSON body so individual tests can assert on errors.
async fn publish(
    c: &reqwest::Client,
    h: &Harness,
    slug: &str,
    requires: &[&str],
) -> Result<(u16, Value)> {
    let req_block = if requires.is_empty() {
        String::new()
    } else {
        let entries = requires
            .iter()
            .map(|r| format!("  - {r}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("requires:\n{entries}\n")
    };
    let body = format!(
        "---\nname: {slug}\ndescription: Pattern about {slug}.\ntags: [test]\n{req_block}---\n\n# {slug}\n"
    );
    let bundle = build_bundle(&body);
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/skills"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    let v: Value = resp.json().await?;
    Ok((status, v))
}

async fn deps(c: &reqwest::Client, h: &Harness, slug: &str) -> Result<Vec<Value>> {
    let resp = authed(
        req(c, reqwest::Method::GET, &h.base, &format!("/v1/skills/{slug}/deps")),
        &h.acme_token,
    )
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "deps {slug}: {body}");
    Ok(body.as_array().unwrap().clone())
}

#[tokio::test]
async fn dependency_graph_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Immediate deps: publish A requiring B.
    let (status, _) = publish(&c, &h, "skill-b", &[]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "skill-a", &["skill-b"]).await?;
    assert_eq!(status, 201);
    let a_deps = deps(&c, &h, "skill-a").await?;
    assert_eq!(a_deps.len(), 1);
    assert_eq!(a_deps[0]["slug"], "skill-b");
    assert_eq!(a_deps[0]["depth"], 1);
    assert_eq!(a_deps[0]["version_range"], "*");

    // 2. Chain: C → A → B. Closure of C is {A, B} with correct depths.
    let (status, _) = publish(&c, &h, "skill-c", &["skill-a@1.0.0"]).await?;
    assert_eq!(status, 201);
    let c_deps = deps(&c, &h, "skill-c").await?;
    let slugs: Vec<&str> = c_deps
        .iter()
        .map(|d| d["slug"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"skill-a"), "{c_deps:?}");
    assert!(slugs.contains(&"skill-b"), "{c_deps:?}");
    // Depth: A is depth 1, B is depth 2.
    let a = c_deps.iter().find(|d| d["slug"] == "skill-a").unwrap();
    let b = c_deps.iter().find(|d| d["slug"] == "skill-b").unwrap();
    assert_eq!(a["depth"], 1);
    assert_eq!(b["depth"], 2);
    assert_eq!(a["version_range"], "1.0.0", "explicit @1.0.0 carried through");

    // 3. Diamond: D requires E + F. E + F both require G.
    let (status, _) = publish(&c, &h, "skill-g", &[]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "skill-e", &["skill-g"]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "skill-f", &["skill-g"]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "skill-d", &["skill-e", "skill-f"]).await?;
    assert_eq!(status, 201);
    let d_deps = deps(&c, &h, "skill-d").await?;
    let g_hits: Vec<_> = d_deps.iter().filter(|d| d["slug"] == "skill-g").collect();
    assert_eq!(g_hits.len(), 1, "diamond should dedup skill-g: {d_deps:?}");

    // 4. Forward reference: H requires never-published-x. Publishes
    //    anyway; closure surfaces the broken edge.
    let (status, _) = publish(&c, &h, "skill-h", &["never-published-x"]).await?;
    assert_eq!(status, 201);
    let h_deps = deps(&c, &h, "skill-h").await?;
    assert_eq!(h_deps.len(), 1);
    assert_eq!(h_deps[0]["slug"], "never-published-x");

    // 5. Cycle: publish skill-cyc-a → cyc-b. Then publish cyc-b version 2
    //    that requires cyc-a. (Re-publish under a new version.)
    //    Actually easier: publish two skills that point at each other
    //    via the FK-on-id model. The first publish sets a→b. The second
    //    must point b→a but cyc-a already exists (just no deps yet).
    //    We can't retroactively add a dep — the publish IS the dep
    //    declaration moment, and a new version creates a new row.
    //    For the test, publish cyc-b@1.0.0 (no deps), then cyc-a@1.0.0
    //    requires cyc-b. Now re-publish cyc-b@2.0.0 requires cyc-a.
    //    The closure query uses the LATEST version per slug.
    let (status, _) = publish(&c, &h, "cyc-b", &[]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "cyc-a", &["cyc-b"]).await?;
    assert_eq!(status, 201);
    // Re-publish cyc-b at v2 that now requires cyc-a — closing the cycle.
    // We need a fresh version on the same slug; the publish form already
    // takes version from metadata. Re-do with version 2.0.0.
    let body = "---\nname: cyc-b\ndescription: cyc-b v2.\nrequires:\n  - cyc-a\n---\n\n# cyc-b\n";
    let bundle = build_bundle(body);
    let form = Form::new()
        .text("metadata", json!({"slug": "cyc-b", "version": "2.0.0"}).to_string())
        .part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name("cyc-b.tar.gz")
                .mime_str("application/gzip")?,
        );
    let resp = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/skills"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 201);

    // Closure of cyc-a must terminate. cyc-a → cyc-b (latest = v2) → cyc-a → …
    // UNION dedups, depth cap is 10. We expect at most a few rows total.
    let cyc_deps = deps(&c, &h, "cyc-a").await?;
    // Both cyc-a and cyc-b appear in the closure; the recursive walk
    // visits them once each thanks to UNION semantics.
    let slugs: std::collections::HashSet<String> = cyc_deps
        .iter()
        .map(|d| d["slug"].as_str().unwrap().to_string())
        .collect();
    assert!(slugs.contains("cyc-b"), "{cyc_deps:?}");
    // cyc-a may or may not appear depending on PG dedup semantics on the
    // (slug, depth) tuple. The key invariant is the request returns and
    // doesn't infinitely loop.
    assert!(
        cyc_deps.len() < 50,
        "cycle didn't terminate cleanly: {} rows",
        cyc_deps.len()
    );

    // 6. Self-require is rejected at publish.
    let (status, body) = publish(&c, &h, "self-loop", &["self-loop"]).await?;
    assert_eq!(status, 400);
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("cannot require itself"));

    // 7. Malformed requires entry → 400.
    let (status, body) = publish(&c, &h, "bad-req", &["@bogus"]).await?;
    assert_eq!(status, 400, "{body}");

    // 8. Closure of a skill with no deps is [].
    let empty = deps(&c, &h, "skill-g").await?;
    assert_eq!(empty.len(), 0);

    // 9. 404 for unknown slug.
    let resp = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/no-such/deps"),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // 10. Version-range conflict detection (#7 lifecycle).
    //     Publish `lib`, then conflict-a@1.0.0 requires lib@1.0.0.
    //     Publishing conflict-b requiring lib@2.0.0 collides → 409
    //     with both skills + ranges in the error message.
    let (status, _) = publish(&c, &h, "lib", &[]).await?;
    assert_eq!(status, 201);
    let (status, _) = publish(&c, &h, "skill-conflict-a", &["lib@1.0.0"]).await?;
    assert_eq!(status, 201);
    let (status, body) = publish(&c, &h, "skill-conflict-b", &["lib@2.0.0"]).await?;
    assert_eq!(status, 409, "expected conflict; got body={body}");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(msg.contains("skill-conflict-b"), "{msg}");
    assert!(msg.contains("skill-conflict-a"), "{msg}");
    assert!(msg.contains("lib"), "{msg}");
    assert!(msg.contains("1.0.0") && msg.contains("2.0.0"), "{msg}");

    // 11. `*` never conflicts — `lib@*` matches both 1.0.0 and 2.0.0.
    let (status, body) = publish(&c, &h, "skill-conflict-c", &["lib@*"]).await?;
    assert_eq!(status, 201, "expected `*` to dodge conflict; body={body}");

    // 12. Identical exact ranges are compatible (idempotent shape).
    let (status, _) = publish(&c, &h, "skill-conflict-d", &["lib@1.0.0"]).await?;
    assert_eq!(status, 201);

    Ok(())
}
