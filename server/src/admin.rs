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
