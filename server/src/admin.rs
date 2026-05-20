//! Admin ops — exposed as plain functions on the library so the binary's
//! main and the integration tests can both call them. No network exposure;
//! talks directly to Postgres.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
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
    /// Display prefix (first ~12 chars of `raw_token`). Stored on the row
    /// so the management UI can identify a token without needing the
    /// secret. Not unique, not used for auth.
    pub prefix: String,
    pub created_at: DateTime<Utc>,
}

/// Listable view of an API token row. Excludes `hashed_token` — that column
/// must never leak past the auth fast-path.
#[derive(Debug, Clone)]
pub struct TokenSummary {
    pub id: Uuid,
    pub name: String,
    pub prefix: Option<String>,
    pub scope: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
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

/// Set or clear a tenant's data-residency fields.
///
/// Pass `region = Some` to tag the tenant with a free-form region marker
/// (`"eu-west-1"`, `"us-east-1"`, …). Pass `storage_uri = Some` to override
/// the global bundle backend for this tenant only. Either or both can be
/// `Some` per call; `None` leaves the column unchanged.
///
/// `storage_uri` is validated by attempting to construct a `Storage` from
/// it — typos are caught here, not at first bundle write. A successful
/// return guarantees the URI resolves; reachability of a remote backend
/// (S3 credentials, DNS) is still verified on first use.
pub async fn set_tenant_residency(
    db: &PgPool,
    slug: &str,
    region: Option<&str>,
    storage_uri: Option<&str>,
) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{slug}` not found or suspended"))?;

    if let Some(uri) = storage_uri {
        crate::storage::Storage::from_uri(uri)
            .with_context(|| format!("validate storage_uri {uri:?}"))?;
    }

    // COALESCE so a single-field update doesn't clobber the other.
    // Sentinel `''` distinguishes "leave alone" (NULL) from "explicitly
    // clear" (empty string treated as NULL by NULLIF).
    let result = sqlx::query(
        "UPDATE tenants
           SET region      = COALESCE(NULLIF($2, ''), region),
               storage_uri = COALESCE(NULLIF($3, ''), storage_uri)
         WHERE id = $1",
    )
    .bind(tenant_id)
    .bind(region.unwrap_or(""))
    .bind(storage_uri.unwrap_or(""))
    .execute(db)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow!("expected 1 row updated, got {}", result.rows_affected()));
    }

    println!("residency updated for `{slug}`");
    if let Some(r) = region {
        println!("  region:      {r}");
    }
    if let Some(u) = storage_uri {
        println!("  storage_uri: {u}");
    }
    println!(
        "\n→ restart the server (or wait for cache TTL) so per-tenant storage rebuilds from the new URI."
    );
    Ok(())
}

/// Set or clear a tenant's session idle-timeout policy.
///
/// `max_age_secs = Some(n)` sets the per-tenant cap on the web's session
/// cookie maxAge. `None` clears the column (falls back to web's 14-day
/// default at next login). The DB CHECK constraint enforces the
/// 60..2_592_000 range so callers don't need to.
///
/// Sessions in flight are not invalidated — policy applies at next login.
pub async fn set_session_max_age(db: &PgPool, slug: &str, max_age_secs: Option<i32>) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{slug}` not found or suspended"))?;

    let result = sqlx::query("UPDATE tenants SET session_max_age_secs = $2 WHERE id = $1")
        .bind(tenant_id)
        .bind(max_age_secs)
        .execute(db)
        .await
        .with_context(|| {
            // The CHECK constraint will reject out-of-range values; surface
            // that as a clean error message instead of a raw sqlx message.
            format!("set session_max_age_secs for `{slug}` (range 60..2592000 seconds)")
        })?;
    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "expected 1 row updated, got {}",
            result.rows_affected()
        ));
    }

    match max_age_secs {
        None => println!("session policy cleared for `{slug}` (falls back to 14d default)"),
        Some(n) => println!(
            "session policy set for `{slug}`: maxAge = {n} seconds (~{:.1} days)",
            n as f64 / 86_400.0
        ),
    }
    println!("\n→ applies at next login. Sessions already in flight keep their old maxAge.");
    Ok(())
}

/// Set, update, or clear a tenant's CLI startup banner (#9).
///
/// Semantics:
///   - `clear = true` → both `banner_text` and `banner_url` are set to NULL
///     regardless of `text`/`url`. Mutually exclusive with the other args
///     in the CLI front-end; the helper itself just honors the flag.
///   - `text = Some("")` or `url = Some("")` → that column is explicitly
///     cleared (sentinel for "blank this out without touching the other").
///   - `text = None` or `url = None` → that column is left unchanged.
///   - `text = Some("Welcome…")` / `url = Some("https://…")` → write the
///     new value. CHECK constraints enforce the length and scheme.
///
/// We deliberately do NOT validate URL scheme / text length here — the
/// DB CHECK constraint is the single source of truth and surfaces a
/// clean error via the `with_context` wrapper.
pub async fn set_tenant_banner(
    db: &PgPool,
    slug: &str,
    text: Option<&str>,
    url: Option<&str>,
    clear: bool,
) -> Result<()> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) =
        tenant.ok_or_else(|| anyhow!("tenant `{slug}` not found or suspended"))?;

    // Three update modes mapped to a single SQL statement via $4 (clear flag):
    //   * clear=true  → set both to NULL unconditionally.
    //   * clear=false → for each column, NULL if the caller passed "" sentinel,
    //                   the new value if they passed Some(non-empty),
    //                   COALESCE-leave-alone if they passed None.
    // Encoding `None` as the "sentinel-not-passed" value uses a non-empty
    // string we know can never reach a real banner field (length > 240 would
    // hit CHECK, but we use a literal that text-input never produces — a
    // form-feed). Simpler than two queries.
    const KEEP: &str = "\x0c__skill_pool_keep__\x0c";
    let result = sqlx::query(
        "UPDATE tenants
           SET banner_text = CASE
                   WHEN $4 THEN NULL
                   WHEN $2 = $5 THEN banner_text
                   WHEN $2 = '' THEN NULL
                   ELSE $2
               END,
               banner_url = CASE
                   WHEN $4 THEN NULL
                   WHEN $3 = $5 THEN banner_url
                   WHEN $3 = '' THEN NULL
                   ELSE $3
               END
         WHERE id = $1",
    )
    .bind(tenant_id)
    .bind(text.unwrap_or(KEEP))
    .bind(url.unwrap_or(KEEP))
    .bind(clear)
    .bind(KEEP)
    .execute(db)
    .await
    .with_context(|| {
        format!("set banner for `{slug}` (text ≤240 chars, url must be https://...)")
    })?;
    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "expected 1 row updated, got {}",
            result.rows_affected()
        ));
    }

    if clear {
        println!("banner cleared for `{slug}`");
    } else {
        println!("banner updated for `{slug}`");
        if let Some(t) = text {
            if t.is_empty() {
                println!("  text: (cleared)");
            } else {
                println!("  text: {t}");
            }
        }
        if let Some(u) = url {
            if u.is_empty() {
                println!("  url:  (cleared)");
            } else {
                println!("  url:  {u}");
            }
        }
    }
    println!(
        "\n→ CLI fetches this on next shell session (or in 24h, whichever first)."
    );
    Ok(())
}

