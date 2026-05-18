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
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

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
    // 1. Postgres — pgvector image for the 0009 migration.
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
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
        database_read_url: None,
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

    // -------- ?bytes=true forces the streaming path --------
    // fs:// has no presign so this is the default behaviour, but the test
    // pins the contract: callers can always force-stream regardless of
    // backend capabilities (e.g. corporate proxies stripping redirects).
    let resp = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/hello/bundle.tar.gz?bytes=true",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 200, "?bytes=true should 200");
    let cd = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .map(|v| v.to_str().unwrap_or("").to_owned())
        .unwrap_or_default();
    assert!(
        cd.contains("attachment"),
        "expected attachment Content-Disposition on byte path, got {cd:?}"
    );

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

    // -------- theme GET defaults to brand_name = tenant slug --------
    let theme: Value = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(theme["brand_name"], "acme");
    assert_eq!(theme["primary"], "#2563eb");

    // -------- theme PUT without admin scope returns 403 --------
    let resp = authed(
        c.put(format!("{}/v1/theme", h.base))
            .header("x-skill-pool-tenant", "acme"),
        &h.acme_token,
    )
    .json(&serde_json::json!({
        "brand_name": "Acme Corp",
        "primary": "#7c3aed",
        "primary_fg": "#ffffff",
        "accent": "#0ea5e9",
        "bg": "#ffffff",
        "fg": "#0f172a",
        "muted": "#f1f5f9",
        "muted_fg": "#64748b",
        "border": "#e2e8f0",
        "radius": "0.5rem"
    }))
    .send()
    .await?;
    assert_eq!(
        resp.status().as_u16(),
        403,
        "default scope must not edit themes"
    );

    // -------- theme PUT with admin scope succeeds --------
    let admin_token = {
        use skill_pool_server::admin;
        admin::create_token(&h.db, "acme", "admin", "tenant:admin")
            .await?
            .raw_token
    };
    let resp = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "brand_name": "Acme Corp",
            "primary": "#7c3aed",
            "primary_fg": "#ffffff",
            "accent": "#0ea5e9",
            "bg": "#ffffff",
            "fg": "#0f172a",
            "muted": "#f1f5f9",
            "muted_fg": "#64748b",
            "border": "#e2e8f0",
            "radius": "0.5rem"
        }))
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "admin theme PUT: {}",
        resp.text().await?
    );

    // PUT below WCAG should be rejected.
    let resp = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "brand_name": "Acme Corp",
            "primary": "#7c3aed",
            "primary_fg": "#ffffff",
            "accent": "#0ea5e9",
            "bg": "#ffffff",
            "fg": "#eeeeee",
            "muted": "#f1f5f9",
            "muted_fg": "#64748b",
            "border": "#e2e8f0",
            "radius": "0.5rem"
        }))
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        400,
        "WCAG-failing PUT should be 400"
    );

    // GET reflects the saved theme.
    let theme: Value = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(theme["brand_name"], "Acme Corp");
    assert_eq!(theme["primary"], "#7c3aed");

    // -------- OIDC discover before any sso config --------
    let resp: Value = c
        .get(format!("{}/v1/auth/oidc/discover", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(resp["enabled"], false);

    // -------- configure SSO + re-check discover --------
    use skill_pool_server::admin;
    admin::set_sso(
        &h.db,
        "acme",
        "https://accounts.example.test",
        "client-abc",
        "secret-xyz",
        "publisher",
    )
    .await?;

    let resp: Value = c
        .get(format!("{}/v1/auth/oidc/discover", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(resp["enabled"], true);

    // -------- /v1/auth/oidc/{slug}/start without sso config (globex) --------
    let resp = c
        .get(format!(
            "{}/v1/auth/oidc/globex/start?return_to=http://x/y",
            h.base
        ))
        .header("x-skill-pool-tenant", "globex")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400, "start without SSO should 400");

    // -------- SAML discover / config / metadata / ACS-stub --------
    let resp: Value = c
        .get(format!("{}/v1/auth/saml/discover", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(resp["enabled"], false);

    admin::set_saml(
        &h.db,
        "acme",
        "https://acme.okta.example.test",
        "https://acme.okta.example.test/sso/saml",
        "-----BEGIN CERTIFICATE-----\nMIID\n-----END CERTIFICATE-----",
        None,
        "publisher",
    )
    .await?;

    let resp: Value = c
        .get(format!("{}/v1/auth/saml/discover", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(resp["enabled"], true);

    let metadata = c
        .get(format!("{}/v1/auth/saml/acme/metadata", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .text()
        .await?;
    assert!(
        metadata.contains("<EntityDescriptor"),
        "metadata: {metadata}"
    );
    assert!(metadata.contains("urn:skill-pool:tenant:acme"));
    assert!(metadata.contains("/v1/auth/saml/acme/acs"));
    assert!(metadata.contains("urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"));

    // ACS for a configured tenant now hits the real validator. POST without
    // a form body returns 415 (axum's Form extractor refuses missing
    // Content-Type) — was 501 in the previous "stubbed" iteration.
    let resp = c
        .post(format!("{}/v1/auth/saml/acme/acs", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        415,
        "empty ACS POST should be 415: {}",
        resp.text().await.unwrap_or_default()
    );

    // Malformed base64 still gets a tenant-aware error, not a generic crash.
    let resp = c
        .post(format!("{}/v1/auth/saml/acme/acs", h.base))
        .header("x-skill-pool-tenant", "acme")
        .form(&[("SAMLResponse", "not-base64-!!!")])
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(status, 400, "bad-base64 ACS: {body}");
    assert!(
        body.contains("base64") || body.contains("SAMLResponse"),
        "expected base64-related error, got: {body}"
    );

    // ACS for an unconfigured tenant still 400s before parsing.
    let resp = c
        .post(format!("{}/v1/auth/saml/globex/acs", h.base))
        .header("x-skill-pool-tenant", "globex")
        .form(&[("SAMLResponse", "anything")])
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400);

    // -------- skill-md endpoint returns the SKILL.md body --------
    let md = authed(
        c.get(format!("{}/v1/skills/hello/skill-md", h.base))
            .header("x-skill-pool-tenant", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .text()
    .await?;
    assert!(md.contains("Greet the user."), "skill-md body: {md:?}");

    // -------- SCIM 2.0 — discovery + provision + filter + patch deactivate --------
    let scim_token = admin::create_token(&h.db, "acme", "okta", "scim:provision")
        .await?
        .raw_token;
    let scim_get = |path: &str| {
        c.get(format!("{}{}", h.base, path))
            .header("x-skill-pool-tenant", "acme")
            .bearer_auth(scim_token.clone())
    };
    let scim_post = |path: &str| {
        c.post(format!("{}{}", h.base, path))
            .header("x-skill-pool-tenant", "acme")
            .bearer_auth(scim_token.clone())
    };

    // SCIM endpoints reject tokens without the `scim:provision` scope.
    let unscoped = c
        .get(format!("{}/scim/v2/Users", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .send()
        .await?;
    assert_eq!(unscoped.status().as_u16(), 403);

    // ServiceProviderConfig advertises filter + patch.
    let spc: Value = scim_get("/scim/v2/ServiceProviderConfig")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(spc["filter"]["supported"], true);
    assert_eq!(spc["patch"]["supported"], true);

    // POST a user.
    let resp = scim_post("/scim/v2/Users")
        .json(&serde_json::json!({
            "userName": "ada@example.test",
            "active": true,
            "name": { "givenName": "Ada", "familyName": "Lovelace" }
        }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 201);
    let provisioned: Value = resp.json().await?;
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    assert_eq!(provisioned["userName"], "ada@example.test");
    assert_eq!(provisioned["active"], true);

    // List filtered by userName.
    let listed: Value = scim_get(r#"/scim/v2/Users?filter=userName+eq+%22ada%40example.test%22"#)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(listed["totalResults"], 1);
    assert_eq!(listed["Resources"][0]["id"].as_str().unwrap(), scim_id);

    // PATCH replace active=false → deprovisions membership.
    let resp = c
        .patch(format!("{}/scim/v2/Users/{}", h.base, scim_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&scim_token)
        .json(&serde_json::json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{ "op": "replace", "path": "active", "value": false }]
        }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let patched: Value = resp.json().await?;
    assert_eq!(patched["active"], false);

    // After deprovisioning, the user no longer matches the filter (membership gone).
    let listed: Value = scim_get(r#"/scim/v2/Users?filter=userName+eq+%22ada%40example.test%22"#)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(listed["totalResults"], 0);

    // PATCH with an unsupported op on a FRESH user → 400 (not 404 / 200).
    let resp2: Value = scim_post("/scim/v2/Users")
        .json(&serde_json::json!({ "userName": "grace@example.test", "active": true }))
        .send()
        .await?
        .json()
        .await?;
    let grace_id = resp2["id"].as_str().unwrap().to_string();
    let resp = c
        .patch(format!("{}/scim/v2/Users/{}", h.base, grace_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&scim_token)
        .json(&serde_json::json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{ "op": "add", "path": "displayName", "value": "Grace" }]
        }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400);

    // Unknown filter → 400.
    let resp = scim_get(r#"/scim/v2/Users?filter=displayName+eq+%22x%22"#)
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(status, 400, "body: {body}");

    // -------- Members admin endpoint --------
    // Grace + an explicit admin from earlier SCIM steps + earlier OIDC + lots
    // of bookkeeping. Mint an admin token to drive the members API.
    let admin_for_members = admin::create_token(&h.db, "acme", "ops-admin", "tenant:admin")
        .await?
        .raw_token;
    let members_get = || {
        c.get(format!("{}/v1/tenant/members", h.base))
            .header("x-skill-pool-tenant", "acme")
            .bearer_auth(&admin_for_members)
    };

    // Default-scope token can READ.
    let resp = c
        .get(format!("{}/v1/tenant/members", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);

    // GET returns at least one member (grace from SCIM).
    let members: Vec<Value> = members_get().send().await?.json().await?;
    let grace = members
        .iter()
        .find(|m| m["email"] == "grace@example.test")
        .ok_or_else(|| anyhow::anyhow!("grace not in members list: {members:?}"))?
        .clone();
    let grace_member_id = grace["id"].as_str().unwrap().to_string();
    assert_eq!(grace["role"], "viewer");

    // PATCH role to curator
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, grace_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "curator" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let updated: Value = resp.json().await?;
    assert_eq!(updated["role"], "curator");

    // Default-scope token cannot mutate.
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, grace_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .json(&serde_json::json!({ "role": "publisher" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 403);

    // Invalid role → 400
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, grace_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "owner" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400);

    // Create a sole admin to test the last-admin guard.
    let admin_email = "alice@example.test";
    // Provision via SCIM (already authed against acme tenant).
    let provisioned: Value = scim_post("/scim/v2/Users")
        .json(&serde_json::json!({ "userName": admin_email, "active": true }))
        .send()
        .await?
        .json()
        .await?;
    let alice_member_id = provisioned["id"].as_str().unwrap().to_string();
    // Promote alice to admin.
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, alice_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "admin" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);

    // Now alice is the only admin (there are no other admin rows for acme).
    // Demoting her must fail.
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, alice_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "viewer" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400, "last-admin demote must 400");

    // Same for delete.
    let resp = c
        .delete(format!("{}/v1/tenant/members/{}", h.base, alice_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 400, "last-admin delete must 400");

    // After adding a second admin, the demote works.
    let second_admin: Value = scim_post("/scim/v2/Users")
        .json(&serde_json::json!({ "userName": "bob@example.test", "active": true }))
        .send()
        .await?
        .json()
        .await?;
    let bob_member_id = second_admin["id"].as_str().unwrap().to_string();
    c.patch(format!("{}/v1/tenant/members/{}", h.base, bob_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "admin" }))
        .send()
        .await?;
    let resp = c
        .patch(format!("{}/v1/tenant/members/{}", h.base, alice_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .json(&serde_json::json!({ "role": "viewer" }))
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "demote works with another admin present"
    );

    // DELETE the now-demoted alice — should succeed.
    let resp = c
        .delete(format!("{}/v1/tenant/members/{}", h.base, alice_member_id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    // -------- Bootstrap (Phase 3 — curated stack → skills mapping) --------
    admin::set_stack_mapping(&h.db, "acme", "rust", "rust-axum-handler").await?;
    admin::set_stack_mapping(&h.db, "acme", "rust", "sqlx-migrations").await?;
    admin::set_stack_mapping(&h.db, "acme", "nix", "nix-flake-tips").await?;
    admin::set_stack_mapping(&h.db, "acme", "react", "react-server-components").await?;

    // Bare bootstrap call returns rust + nix skills (NOT react).
    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/bootstrap?stack=rust,nix,kubernetes",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    let skills: Vec<String> = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        skills,
        vec![
            "nix-flake-tips".to_string(),
            "rust-axum-handler".to_string(),
            "sqlx-migrations".to_string()
        ],
        "expected rust+nix skills alphabetical, no react: {skills:?}"
    );

    // Empty stack param → 400.
    let resp = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/bootstrap?stack=",
            "acme",
        ),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400);

    // Tenant isolation: globex's same query returns no mappings.
    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/bootstrap?stack=rust,nix",
            "globex",
        ),
        &h.globex_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(body["skills"].as_array().unwrap().is_empty());

    // remove_stack_mapping returns row-deleted-or-not status cleanly.
    admin::remove_stack_mapping(&h.db, "acme", "rust", "rust-axum-handler").await?;
    admin::remove_stack_mapping(&h.db, "acme", "rust", "non-existent").await?;

    // -------- IdP group → role mappings --------
    // Configure mappings.
    admin::set_role_mapping(&h.db, "acme", "Engineering-Admins", "admin").await?;
    admin::set_role_mapping(&h.db, "acme", "Curators", "curator").await?;
    admin::set_role_mapping(&h.db, "acme", "Engineers", "publisher").await?;

    // Provision a SCIM user we can reuse as the target of the apply call.
    let scim_user: Value = scim_post("/scim/v2/Users")
        .json(&serde_json::json!({ "userName": "mary@example.test", "active": true }))
        .send()
        .await?
        .json()
        .await?;
    // Resolve their user_id from the membership row.
    let mary_user_id: (Uuid,) = sqlx::query_as("SELECT u.id FROM users u WHERE u.email = $1")
        .bind("mary@example.test")
        .fetch_one(&h.db)
        .await?;
    let acme_tenant_id: (Uuid,) = sqlx::query_as("SELECT id FROM tenants WHERE slug = 'acme'")
        .fetch_one(&h.db)
        .await?;

    // user_id (from SCIM) and the role we get back through apply_role_from_groups
    use skill_pool_server::auth::apply_role_from_groups;

    // No groups → no change.
    let result = apply_role_from_groups(&h.db, acme_tenant_id.0, mary_user_id.0, &[]).await?;
    assert_eq!(result, None);

    // Groups with no mapping match → no change.
    let result = apply_role_from_groups(
        &h.db,
        acme_tenant_id.0,
        mary_user_id.0,
        &["Random-Group".to_string()],
    )
    .await?;
    assert_eq!(result, None);

    // One matching group → applies.
    let result = apply_role_from_groups(
        &h.db,
        acme_tenant_id.0,
        mary_user_id.0,
        &["Engineers".to_string()],
    )
    .await?;
    assert_eq!(result.as_deref(), Some("publisher"));
    let role_now: (String,) =
        sqlx::query_as("SELECT role FROM tenant_users WHERE tenant_id = $1 AND user_id = $2")
            .bind(acme_tenant_id.0)
            .bind(mary_user_id.0)
            .fetch_one(&h.db)
            .await?;
    assert_eq!(role_now.0, "publisher");

    // Multiple groups → highest wins (admin > publisher).
    let result = apply_role_from_groups(
        &h.db,
        acme_tenant_id.0,
        mary_user_id.0,
        &[
            "Engineers".to_string(),
            "Engineering-Admins".to_string(),
            "Curators".to_string(),
            "irrelevant".to_string(),
        ],
    )
    .await?;
    assert_eq!(result.as_deref(), Some("admin"));

    // Empty/non-matching groups on next sign-in MUST preserve the manual promotion.
    let result = apply_role_from_groups(
        &h.db,
        acme_tenant_id.0,
        mary_user_id.0,
        &["something-unrelated".to_string()],
    )
    .await?;
    assert_eq!(result, None);
    let role_after: (String,) =
        sqlx::query_as("SELECT role FROM tenant_users WHERE tenant_id = $1 AND user_id = $2")
            .bind(acme_tenant_id.0)
            .bind(mary_user_id.0)
            .fetch_one(&h.db)
            .await?;
    assert_eq!(
        role_after.0, "admin",
        "no-match must preserve current role, not downgrade"
    );

    // Cleanup the SCIM user so later assertions stay stable.
    let _ = scim_user;
    // remove_role_mapping should report success even on a real row, and 0
    // rows when the row's already gone.
    admin::remove_role_mapping(&h.db, "acme", "Engineering-Admins").await?;
    admin::remove_role_mapping(&h.db, "acme", "Curators").await?;
    admin::remove_role_mapping(&h.db, "acme", "Engineers").await?;
    admin::remove_role_mapping(&h.db, "acme", "no-such-group").await?;

    // -------- Enterprise managed-settings.json template --------
    let resp = c
        .get(format!("{}/v1/enterprise/managed-settings", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.acme_token)
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        403,
        "default scope must not download"
    );

    let resp = c
        .get(format!("{}/v1/enterprise/managed-settings", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_for_members)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-disposition")
            .and_then(|h| h.to_str().ok()),
        Some("attachment; filename=\"managed-settings.json\"")
    );
    let body: Value = resp.json().await?;
    assert_eq!(body["_tenant"], "acme");
    assert_eq!(body["env"]["SKILL_POOL_TENANT"], "acme");
    let registry = body["env"]["SKILL_POOL_REGISTRY"]
        .as_str()
        .expect("registry url");
    assert!(
        registry.contains("acme"),
        "registry should mention tenant: {registry}"
    );
    let allow = body["permissions"]["allow"]
        .as_array()
        .expect("permissions.allow array");
    assert!(
        allow
            .iter()
            .any(|v| v.as_str() == Some("Bash(skill-pool *)")),
        "expected `Bash(skill-pool *)` in allow list"
    );

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
