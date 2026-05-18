//! Admin ops invoked from the server binary — `skill-pool-server admin ...`.
//! No network exposure; talks directly to Postgres. Run on the box.

use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::auth::hash_token;
use crate::config::Config;
use crate::AdminCmd;

pub async fn run(cfg: &Config, cmd: AdminCmd) -> Result<()> {
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .context("connect to database")?;

    match cmd {
        AdminCmd::TenantCreate { slug, name, plan } => {
            create_tenant(&db, &slug, &name, &plan).await
        }
        AdminCmd::TokenCreate {
            tenant,
            name,
            scope,
        } => create_token(&db, &tenant, &name, &scope).await,
    }
}

async fn create_tenant(db: &sqlx::PgPool, slug: &str, name: &str, plan: &str) -> Result<()> {
    if !matches!(plan, "team" | "business" | "enterprise") {
        return Err(anyhow!("plan must be one of: team, business, enterprise"));
    }
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO tenants (slug, name, plan_tier) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(slug)
    .bind(name)
    .bind(plan)
    .fetch_one(db)
    .await
    .context("insert tenant")?;
    println!("tenant created");
    println!("  id:   {}", row.0);
    println!("  slug: {slug}");
    println!("  plan: {plan}");
    println!("\nnext: skill-pool-server admin token-create --tenant {slug} --name bootstrap");
    Ok(())
}

async fn create_token(db: &sqlx::PgPool, tenant_slug: &str, name: &str, scope: &str) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let raw = generate_token();
    let hashed = hash_token(&raw);

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO tenant_api_tokens (tenant_id, hashed_token, name, scope) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id)
    .bind(&hashed)
    .bind(name)
    .bind(scope)
    .fetch_one(db)
    .await
    .context("insert token")?;

    println!("token created");
    println!("  id:     {}", row.0);
    println!("  tenant: {tenant_slug}");
    println!("  scope:  {scope}");
    println!();
    println!("RAW TOKEN (shown once — copy now):");
    println!("  {raw}");
    Ok(())
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("spk_{}", hex::encode(bytes))
}