/// Set or clear a tenant's per-tenant rate-limit overrides (#8 §L20).
///
/// Semantics:
///   * `clear = true` → both `rate_limit_rpm` and `rate_limit_burst`
///     are set to NULL (revert to plan default). Mutually exclusive
///     with `rpm`/`burst` in the CLI front-end.
///   * `rpm = Some(n)` / `burst = Some(n)` → write the override. NULL on
///     the omitted column is preserved (COALESCE).
///   * `n = 0` → rejected with a clean error before hitting the DB
///     CHECK so the operator gets a useful message.
///
/// Range validation is the DB's job (CHECK constraints on the
/// migration); we just surface failures cleanly.
pub async fn set_tenant_rate_limits(
    db: &PgPool,
    slug: &str,
    rpm: Option<i32>,
    burst: Option<i32>,
    clear: bool,
) -> Result<()> {
    if let Some(0) = rpm {
        return Err(anyhow!("--rpm must be > 0 (use --clear to revert to plan default)"));
    }
    if let Some(0) = burst {
        return Err(anyhow!("--burst must be > 0 (use --clear to revert to plan default)"));
    }
    if !clear && rpm.is_none() && burst.is_none() {
        return Err(anyhow!("pass --rpm, --burst, or --clear (at least one required)"));
    }

    let tenant: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, plan_tier FROM tenants WHERE slug = $1 AND status = 'active'",
    )
    .bind(slug)
    .fetch_optional(db)
    .await?;
    let (tenant_id, plan) =
        tenant.ok_or_else(|| anyhow!("tenant `{slug}` not found or suspended"))?;

    // Encode "leave alone" as -1 sentinel; CHECK constraint rejects
    // negatives so the column can never hold the sentinel. `--clear`
    // wins over individual values regardless of what was passed.
    const KEEP: i32 = -1;
    let result = sqlx::query(
        "UPDATE tenants \
           SET rate_limit_rpm = CASE \
                   WHEN $4 THEN NULL \
                   WHEN $2 = $5 THEN rate_limit_rpm \
                   ELSE $2 \
               END, \
               rate_limit_burst = CASE \
                   WHEN $4 THEN NULL \
                   WHEN $3 = $5 THEN rate_limit_burst \
                   ELSE $3 \
               END \
         WHERE id = $1",
    )
    .bind(tenant_id)
    .bind(rpm.unwrap_or(KEEP))
    .bind(burst.unwrap_or(KEEP))
    .bind(clear)
    .bind(KEEP)
    .execute(db)
    .await
    .with_context(|| {
        format!(
            "set rate limits for `{slug}` (rpm in 1..=100000, burst in 1..=10000)"
        )
    })?;
    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "expected 1 row updated, got {}",
            result.rows_affected()
        ));
    }

    // Read back so we can print the *effective* limits (plan default
    // when both columns are NULL).
    let (rpm_override, burst_override): (Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT rate_limit_rpm, rate_limit_burst FROM tenants WHERE id = $1",
    )
    .bind(tenant_id)
    .fetch_one(db)
    .await?;
    let effective =
        crate::rate_limit::resolve_for_tenant(&plan, rpm_override, burst_override);

    if clear {
        println!("rate limits cleared for `{slug}` (reverted to `{plan}` plan defaults)");
    } else {
        println!("rate limits updated for `{slug}` (plan: {plan})");
    }
    println!("  rpm:   {}  ({})", effective.rpm,
        if rpm_override.is_some() { "override" } else { "plan default" });
    println!("  burst: {}  ({})", effective.burst,
        if burst_override.is_some() { "override" } else { "plan default" });
    println!(
        "\n→ applies immediately to new requests (no per-process cache TTL in v1)."
    );
    Ok(())
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
    create_token_inner(db, tenant_slug, name, scope, None).await
}

/// User-scoped variant: records `created_by` so `list_user_tokens` can scope
/// to the calling principal. Used by the personal token-management UI.
pub async fn create_user_token(
    db: &PgPool,
    tenant_slug: &str,
    name: &str,
    scope: &str,
    created_by: Uuid,
) -> Result<CreatedToken> {
    create_token_inner(db, tenant_slug, name, scope, Some(created_by)).await
}

async fn create_token_inner(
    db: &PgPool,
    tenant_slug: &str,
    name: &str,
    scope: &str,
    created_by: Option<Uuid>,
) -> Result<CreatedToken> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let raw = generate_token();
    let hashed = hash_token(&raw);
    // First 12 chars: `spk_` + 8 hex chars. Non-secret display affordance —
    // pairs a UI row to whatever copy the caller pasted into a script.
    let prefix = raw.chars().take(12).collect::<String>();

    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO tenant_api_tokens \
            (tenant_id, hashed_token, name, scope, token_prefix, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(&hashed)
    .bind(name)
    .bind(scope)
    .bind(&prefix)
    .bind(created_by)
    .fetch_one(db)
    .await
    .context("insert token")?;

    Ok(CreatedToken {
        id: row.0,
        raw_token: raw,
        prefix,
        created_at: row.1,
    })
}

/// List API tokens minted by `user_id` in `tenant_slug`. Includes both
/// active and revoked rows so the UI can render history. The `hashed_token`
/// column is intentionally excluded from the projection — see `TokenSummary`.
#[allow(clippy::type_complexity)] // sqlx query_as tuple type, one-off
pub async fn list_user_tokens(
    db: &PgPool,
    tenant_slug: &str,
    user_id: Uuid,
) -> Result<Vec<TokenSummary>> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let rows: Vec<(
        Uuid,
        String,
        Option<String>,
        String,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, name, token_prefix, scope, created_at, last_used_at, revoked_at \
         FROM tenant_api_tokens \
         WHERE tenant_id = $1 AND created_by = $2 \
         ORDER BY created_at DESC",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("list user tokens")?;

    Ok(rows
        .into_iter()
        .map(
            |(id, name, prefix, scope, created_at, last_used_at, revoked_at)| TokenSummary {
                id,
                name,
                prefix,
                scope,
                created_at,
                last_used_at,
                revoked_at,
            },
        )
        .collect())
}

