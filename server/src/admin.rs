//! Admin ops — exposed as plain functions on the library so the binary's
//! main and the integration tests can both call them. No network exposure;
//! talks directly to Postgres.

use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::hash_token;
use crate::config::Config;

pub struct CreatedTenant {
    pub id: Uuid,
}

pub struct DeletedTenant {
    pub id: Uuid,
    pub slug: String,
}

pub struct CreatedToken {
    pub id: Uuid,
    pub raw_token: String,
}

pub async fn connect(cfg: &Config) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .context("connect to database")
}

pub async fn create_tenant(
    db: &PgPool,
    slug: &str,
    name: &str,
    plan: &str,
) -> Result<CreatedTenant> {
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
    Ok(CreatedTenant { id: row.0 })
}

/// Hard-delete a tenant by slug. Relies on `ON DELETE CASCADE` on every
/// business table (audit_events, skills, skill_drafts, skill_usage_events,
/// tenant_theme, tenant_api_tokens, tenant_users, tenant_oidc, tenant_saml,
/// tenant_role_mappings, tenant_stack_mappings, skill_dependencies, …) to
/// remove all child rows in a single transaction.
///
/// Bundle storage is NOT swept by this function — bundle keys live under
/// `{tenant_id}/...` in the configured `SKILL_POOL_STORAGE_URI` and require
/// a separate operator action (the CLI prints the prefix). This is deliberate:
///
///   - On `fs://` the operator can `rm -rf <storage_root>/<tenant_id>`.
///   - On `s3://` they run `aws s3 rm s3://<bucket>/<tenant_id>/ --recursive`.
///   - For forensic / compliance reasons you may want to retain the bundles.
///
/// **Audit caveat:** the cascade also removes the tenant's `audit_events`
/// rows. Export to SIEM before calling this if your compliance regime
/// requires retaining audit history beyond the tenant lifetime.
pub async fn delete_tenant(db: &PgPool, slug: &str) -> Result<DeletedTenant> {
    let tenant: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
        .bind(slug)
        .fetch_optional(db)
        .await
        .context("look up tenant by slug")?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{slug}` not found"))?;

    let result = sqlx::query("DELETE FROM tenants WHERE id = $1")
        .bind(tenant_id)
        .execute(db)
        .await
        .context("delete tenant row (cascade removes children)")?;

    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "expected 1 row deleted for tenant `{slug}`, got {}",
            result.rows_affected()
        ));
    }

    Ok(DeletedTenant {
        id: tenant_id,
        slug: slug.to_string(),
    })
}

pub async fn create_token(
    db: &PgPool,
    tenant_slug: &str,
    name: &str,
    scope: &str,
) -> Result<CreatedToken> {
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

    Ok(CreatedToken {
        id: row.0,
        raw_token: raw,
    })
}

pub async fn set_stack_mapping(
    db: &PgPool,
    tenant_slug: &str,
    stack_tag: &str,
    skill_slug: &str,
) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    sqlx::query(
        "INSERT INTO tenant_stack_mappings (tenant_id, stack_tag, skill_slug) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, stack_tag, skill_slug) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(stack_tag)
    .bind(skill_slug)
    .execute(db)
    .await
    .context("insert tenant_stack_mappings")?;

    println!("mapping set for tenant `{tenant_slug}`:");
    println!("  stack tag: {stack_tag}");
    println!("  skill:     {skill_slug}");
    Ok(())
}

pub async fn list_stack_mappings(db: &PgPool, tenant_slug: &str) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT stack_tag, skill_slug FROM tenant_stack_mappings \
         WHERE tenant_id = $1 ORDER BY stack_tag, skill_slug",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    if rows.is_empty() {
        println!("(no stack mappings for tenant `{tenant_slug}`)");
    } else {
        println!("stack mappings for tenant `{tenant_slug}`:");
        for (tag, slug) in rows {
            println!("  {tag:<24} -> {slug}");
        }
    }
    Ok(())
}

pub async fn remove_stack_mapping(
    db: &PgPool,
    tenant_slug: &str,
    stack_tag: &str,
    skill_slug: &str,
) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let result = sqlx::query(
        "DELETE FROM tenant_stack_mappings \
         WHERE tenant_id = $1 AND stack_tag = $2 AND skill_slug = $3",
    )
    .bind(tenant_id)
    .bind(stack_tag)
    .bind(skill_slug)
    .execute(db)
    .await
    .context("delete tenant_stack_mappings")?;

    if result.rows_affected() == 0 {
        println!("(no mapping found for `{stack_tag}` -> `{skill_slug}`)");
    } else {
        println!("removed mapping `{stack_tag}` -> `{skill_slug}`");
    }
    Ok(())
}

pub async fn set_role_mapping(
    db: &PgPool,
    tenant_slug: &str,
    idp_group: &str,
    role: &str,
) -> Result<()> {
    if !matches!(role, "viewer" | "publisher" | "curator" | "admin") {
        return Err(anyhow!("role must be viewer / publisher / curator / admin"));
    }
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    sqlx::query(
        "INSERT INTO tenant_role_mappings (tenant_id, idp_group, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, idp_group) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(tenant_id)
    .bind(idp_group)
    .bind(role)
    .execute(db)
    .await
    .context("upsert tenant_role_mappings")?;

    println!("mapping set for tenant `{tenant_slug}`:");
    println!("  IdP group: {idp_group}");
    println!("  role:      {role}");
    Ok(())
}

pub async fn list_role_mappings(db: &PgPool, tenant_slug: &str) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT idp_group, role FROM tenant_role_mappings \
         WHERE tenant_id = $1 ORDER BY idp_group",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    if rows.is_empty() {
        println!("(no role mappings for tenant `{tenant_slug}`)");
    } else {
        println!("role mappings for tenant `{tenant_slug}`:");
        for (group, role) in rows {
            println!("  {group:<40} -> {role}");
        }
    }
    Ok(())
}

pub async fn remove_role_mapping(db: &PgPool, tenant_slug: &str, idp_group: &str) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let result =
        sqlx::query("DELETE FROM tenant_role_mappings WHERE tenant_id = $1 AND idp_group = $2")
            .bind(tenant_id)
            .bind(idp_group)
            .execute(db)
            .await
            .context("delete tenant_role_mappings")?;

    if result.rows_affected() == 0 {
        println!("(no mapping found for group `{idp_group}` on tenant `{tenant_slug}`)");
    } else {
        println!("removed mapping for `{idp_group}` on tenant `{tenant_slug}`");
    }
    Ok(())
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("spk_{}", hex::encode(bytes))
}

pub async fn set_saml(
    db: &PgPool,
    tenant_slug: &str,
    idp_entity_id: &str,
    idp_sso_url: &str,
    idp_x509_cert: &str,
    sp_entity_id: Option<&str>,
    default_role: &str,
) -> Result<()> {
    if !matches!(default_role, "viewer" | "publisher" | "curator" | "admin") {
        return Err(anyhow!(
            "default_role must be viewer / publisher / curator / admin"
        ));
    }
    if !idp_x509_cert.contains("-----BEGIN CERTIFICATE-----") {
        return Err(anyhow!(
            "idp_x509_cert must be PEM-encoded (include the BEGIN CERTIFICATE marker)"
        ));
    }
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    sqlx::query(
        "INSERT INTO tenant_saml \
           (tenant_id, idp_entity_id, idp_sso_url, idp_x509_cert, sp_entity_id, default_role) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
           idp_entity_id = EXCLUDED.idp_entity_id, \
           idp_sso_url = EXCLUDED.idp_sso_url, \
           idp_x509_cert = EXCLUDED.idp_x509_cert, \
           sp_entity_id = EXCLUDED.sp_entity_id, \
           default_role = EXCLUDED.default_role",
    )
    .bind(tenant_id)
    .bind(idp_entity_id)
    .bind(idp_sso_url)
    .bind(idp_x509_cert)
    .bind(sp_entity_id)
    .bind(default_role)
    .execute(db)
    .await
    .context("upsert tenant_saml")?;

    println!("saml configured for tenant `{tenant_slug}`");
    println!("  IdP entity: {idp_entity_id}");
    println!("  IdP SSO:    {idp_sso_url}");
    println!("  default:    {default_role}");
    println!();
    println!("Hand the IdP admin this metadata URL:");
    println!("  https://<public-origin>/v1/auth/saml/{tenant_slug}/metadata");
    Ok(())
}

pub async fn set_sso(
    db: &PgPool,
    tenant_slug: &str,
    issuer_url: &str,
    client_id: &str,
    client_secret: &str,
    default_role: &str,
) -> Result<()> {
    if !matches!(default_role, "viewer" | "publisher" | "curator" | "admin") {
        return Err(anyhow!(
            "default_role must be viewer / publisher / curator / admin"
        ));
    }
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    sqlx::query(
        "INSERT INTO tenant_sso (tenant_id, issuer_url, client_id, client_secret, default_role) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
           issuer_url = EXCLUDED.issuer_url, \
           client_id = EXCLUDED.client_id, \
           client_secret = EXCLUDED.client_secret, \
           default_role = EXCLUDED.default_role",
    )
    .bind(tenant_id)
    .bind(issuer_url)
    .bind(client_id)
    .bind(client_secret)
    .bind(default_role)
    .execute(db)
    .await
    .context("upsert tenant_sso")?;

    println!("sso configured for tenant `{tenant_slug}`");
    println!("  issuer: {issuer_url}");
    println!("  client: {client_id}");
    println!("  role:   {default_role}");
    Ok(())
}

/// Phase 5+ operational task: compute and store description_embedding
/// for skills that pre-date the embedding column (or were published
/// before the operator turned the feature on). Idempotent — only rows
/// with NULL embedding are touched.
///
/// Streams rows in pages so a large catalog can be processed without
/// holding the whole result set in memory.
pub async fn backfill_embeddings(
    db: &PgPool,
    embedder: &dyn crate::embedding::Embedder,
    tenant_slug: Option<&str>,
    limit: usize,
    dry_run: bool,
) -> Result<()> {
    if embedder.dimension().is_none() {
        return Err(anyhow!(
            "no embedder configured (NullEmbedder); rebuild with `--features fastembed` \
             and set embedding.enabled=true"
        ));
    }
    println!(
        "backfilling embeddings (limit={limit}, tenant={}, dry_run={dry_run})",
        tenant_slug.unwrap_or("ALL"),
    );

    let tenant_id: Option<Uuid> = match tenant_slug {
        Some(slug) => {
            let row: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
                    .bind(slug)
                    .fetch_optional(db)
                    .await?;
            Some(row.ok_or_else(|| anyhow!("tenant `{slug}` not found"))?.0)
        }
        None => None,
    };

    let page_size: i64 = 50;
    let mut processed: usize = 0;
    let mut updated: usize = 0;

    while processed < limit {
        let remaining = (limit - processed).min(page_size as usize) as i64;
        let rows: Vec<(Uuid, String, String)> = match tenant_id {
            Some(t) => sqlx::query_as(
                "SELECT id, slug, description \
                 FROM skills \
                 WHERE tenant_id = $1 AND description_embedding IS NULL \
                 ORDER BY created_at ASC \
                 LIMIT $2",
            )
            .bind(t)
            .bind(remaining)
            .fetch_all(db)
            .await?,
            None => sqlx::query_as(
                "SELECT id, slug, description \
                 FROM skills \
                 WHERE description_embedding IS NULL \
                 ORDER BY created_at ASC \
                 LIMIT $1",
            )
            .bind(remaining)
            .fetch_all(db)
            .await?,
        };

        if rows.is_empty() {
            break;
        }

        for (id, slug, description) in rows {
            processed += 1;
            let embedding = match embedder.embed(&description) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    println!("  skip:    {slug} (embedder returned None)");
                    continue;
                }
                Err(e) => {
                    println!("  error:   {slug}: {e}");
                    continue;
                }
            };
            if dry_run {
                println!("  would:   {slug} ({} dim)", embedding.len());
                continue;
            }
            let lit = crate::embedding::vector_to_pg_literal(&embedding);
            sqlx::query(
                "UPDATE skills SET description_embedding = $1::text::vector \
                 WHERE id = $2",
            )
            .bind(&lit)
            .bind(id)
            .execute(db)
            .await?;
            updated += 1;
            println!("  done:    {slug}");
        }
    }

    println!();
    println!(
        "summary: {processed} processed, {updated} updated{}",
        if dry_run { " (dry-run, no writes)" } else { "" }
    );
    Ok(())
}