/// Revoke a token owned by `user_id`. Idempotent — calling twice is a no-op
/// (the second call updates 0 rows because `revoked_at IS NOT NULL` short-
/// circuits the WHERE). Returns `Ok(false)` if no row matches (wrong tenant,
/// wrong owner, or never existed) so the route can map that to a 404 without
/// peeking at the row first.
pub async fn revoke_user_token(
    db: &PgPool,
    tenant_slug: &str,
    user_id: Uuid,
    token_id: Uuid,
) -> Result<bool> {
    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    // Two-step: first check the row exists at all (so we can distinguish
    // 404 from "already revoked is fine"), then update. The UPDATE is a
    // no-op for already-revoked rows, which is the idempotency contract.
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_api_tokens \
         WHERE tenant_id = $1 AND id = $2 AND created_by = $3",
    )
    .bind(tenant_id)
    .bind(token_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if exists.is_none() {
        return Ok(false);
    }

    sqlx::query(
        "UPDATE tenant_api_tokens SET revoked_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND created_by = $3 AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(token_id)
    .bind(user_id)
    .execute(db)
    .await
    .context("revoke user token")?;

    Ok(true)
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

// ---------------------------------------------------------------------------
// Custom domains (Phase 5 / Enterprise)
// ---------------------------------------------------------------------------

/// CLI helper: add a hostname for a tenant, returning the verification
/// record the operator should hand back to the tenant admin to paste
/// into DNS.
pub async fn add_custom_domain(db: &PgPool, tenant_slug: &str, hostname: &str) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let host = normalize_admin_hostname(hostname)?;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let token = hex::encode(buf);

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO tenant_custom_domains (tenant_id, hostname, verification_token) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(tenant_id)
    .bind(&host)
    .bind(&token)
    .fetch_one(db)
    .await
    .with_context(|| format!("insert tenant_custom_domains row for {host}"))?;

    println!("custom domain added for tenant `{tenant_slug}`:");
    println!("  id:       {}", row.0);
    println!("  hostname: {host}");
    println!();
    println!("Ask the tenant admin to add this DNS record:");
    println!("  _skill-pool-verify.{host} TXT {token}");
    println!();
    println!("Then run:");
    println!("  skill-pool-server admin custom-domain --tenant {tenant_slug} verify --id {}", row.0);
    Ok(())
}

pub async fn list_custom_domains(db: &PgPool, tenant_slug: &str) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, hostname, status, last_error FROM tenant_custom_domains \
         WHERE tenant_id = $1 ORDER BY created_at DESC",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    if rows.is_empty() {
        println!("(no custom domains for tenant `{tenant_slug}`)");
    } else {
        println!("custom domains for tenant `{tenant_slug}`:");
        for (id, hostname, status, last_error) in rows {
            print!("  {hostname:<40} {status:<10} {id}");
            if let Some(e) = last_error {
                if status == "failed" {
                    print!("  — {e}");
                }
            }
            println!();
        }
    }
    Ok(())
}

pub async fn verify_custom_domain(db: &PgPool, tenant_slug: &str, id: Uuid) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT hostname, verification_token FROM tenant_custom_domains \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    let (hostname, _token) = row.ok_or_else(|| anyhow!("custom domain {id} not found"))?;
    // The route handler does the heavy lifting; the CLI just nudges the
    // tenant admin to call the endpoint. We could re-implement the
    // hickory lookup here, but keeping a single code path means one
    // place to fix bugs.
    println!(
        "verification is performed at runtime via the HTTP endpoint:\n\
         \n\
         curl -X POST -H 'Authorization: Bearer $TOKEN' \\\n\
              -H 'x-skill-pool-tenant: {tenant_slug}' \\\n\
              http://<host>/v1/tenant/custom-domains/{id}/verify\n\
         \n\
         (host = {hostname})"
    );
    Ok(())
}

/// Operator override: skip DNS, flip status straight to `active`. Used
/// when a tenant has provided their own certificate via a private CA or
/// when DNS verification is impractical (air-gapped deploys).
pub async fn activate_custom_domain(db: &PgPool, tenant_slug: &str, id: Uuid) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let result = sqlx::query(
        "UPDATE tenant_custom_domains \
            SET status = 'active', \
                last_checked_at = now(), \
                last_error = NULL, \
                activated_at = COALESCE(activated_at, now()) \
          WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(tenant_id)
    .execute(db)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow!("custom domain {id} not found"));
    }
    println!("custom domain {id} for tenant `{tenant_slug}` marked active");
    println!("(running server processes will pick this up on the next refresh tick, ~60s)");
    Ok(())
}

pub async fn remove_custom_domain(db: &PgPool, tenant_slug: &str, id: Uuid) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let result = sqlx::query(
        "DELETE FROM tenant_custom_domains WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(tenant_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        println!("(no custom domain {id} for tenant `{tenant_slug}`)");
    } else {
        println!("removed custom domain {id} from tenant `{tenant_slug}`");
    }
    Ok(())
}

async fn lookup_tenant_id(db: &PgPool, tenant_slug: &str) -> Result<Uuid> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    Ok(row.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?.0)
}

fn normalize_admin_hostname(raw: &str) -> Result<String> {
    let h = raw.trim().trim_end_matches('.').to_lowercase();
    if h.is_empty() {
        return Err(anyhow!("hostname is empty"));
    }
    if !h.contains('.') {
        return Err(anyhow!("hostname must be fully qualified"));
    }
    Ok(h)
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

/// Inputs for `set_email_branding`. Bundled in a struct so the
/// helper signature is stable as the column list grows.
pub struct EmailBrandingArgs<'a> {
    pub from_addr: &'a str,
    pub from_name: Option<&'a str>,
    pub reply_to: Option<&'a str>,
    pub smtp_url: &'a str,
    pub smtp_password: &'a str,
    pub footer_html: Option<&'a str>,
}

/// Configure (or update) per-tenant branded transactional email. The
/// SMTP password is encrypted at rest with AES-256-GCM via
/// `email_branding::encrypt_password`. See `docs/enterprise/branded-emails.md`.
pub async fn set_email_branding(
    db: &PgPool,
    tenant_slug: &str,
    args: EmailBrandingArgs<'_>,
) -> Result<()> {
    if !crate::email_branding::looks_like_email(args.from_addr.trim()) {
        return Err(anyhow!("from_addr must be a valid email address"));
    }
    if let Some(rt) = args.reply_to {
        if !rt.trim().is_empty() && !crate::email_branding::looks_like_email(rt.trim()) {
            return Err(anyhow!("reply_to must be a valid email address or omitted"));
        }
    }
    if !(args.smtp_url.starts_with("smtp://") || args.smtp_url.starts_with("smtps://")) {
        return Err(anyhow!("smtp_url must start with smtp:// or smtps://"));
    }
    if args.smtp_password.is_empty() {
        return Err(anyhow!("smtp_password must not be empty"));
    }

    let tenant: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
            .bind(tenant_slug)
            .fetch_optional(db)
            .await?;
    let (tenant_id,) =
        tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found or suspended"))?;

    let enc = crate::email_branding::encrypt_password(args.smtp_password);

    sqlx::query(
        "INSERT INTO tenant_email_branding \
            (tenant_id, from_addr, from_name, reply_to, smtp_url, smtp_password_enc, footer_html, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            from_addr = EXCLUDED.from_addr, \
            from_name = EXCLUDED.from_name, \
            reply_to = EXCLUDED.reply_to, \
            smtp_url = EXCLUDED.smtp_url, \
            smtp_password_enc = EXCLUDED.smtp_password_enc, \
            footer_html = EXCLUDED.footer_html, \
            updated_at = now()",
    )
    .bind(tenant_id)
    .bind(args.from_addr.trim())
    .bind(args.from_name.map(str::trim).filter(|s| !s.is_empty()))
    .bind(args.reply_to.map(str::trim).filter(|s| !s.is_empty()))
    .bind(args.smtp_url)
    .bind(&enc)
    .bind(args.footer_html.filter(|s| !s.is_empty()))
    .execute(db)
    .await
    .context("upsert tenant_email_branding")?;

    println!("email branding configured for `{tenant_slug}`");
    println!("  from:      {}", args.from_addr);
    if let Some(n) = args.from_name {
        println!("  from_name: {n}");
    }
    if let Some(rt) = args.reply_to {
        println!("  reply_to:  {rt}");
    }
    println!("  smtp_url:  {}", args.smtp_url);
    if std::env::var(crate::email_branding::ENCRYPTION_KEY_ENV).is_err() {
        println!(
            "\n[WARN] {} is unset — password stored as base64 (NOT encrypted). \
             Set this env in production deployments.",
            crate::email_branding::ENCRYPTION_KEY_ENV
        );
    }
    Ok(())
}

/// One-shot: send a test message through the tenant's branded SMTP
/// transport, mirroring the `POST /v1/tenant/email-branding/test`
/// endpoint. Useful from a deploy console without minting an admin
/// token first.
pub async fn email_branding_test(db: &PgPool, tenant_slug: &str, recipient: &str) -> Result<()> {
    if !crate::email_branding::looks_like_email(recipient) {
        return Err(anyhow!("recipient must be a valid email address"));
    }
    let tenant: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
        .bind(tenant_slug)
        .fetch_optional(db)
        .await?;
    let (tenant_id,) = tenant.ok_or_else(|| anyhow!("tenant `{tenant_slug}` not found"))?;

    let row = crate::email_branding::load_row(db, tenant_id)
        .await?
        .ok_or_else(|| anyhow!("no email branding configured for `{tenant_slug}`"))?;

    let cache = crate::email_branding::TransportCache::new();
    let subject = format!("[skill-pool] Branded-email test for tenant {tenant_slug}");
    let body = format!(
        "This is a CLI-initiated test message confirming that branded transactional \
         email is wired correctly for `{tenant_slug}`.\n",
    );
    match crate::email_branding::send_branded(&cache, &row, recipient, &subject, &body).await {
        crate::email_branding::SendOutcome::Success => {
            println!("test email queued to {recipient} via {}", row.smtp_url);
            Ok(())
        }
        crate::email_branding::SendOutcome::Failed(e) => {
            Err(anyhow!("send failed: {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Projects (Layer 2 — schema-backed curator bundles)
// ---------------------------------------------------------------------------

/// A project row as returned by create / update / list operations.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub git_remote: Option<String>,
    pub stack_tags: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A single item in a project's curated list.
#[derive(Debug, Clone)]
pub struct ProjectItem {
    pub skill_slug: String,
    pub kind: String,
    pub position: i32,
}

/// A project with its curated item list.
#[derive(Debug, Clone)]
pub struct ProjectWithItems {
    pub project: Project,
    pub items: Vec<ProjectItem>,
}

/// Partial-update patch for `update_project`. All fields are optional;
/// `None` means "leave unchanged".
#[derive(Debug, Default)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub git_remote: Option<Option<String>>,
    pub stack_tags: Option<Vec<String>>,
    /// Auto-refresh interval for project plans in seconds. `Some(None)`
    /// clears it (explicit-only). `Some(Some(n))` sets n-second polling.
    /// `None` leaves the column unchanged.
    pub plan_auto_refresh_interval_secs: Option<Option<i32>>,
}

// Private type alias to avoid repetitive complex tuple type annotations in
// query_as calls. Clippy `type_complexity` lint is suppressed on the
// functions that use it (same pattern as `list_user_tokens` above).
//
// Columns: (id, slug, name, description, git_remote, stack_tags, created_at, updated_at)
type ProjectRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Vec<String>,
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
);

/// Normalise a git remote URL so that SSH and HTTPS forms of the same
/// repository resolve to the same string, enabling reliable lookup.
///
/// Transformations applied (in order):
///   1. Strip trailing `.git` suffix.
///   2. Convert `git@host:owner/repo` SSH shorthand to `https://host/owner/repo`.
///   3. Lowercase the scheme + host portion.
///   4. Strip any trailing `/`.
///
/// The URL is returned as-is (lowercased) if it doesn't match any known
/// pattern — this keeps forward compatibility with future VCS hostings.
pub fn normalize_git_remote(url: &str) -> String {
    let s = url.trim();

    // Strip trailing .git before further processing.
    let s = s.strip_suffix(".git").unwrap_or(s);

    // Convert SSH shorthand: git@github.com:owner/repo → https://github.com/owner/repo
    // Pattern: starts with an optional "git@" prefix, then host:path.
    let normalized = if let Some(rest) = s.strip_prefix("git@") {
        // rest = "github.com:owner/repo"
        if let Some(colon_pos) = rest.find(':') {
            let host = &rest[..colon_pos];
            let path = &rest[colon_pos + 1..];
            format!("https://{}/{}", host.to_lowercase(), path)
        } else {
            s.to_string()
        }
    } else {
        // Already https:// or http:// or unknown — lowercase the scheme+host.
        // We do a best-effort lowercase of the entire string for simplicity;
        // paths are case-sensitive on some hosts (GitHub is not, but GitLab
        // can be) so we only lowercase up to the third `/` (end of host).
        if let Some(after_scheme) = s.find("://").map(|i| i + 3) {
            let scheme_host_end = s[after_scheme..]
                .find('/')
                .map(|i| after_scheme + i)
                .unwrap_or(s.len());
            let scheme_host = s[..scheme_host_end].to_lowercase();
            format!("{}{}", scheme_host, &s[scheme_host_end..])
        } else {
            s.to_lowercase()
        }
    };

    // Strip trailing slash.
    normalized.trim_end_matches('/').to_string()
}

/// Create a new project for the tenant. Returns the newly created row.
///
/// Returns an error if a project with the same slug already exists in the
/// tenant (the `UNIQUE (tenant_id, slug)` constraint surfaces as a
/// `Conflict` error).
pub async fn create_project(
    db: &PgPool,
    tenant_slug: &str,
    slug: &str,
    name: &str,
    description: Option<&str>,
    git_remote: Option<&str>,
) -> Result<Project> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let normalized_remote = git_remote.map(normalize_git_remote);

    let row: ProjectRow = sqlx::query_as(
            "INSERT INTO tenant_projects \
               (tenant_id, slug, name, description, git_remote) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, slug::text, name, description, git_remote, stack_tags, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(normalized_remote.as_deref())
        .fetch_one(db)
        .await
        .with_context(|| format!("create project `{slug}` for tenant `{tenant_slug}`"))?;

    Ok(Project {
        id: row.0,
        slug: row.1,
        name: row.2,
        description: row.3,
        git_remote: row.4,
        stack_tags: row.5,
        created_at: row.6,
        updated_at: row.7,
    })
}

/// Look up a single project (without items) by its slug.
/// Returns `None` if no project with that slug exists for the tenant.
pub async fn get_project(
    db: &PgPool,
    tenant_slug: &str,
    slug: &str,
) -> Result<Option<ProjectWithItems>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let project_row: Option<ProjectRow> = sqlx::query_as(
            "SELECT id, slug::text, name, description, git_remote, stack_tags, created_at, updated_at \
             FROM tenant_projects \
             WHERE tenant_id = $1 AND slug = $2",
        )
        .bind(tenant_id)
        .bind(slug)
        .fetch_optional(db)
        .await?;

    let Some(p) = project_row else {
        return Ok(None);
    };
    let project_id = p.0;
    let project = Project {
        id: project_id,
        slug: p.1,
        name: p.2,
        description: p.3,
        git_remote: p.4,
        stack_tags: p.5,
        created_at: p.6,
        updated_at: p.7,
    };

    let item_rows: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT skill_slug, kind, position \
         FROM tenant_project_items \
         WHERE project_id = $1 \
         ORDER BY position ASC, skill_slug ASC",
    )
    .bind(project_id)
    .fetch_all(db)
    .await?;

    let items = item_rows
        .into_iter()
        .map(|(skill_slug, kind, position)| ProjectItem { skill_slug, kind, position })
        .collect();

    Ok(Some(ProjectWithItems { project, items }))
}

/// List all projects for the tenant (without items). Ordered by slug.
pub async fn list_projects(db: &PgPool, tenant_slug: &str) -> Result<Vec<Project>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let rows: Vec<ProjectRow> = sqlx::query_as(
            "SELECT id, slug::text, name, description, git_remote, stack_tags, created_at, updated_at \
             FROM tenant_projects \
             WHERE tenant_id = $1 \
             ORDER BY slug ASC",
        )
        .bind(tenant_id)
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| Project {
            id: r.0,
            slug: r.1,
            name: r.2,
            description: r.3,
            git_remote: r.4,
            stack_tags: r.5,
            created_at: r.6,
            updated_at: r.7,
        })
        .collect())
}

/// Like `list_projects` but co-fetches the item count per project in a single
/// query. Lets the admin list UI render an "Items" column without N+1.
#[allow(clippy::type_complexity)] // sqlx tuple — one-off projection
pub async fn list_projects_with_counts(
    db: &PgPool,
    tenant_slug: &str,
) -> Result<Vec<(Project, i64)>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    type Row = (
        uuid::Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Vec<String>,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        i64,
    );

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT tp.id, tp.slug::text, tp.name, tp.description, tp.git_remote, \
                tp.stack_tags, tp.created_at, tp.updated_at, \
                COALESCE(ic.cnt, 0) AS item_count \
         FROM tenant_projects tp \
         LEFT JOIN ( \
             SELECT project_id, COUNT(*) AS cnt \
             FROM tenant_project_items \
             GROUP BY project_id \
         ) ic ON ic.project_id = tp.id \
         WHERE tp.tenant_id = $1 \
         ORDER BY tp.slug ASC",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let project = Project {
                id: r.0,
                slug: r.1,
                name: r.2,
                description: r.3,
                git_remote: r.4,
                stack_tags: r.5,
                created_at: r.6,
                updated_at: r.7,
            };
            (project, r.8)
        })
        .collect())
}

/// Apply a partial update to a project's metadata fields.
/// Only non-`None` fields in `patch` are written; others are left unchanged.
/// Returns the updated project row.
pub async fn update_project(
    db: &PgPool,
    tenant_slug: &str,
    slug: &str,
    patch: ProjectPatch,
) -> Result<Project> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    // COALESCE pattern: pass a sentinel that can never occur in valid data so
    // we can distinguish "leave unchanged" from "set to NULL/empty".
    // We use the same SQL CASE approach as `set_tenant_banner`.
    //
    // For nullable fields (description, git_remote) we need three states:
    //   - None (patch field is None)  → keep existing value
    //   - Some(None)                  → set to NULL
    //   - Some(Some(v))               → set to v
    //
    // We encode this by using two parameters per nullable: a flag and the value.

    let normalized_remote = patch
        .git_remote
        .as_ref()
        .and_then(|o| o.as_deref().map(normalize_git_remote));

    // Build update SQL dynamically based on which fields are set.
    // This avoids touching unchanged columns and avoids multi-state CASE
    // complexity by using COALESCE(NULLIF($n, sentinel), col) patterns.
    //
    // Simpler approach: always write all columns, using COALESCE to skip
    // unchanged ones. We read the current row first so we can fill gaps.
    type CurRow = (String, Option<String>, Option<String>, Vec<String>, Option<i32>);
    let current: Option<CurRow> = sqlx::query_as(
        "SELECT name, description, git_remote, stack_tags, plan_auto_refresh_interval_secs \
         FROM tenant_projects \
         WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(slug)
    .fetch_optional(db)
    .await?;

    let (cur_name, cur_desc, cur_remote, cur_tags, cur_refresh) =
        current.ok_or_else(|| anyhow!("project `{slug}` not found for tenant `{tenant_slug}`"))?;

    let new_name = patch.name.as_deref().unwrap_or(&cur_name).to_string();
    let new_desc: Option<String> = match patch.description {
        None => cur_desc,
        Some(v) => v,
    };
    let new_remote: Option<String> = match patch.git_remote {
        None => cur_remote,
        Some(None) => None,
        Some(Some(_)) => normalized_remote,
    };
    let new_tags: Vec<String> = patch.stack_tags.unwrap_or(cur_tags);
    let new_refresh: Option<i32> = match patch.plan_auto_refresh_interval_secs {
        None => cur_refresh,
        Some(v) => v,
    };

    let row: ProjectRow = sqlx::query_as(
            "UPDATE tenant_projects \
             SET name = $3, description = $4, git_remote = $5, stack_tags = $6, \
                 plan_auto_refresh_interval_secs = $7, updated_at = now() \
             WHERE tenant_id = $1 AND slug = $2 \
             RETURNING id, slug::text, name, description, git_remote, stack_tags, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(&new_name)
        .bind(&new_desc)
        .bind(&new_remote)
        .bind(&new_tags)
        .bind(new_refresh)
        .fetch_one(db)
        .await
        .with_context(|| format!("update project `{slug}` for tenant `{tenant_slug}`"))?;

    Ok(Project {
        id: row.0,
        slug: row.1,
        name: row.2,
        description: row.3,
        git_remote: row.4,
        stack_tags: row.5,
        created_at: row.6,
        updated_at: row.7,
    })
}

/// Delete a project and all its items (cascade). Returns `Ok(false)` if the
/// project was not found so the route can map that to a 404.
pub async fn delete_project(db: &PgPool, tenant_slug: &str, slug: &str) -> Result<bool> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let result = sqlx::query(
        "DELETE FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(slug)
    .execute(db)
    .await
    .with_context(|| format!("delete project `{slug}` for tenant `{tenant_slug}`"))?;

    Ok(result.rows_affected() > 0)
}

/// Replace the full item list for a project atomically.
///
/// Executes DELETE + batch INSERT in a single transaction so callers
/// always see a consistent list. The `position` field is set to the
/// index of each item in the input slice (0-based), preserving curator
/// order.
///
/// Items are `(skill_slug, kind)` pairs. Duplicate pairs are silently
/// deduplicated (the PK constraint would reject them anyway).
pub async fn set_project_items(
    db: &PgPool,
    tenant_slug: &str,
    project_slug: &str,
    items: Vec<(String, String)>,
) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    // Resolve the project id within this tenant.
    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let (project_id,) =
        proj.ok_or_else(|| anyhow!("project `{project_slug}` not found for tenant `{tenant_slug}`"))?;

    let mut tx = db.begin().await?;

    sqlx::query("DELETE FROM tenant_project_items WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;

    for (position, (skill_slug, kind)) in items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO tenant_project_items (project_id, skill_slug, kind, position) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (project_id, skill_slug, kind) DO UPDATE SET position = EXCLUDED.position",
        )
        .bind(project_id)
        .bind(skill_slug)
        .bind(kind)
        .bind(position as i32)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Look up a project by its (normalized) git remote URL within a tenant.
///
/// The `git_remote` argument is normalized before the query using
/// [`normalize_git_remote`] so that SSH and HTTPS forms of the same
/// repository resolve correctly.
///
/// Returns `None` if no project is linked to that remote URL.
pub async fn resolve_project_by_remote(
    db: &PgPool,
    tenant_slug: &str,
    git_remote: &str,
) -> Result<Option<Project>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;
    let normalized = normalize_git_remote(git_remote);

    let row: Option<ProjectRow> = sqlx::query_as(
            "SELECT id, slug::text, name, description, git_remote, stack_tags, created_at, updated_at \
             FROM tenant_projects \
             WHERE tenant_id = $1 AND git_remote = $2",
        )
        .bind(tenant_id)
        .bind(&normalized)
        .fetch_optional(db)
        .await?;

    Ok(row.map(|r| Project {
        id: r.0,
        slug: r.1,
        name: r.2,
        description: r.3,
        git_remote: r.4,
        stack_tags: r.5,
        created_at: r.6,
        updated_at: r.7,
    }))
}

// ---------------------------------------------------------------------------
// Project Plans (PL server-side — Layer 2 extension)
// ---------------------------------------------------------------------------

/// A project-plan version row as returned by admin functions.
#[derive(Debug, Clone)]
pub struct Plan {
    pub id: Uuid,
    pub project_id: Uuid,
    pub version: i32,
    pub body_md: String,
    pub body_sha256: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub source_etag: Option<String>,
    pub imported_by: Option<Uuid>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub fetch_error: Option<String>,
    pub fetch_error_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Outcome of a `refresh_plan_from_source` call.
#[derive(Debug)]
pub enum RefreshOutcome {
    /// Content unchanged (hash match).
    Unchanged,
    /// New version imported. Boxed to equalise variant sizes.
    Updated(Box<Plan>),
    /// Fetch or parse failed; last-good version retained.
    Failed(String),
}

// Private type alias to avoid repeating the long tuple in multiple query_as calls.
// Columns: (id, project_id, version, body_md, body_sha256, source_type,
//           source_url, source_etag, imported_by, imported_at, status,
//           fetch_error, fetch_error_at)
type PlanRow = (
    Uuid,
    Uuid,
    i32,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
    String,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
);

fn plan_from_row(r: PlanRow) -> Plan {
    Plan {
        id: r.0,
        project_id: r.1,
        version: r.2,
        body_md: r.3,
        body_sha256: r.4,
        source_type: r.5,
        source_url: r.6,
        source_etag: r.7,
        imported_by: r.8,
        imported_at: r.9,
        status: r.10,
        fetch_error: r.11,
        fetch_error_at: r.12,
    }
}

/// Compute SHA-256 of `body_md` and return it as lowercase hex.
pub fn sha256_hex(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(body.as_bytes());
    hex::encode(hash)
}

/// Arguments for [`import_plan`]. Bundled in a struct to stay within
/// clippy's `too-many-arguments` limit (max 7) while keeping all fields
/// explicit at call sites.
pub struct ImportPlanArgs<'a> {
    pub tenant_slug: &'a str,
    pub project_slug: &'a str,
    pub body_md: &'a str,
    pub source_type: &'a str,
    pub source_url: Option<&'a str>,
    pub etag: Option<&'a str>,
    pub imported_by: Option<Uuid>,
}

/// Import a plan version for a project.
///
/// Atomically:
///   1. Compute SHA-256 of `body_md`. If it matches the current active row,
///      return that row unchanged (no-op idempotency).
///   2. INSERT a new version row (`version = MAX + 1`, `status = 'active'`).
///   3. UPDATE all previous `active` rows for this project to `superseded`.
///
/// The partial unique index `idx_project_plans_active_one` guarantees at most
/// one `active` row per project; the transaction ordering (UPDATE before
/// RETURNING) ensures no transient constraint violation.
pub async fn import_plan(db: &PgPool, args: ImportPlanArgs<'_>) -> Result<Plan> {
    let ImportPlanArgs {
        tenant_slug,
        project_slug,
        body_md,
        source_type,
        source_url,
        etag,
        imported_by,
    } = args;

    if !matches!(source_type, "file" | "url") {
        return Err(anyhow!("source_type must be 'file' or 'url'"));
    }

    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let (project_id,) = proj.ok_or_else(|| {
        anyhow!("project `{project_slug}` not found for tenant `{tenant_slug}`")
    })?;

    let new_hash = sha256_hex(body_md);

    // Dedup: if active row has same content, return it as-is.
    let existing: Option<PlanRow> = sqlx::query_as(
        "SELECT id, project_id, version, body_md, body_sha256, source_type, \
                source_url, source_etag, imported_by, imported_at, status, \
                fetch_error, fetch_error_at \
         FROM tenant_project_plans \
         WHERE project_id = $1 AND status = 'active' AND body_sha256 = $2",
    )
    .bind(project_id)
    .bind(&new_hash)
    .fetch_optional(db)
    .await?;

    if let Some(row) = existing {
        return Ok(plan_from_row(row));
    }

    let mut tx = db.begin().await?;

    // Get next version number.
    let next_version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM tenant_project_plans WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;

    // Mark any existing active row as superseded before inserting the new one
    // (avoids transient conflict on the partial unique index).
    sqlx::query(
        "UPDATE tenant_project_plans SET status = 'superseded' \
         WHERE project_id = $1 AND status = 'active'",
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?;

    // Insert new active row.
    let row: PlanRow = sqlx::query_as(
        "INSERT INTO tenant_project_plans \
           (tenant_id, project_id, version, body_md, body_sha256, \
            source_type, source_url, source_etag, imported_by, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active') \
         RETURNING id, project_id, version, body_md, body_sha256, source_type, \
                   source_url, source_etag, imported_by, imported_at, status, \
                   fetch_error, fetch_error_at",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(next_version)
    .bind(body_md)
    .bind(&new_hash)
    .bind(source_type)
    .bind(source_url)
    .bind(etag)
    .bind(imported_by)
    .fetch_one(&mut *tx)
    .await
    .with_context(|| {
        format!("insert plan v{next_version} for project `{project_slug}`")
    })?;

    tx.commit().await?;
    Ok(plan_from_row(row))
}

/// Return the active plan for a project, or `None` if no plan has been imported.
pub async fn get_active_plan(
    db: &PgPool,
    tenant_slug: &str,
    project_slug: &str,
) -> Result<Option<Plan>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let Some((project_id,)) = proj else {
        return Err(anyhow!(
            "project `{project_slug}` not found for tenant `{tenant_slug}`"
        ));
    };

    let row: Option<PlanRow> = sqlx::query_as(
        "SELECT id, project_id, version, body_md, body_sha256, source_type, \
                source_url, source_etag, imported_by, imported_at, status, \
                fetch_error, fetch_error_at \
         FROM tenant_project_plans \
         WHERE project_id = $1 AND status = 'active'",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(plan_from_row))
}

/// List plan versions for a project in descending version order.
/// Body text is intentionally included for completeness; callers that
/// only need the slim listing should project body out in the response layer.
pub async fn list_plan_versions(
    db: &PgPool,
    tenant_slug: &str,
    project_slug: &str,
    limit: i64,
) -> Result<Vec<Plan>> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let Some((project_id,)) = proj else {
        return Err(anyhow!(
            "project `{project_slug}` not found for tenant `{tenant_slug}`"
        ));
    };

    let rows: Vec<PlanRow> = sqlx::query_as(
        "SELECT id, project_id, version, body_md, body_sha256, source_type, \
                source_url, source_etag, imported_by, imported_at, status, \
                fetch_error, fetch_error_at \
         FROM tenant_project_plans \
         WHERE project_id = $1 \
         ORDER BY version DESC \
         LIMIT $2",
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(plan_from_row).collect())
}

/// Activate a historical version (revert). Atomically:
///   1. Mark the requested version as `active`.
///   2. Mark the previous `active` row as `superseded`.
pub async fn activate_plan_version(
    db: &PgPool,
    tenant_slug: &str,
    project_slug: &str,
    version: i32,
) -> Result<Plan> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let Some((project_id,)) = proj else {
        return Err(anyhow!(
            "project `{project_slug}` not found for tenant `{tenant_slug}`"
        ));
    };

    // Verify the target version exists.
    let target: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_project_plans WHERE project_id = $1 AND version = $2",
    )
    .bind(project_id)
    .bind(version)
    .fetch_optional(db)
    .await?;
    if target.is_none() {
        return Err(anyhow!(
            "plan version {version} not found for project `{project_slug}`"
        ));
    }

    let mut tx = db.begin().await?;

    // Supersede current active (if any).
    sqlx::query(
        "UPDATE tenant_project_plans SET status = 'superseded' \
         WHERE project_id = $1 AND status = 'active'",
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?;

    // Activate the target version and clear any stale fetch_error from it.
    let row: PlanRow = sqlx::query_as(
        "UPDATE tenant_project_plans \
         SET status = 'active', fetch_error = NULL, fetch_error_at = NULL \
         WHERE project_id = $1 AND version = $2 \
         RETURNING id, project_id, version, body_md, body_sha256, source_type, \
                   source_url, source_etag, imported_by, imported_at, status, \
                   fetch_error, fetch_error_at",
    )
    .bind(project_id)
    .bind(version)
    .fetch_one(&mut *tx)
    .await
    .with_context(|| {
        format!("activate plan version {version} for project `{project_slug}`")
    })?;

    tx.commit().await?;
    Ok(plan_from_row(row))
}

/// Re-fetch the plan from its source URL and import a new version if the
/// content has changed.
///
/// Returns:
///   - `Unchanged` — hash matches; nothing written.
///   - `Updated(plan)` — new version created and activated.
///   - `Failed(reason)` — network/parse error; `fetch_error` written to the
///     active row; version unchanged.
///
/// On `Failed`, the function attempts to persist the error message on the
/// current active row and then returns `Failed`. The caller should log at
/// `warn!` level; this function does not panic.
pub async fn refresh_plan_from_source(
    db: &PgPool,
    http: &reqwest::Client,
    tenant_slug: &str,
    project_slug: &str,
) -> Result<RefreshOutcome> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let proj: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tenant_projects WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .fetch_optional(db)
    .await?;
    let Some((project_id,)) = proj else {
        return Err(anyhow!(
            "project `{project_slug}` not found for tenant `{tenant_slug}`"
        ));
    };

    // Load the active plan — we need its source_url and source_type.
    let active: Option<(Uuid, Option<String>, String, String, i32)> = sqlx::query_as(
        "SELECT id, source_url, source_type, body_sha256, version \
         FROM tenant_project_plans \
         WHERE project_id = $1 AND status = 'active'",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let Some((active_id, source_url, source_type, current_hash, _version)) = active else {
        // No active plan — nothing to refresh.
        return Ok(RefreshOutcome::Unchanged);
    };

    let url = match source_url {
        None => return Ok(RefreshOutcome::Unchanged),
        Some(u) => u,
    };

    // Actually fetch.
    let fetch_result = fetch_url_as_markdown(http, &url).await;

    match fetch_result {
        Err(e) => {
            let reason = e.to_string();
            tracing::warn!(
                url = %url,
                project = %project_slug,
                error = %reason,
                "plan refresh failed; keeping last-good version"
            );
            // Persist error on active row without changing the version.
            let _ = sqlx::query(
                "UPDATE tenant_project_plans \
                 SET fetch_error = $1, fetch_error_at = now() \
                 WHERE id = $2",
            )
            .bind(&reason)
            .bind(active_id)
            .execute(db)
            .await;
            Ok(RefreshOutcome::Failed(reason))
        }
        Ok((body_md, new_etag)) => {
            let new_hash = sha256_hex(&body_md);
            if new_hash == current_hash {
                return Ok(RefreshOutcome::Unchanged);
            }
            // New content — import as a new version.
            let plan = import_plan(
                db,
                ImportPlanArgs {
                    tenant_slug,
                    project_slug,
                    body_md: &body_md,
                    source_type: &source_type,
                    source_url: Some(&url),
                    etag: new_etag.as_deref(),
                    imported_by: None, // system refresh — no user
                },
            )
            .await?;
            Ok(RefreshOutcome::Updated(Box::new(plan)))
        }
    }
}

/// Set or clear the auto-refresh interval for a project.
///
/// `secs = Some(n)` — refresh every n seconds.
/// `secs = None`    — explicit-only (clears the column).
pub async fn set_plan_auto_refresh(
    db: &PgPool,
    tenant_slug: &str,
    project_slug: &str,
    secs: Option<i32>,
) -> Result<()> {
    let tenant_id = lookup_tenant_id(db, tenant_slug).await?;

    let result = sqlx::query(
        "UPDATE tenant_projects \
         SET plan_auto_refresh_interval_secs = $3 \
         WHERE tenant_id = $1 AND slug = $2",
    )
    .bind(tenant_id)
    .bind(project_slug)
    .bind(secs)
    .execute(db)
    .await
    .with_context(|| {
        format!("set auto-refresh for project `{project_slug}` (tenant `{tenant_slug}`)")
    })?;

    if result.rows_affected() == 0 {
        return Err(anyhow!(
            "project `{project_slug}` not found for tenant `{tenant_slug}`"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP fetch helper (URL source type)
// ---------------------------------------------------------------------------

const FETCH_MAX_BYTES: usize = 5 * 1024 * 1024; // 5 MB

/// Fetch a URL and return `(body_md, etag)`.
///
/// - Only `https://` is accepted.
/// - Supported Content-Types: `text/markdown`, `text/x-markdown`, `text/plain`
///   (stored as-is) and `text/html` (converted via `htmd`).
/// - Body is capped at `FETCH_MAX_BYTES`.
pub async fn fetch_url_as_markdown(
    http: &reqwest::Client,
    url: &str,
) -> Result<(String, Option<String>)> {
    // HTTPS-only policy.
    if !url.starts_with("https://") {
        return Err(anyhow!("only https:// URLs are supported (got: {url})"));
    }

    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP GET {url}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {status} fetching {url}"));
    }

    // Capture ETag before consuming the body.
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // Check Content-Length before streaming.
    if let Some(len) = response.content_length() {
        if len as usize > FETCH_MAX_BYTES {
            return Err(anyhow!(
                "response body too large: {len} bytes (max {FETCH_MAX_BYTES})"
            ));
        }
    }

    // Determine content type (base type only, ignore params like charset).
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .split(';')
        .next()
        .unwrap_or("text/plain")
        .trim()
        .to_lowercase();

    // Stream body with a cap.
    let bytes = response.bytes().await.with_context(|| format!("read body of {url}"))?;
    if bytes.len() > FETCH_MAX_BYTES {
        return Err(anyhow!(
            "response body too large: {} bytes (max {FETCH_MAX_BYTES})",
            bytes.len()
        ));
    }

    let body_str = String::from_utf8(bytes.to_vec())
        .with_context(|| format!("UTF-8 decode body of {url}"))?;

    let body_md = match content_type.as_str() {
        "text/markdown" | "text/x-markdown" | "text/plain" => body_str,
        "text/html" => {
            htmd::convert(&body_str)
                .with_context(|| format!("HTML→Markdown conversion for {url}"))?
        }
        other => {
            return Err(anyhow!(
                "unsupported Content-Type `{other}` for {url}; \
                 expected text/markdown, text/html, or text/plain"
            ));
        }
    };

    Ok((body_md, etag))
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn sha256_hex_stable() {
        // SHA-256 of the empty string is the well-known constant.
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_hello_world() {
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_hex_different_inputs_differ() {
        assert_ne!(sha256_hex("plan A"), sha256_hex("plan B"));
    }

    #[tokio::test]
    async fn fetch_rejects_http_url() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = fetch_url_as_markdown(&client, "http://example.com/plan.md").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("https://"));
    }
}

#[cfg(test)]
mod normalize_git_remote_tests {
    use super::normalize_git_remote;

    #[test]
    fn strips_trailing_git_suffix() {
        assert_eq!(
            normalize_git_remote("https://github.com/acme/billing.git"),
            "https://github.com/acme/billing"
        );
    }

    #[test]
    fn converts_ssh_shorthand_to_https() {
        assert_eq!(
            normalize_git_remote("git@github.com:acme/billing"),
            "https://github.com/acme/billing"
        );
    }

    #[test]
    fn converts_ssh_shorthand_with_git_suffix() {
        assert_eq!(
            normalize_git_remote("git@github.com:acme/billing.git"),
            "https://github.com/acme/billing"
        );
    }

    #[test]
    fn lowercases_host_preserves_path_case() {
        // GitHub paths are case-sensitive on some platforms; we only lowercase host.
        assert_eq!(
            normalize_git_remote("https://GITHUB.COM/Acme/Billing"),
            "https://github.com/Acme/Billing"
        );
    }

    #[test]
    fn strips_trailing_slash() {
        assert_eq!(
            normalize_git_remote("https://github.com/acme/billing/"),
            "https://github.com/acme/billing"
        );
    }

    #[test]
    fn idempotent_on_already_normalized_url() {
        let url = "https://github.com/acme/billing";
        assert_eq!(normalize_git_remote(url), url);
    }

    #[test]
    fn handles_gitlab_ssh() {
        assert_eq!(
            normalize_git_remote("git@gitlab.com:acme/svc.git"),
            "https://gitlab.com/acme/svc"
        );
    }
}
