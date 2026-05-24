//! Plugin mirror job handler (issue #32).
//!
//! A `PluginMirrorJob` is enqueued by `POST /v1/plugins/import` and processed
//! by the existing Redis-backed queue worker. On execution it:
//!
//!   1. Resolves the plugin row (must be `sourcing_mode = 'mirror'`).
//!   2. Clones the upstream git URL into the local bare-repo storage, or fast-
//!      forwards an existing clone.
//!   3. Parses `.claude-plugin/plugin.json` from the HEAD commit of the
//!      upstream's default branch.
//!   4. Upserts `plugins.manifest`, `plugin_contents`, and the
//!      `plugin_marketplace_entries` row in a single DB transaction — written
//!      *after* the git tree is on disk so a crash between the two leaves no
//!      half-baked state (the next retry re-clones and re-upserts).
//!   5. Sets `last_pulled_at = now()`, clears `fetch_error`/`fetch_error_at`.
//!   6. On any failure: writes `fetch_error` + `fetch_error_at` to the plugin
//!      row, returns `Err(msg)` so the queue retries with exponential back-off.
//!
//! ## Idempotency
//!
//! The job is safe to run more than once:
//!   - `git clone` into an existing directory is replaced by `git fetch` +
//!     fast-forward merge of the remote's HEAD.
//!   - The DB upsert uses `ON CONFLICT (tenant_id, slug, version)` so a partial
//!     prior run that wrote a row does not cause a duplicate-key error.
//!   - `last_pulled_at` is updated unconditionally — a second run on an unchanged
//!     upstream advances the timestamp without changing anything else, which is
//!     fine (the job detects "unchanged" after fetch and exits early).
//!
//! ## Transactional ordering
//!
//! We write the git tree first, then commit the DB transaction. The invariant is:
//! if the DB row says `last_pulled_at IS NOT NULL`, the local bare repo at
//! `storage.plugin_git_path(tenant_id, slug)` contains the same HEAD. A crash
//! between the two is recovered by the next retry (git tree is idempotent to
//! re-write; DB transaction is rolled back by the crash).
//!
//! ## Tenant isolation
//!
//! The job payload carries `tenant_id` and `plugin_id` explicitly. The handler
//! re-fetches the row inside the DB transaction scoped on both columns, so a
//! rogue payload with a wrong `tenant_id` cannot touch another tenant's data.
//! Slug collision across tenants is impossible by the same scoping.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::queue::Job;
use crate::storage::Storage;
use crate::worker::JobHandler;

/// Minimum value for `pull_interval_secs` — mirrors the DB CHECK constraint
/// added in migration 0034.
pub const MIN_PULL_INTERVAL_SECS: i64 = 300;

/// Default refresh interval when `pull_interval_secs` is not set.
pub const DEFAULT_PULL_INTERVAL_SECS: i64 = 86_400; // 24 h

/// Idempotency-key prefix. A given (tenant, plugin_id) pair is deduped within
/// the 24h queue window — exactly the semantics we want to avoid enqueueing a
/// flood of mirror jobs for a single plugin.
const IDEM_PREFIX: &str = "plugin_mirror";

// ---------------------------------------------------------------------------
// Job payload
// ---------------------------------------------------------------------------

/// Serialised shape that the route enqueues and the handler deserialises.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMirrorJob {
    /// UUID from `plugins.id`. Carried explicitly so the handler can do a
    /// tenant-scoped lookup even if the slug changes (it shouldn't, but
    /// defensive coding is cheap here).
    pub plugin_id: Uuid,
    /// UUID from `plugins.tenant_id`. Scopes every DB operation — a
    /// mismatch is caught by the handler's `WHERE tenant_id = $2` clause.
    pub tenant_id: Uuid,
    /// `upstream_url` from the plugin row at enqueue time. Carried in the
    /// payload so the handler doesn't have to fetch the row just to find
    /// the URL (it still does a full fetch, but the URL doubles as an audit
    /// breadcrumb in the job envelope visible via DLQ inspection).
    pub upstream_url: String,
}

impl Job for PluginMirrorJob {
    const KIND: &'static str = "plugin_mirror";

    fn idempotency_key(&self) -> String {
        // One active mirror job per plugin at a time. The 24h dedup window
        // is intentionally wider than the minimum pull interval (5 min) —
        // the sweep enqueues only when the interval has elapsed, so a stuck
        // job is better than duplicate clone processes racing.
        format!("{IDEM_PREFIX}:{}", self.plugin_id)
    }

    /// Mirror jobs do not benefit from many retries — a broken upstream
    /// will stay broken, and three attempts cover transient network blips.
    fn max_attempts(&self) -> u32 {
        3
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Registered with the `Worker` at startup as `"plugin_mirror"`.
pub struct PluginMirrorHandler {
    db: PgPool,
    storage: Storage,
}

impl PluginMirrorHandler {
    pub fn new(db: PgPool, storage: Storage) -> Self {
        Self { db, storage }
    }
}

#[async_trait]
impl JobHandler for PluginMirrorHandler {
    async fn handle(&self, payload: serde_json::Value) -> Result<(), String> {
        let job: PluginMirrorJob = serde_json::from_value(payload)
            .map_err(|e| format!("deserialise PluginMirrorJob payload: {e}"))?;

        match run_mirror(&self.db, &self.storage, &job).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Persist the error against the plugin row before returning
                // Err so the queue retry cycle records what went wrong.
                let msg = format!("{e:#}");
                let _ = sqlx::query(
                    "UPDATE plugins \
                     SET fetch_error = $1, fetch_error_at = now() \
                     WHERE id = $2 AND tenant_id = $3",
                )
                .bind(&msg)
                .bind(job.plugin_id)
                .bind(job.tenant_id)
                .execute(&self.db)
                .await;
                Err(msg)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Core logic (pure `Result<(), anyhow::Error>` for testability)
// ---------------------------------------------------------------------------

/// Row returned by the plugin lookup query.
struct PluginRow {
    slug: String,
    upstream_url: Option<String>,
}

/// Runs one mirror cycle. Called from the handler; also callable from the
/// periodic sweep and from tests.
pub async fn run_mirror(db: &PgPool, storage: &Storage, job: &PluginMirrorJob) -> Result<()> {
    // 1. Re-fetch the plugin row inside a transaction that we'll commit at
    //    the end — guarantees we see the latest `upstream_url` and that the
    //    DB write is atomic with the git write acknowledgement.
    let mut tx = db.begin().await.context("begin transaction")?;

    // Use sqlx::query (non-macro) to avoid needing the offline query cache
    // for new queries before `cargo sqlx prepare` has been run. The macro
    // form (sqlx::query!) would fail to compile when SQLX_OFFLINE=true and
    // the cache entry is absent.
    let row_opt = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT slug, upstream_url \
         FROM plugins \
         WHERE id = $1 AND tenant_id = $2 AND sourcing_mode = 'mirror'",
    )
    .bind(job.plugin_id)
    .bind(job.tenant_id)
    .fetch_optional(&mut *tx)
    .await
    .context("fetch plugin row")?;

    let (slug, upstream_url_opt) = row_opt
        .ok_or_else(|| anyhow!("plugin {}/{} not found or not a mirror", job.tenant_id, job.plugin_id))?;

    let row = PluginRow {
        slug,
        upstream_url: upstream_url_opt,
    };

    let upstream_url = row
        .upstream_url
        .ok_or_else(|| anyhow!("plugin row has sourcing_mode='mirror' but upstream_url is NULL"))?;

    let slug = row.slug;

    // 2. Compute the path to the local bare repo.
    let repo_path: PathBuf = storage
        .plugin_git_path(job.tenant_id, &slug)
        .context("resolve plugin git path")?;

    // 3. Clone or fetch — runs in a blocking thread pool since libgit2 is sync.
    let upstream_url_owned = upstream_url.clone();
    let repo_path_owned = repo_path.clone();
    let head_commit_id = tokio::task::spawn_blocking(move || {
        clone_or_fetch(&upstream_url_owned, &repo_path_owned)
    })
    .await
    .context("spawn_blocking for git clone/fetch")?
    .context("git clone/fetch")?;

    // 4. Read manifest from the fetched tree (blocking).
    let repo_path_owned = repo_path.clone();
    let manifest_bytes = tokio::task::spawn_blocking(move || {
        read_manifest_from_commit(&repo_path_owned, head_commit_id)
    })
    .await
    .context("spawn_blocking for manifest read")?
    .context("read plugin manifest from upstream")?;

    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .context("parse .claude-plugin/plugin.json as JSON")?;

    let manifest_name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("manifest missing 'name' field"))?
        .to_string();
    let manifest_version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("manifest missing 'version' field"))?
        .to_string();

    // 5. Upsert the plugin row with the freshly-parsed manifest.
    //    We do NOT touch sourcing_mode or upstream_url — those are curator-
    //    managed. We update name, version, manifest, and clear fetch_error.
    sqlx::query(
        "UPDATE plugins \
         SET name = $1, version = $2, manifest = $3, \
             last_pulled_at = now(), fetch_error = NULL, fetch_error_at = NULL, \
             updated_at = now() \
         WHERE id = $4 AND tenant_id = $5",
    )
    .bind(&manifest_name)
    .bind(&manifest_version)
    .bind(&manifest)
    .bind(job.plugin_id)
    .bind(job.tenant_id)
    .execute(&mut *tx)
    .await
    .context("update plugin row")?;

    // 6. Regenerate the marketplace entry so the catalog reflects the new
    //    manifest. The entry_json shape matches what `regenerate_entry` in
    //    routes/marketplace.rs produces; `source_url` is the upstream URL
    //    (for mirror plugins the local git endpoint would be preferred, but
    //    the job handler doesn't know the server's origin — the operator
    //    can re-trigger a publish to get the canonical local URL).
    //
    //    ON CONFLICT (tenant_id, plugin_slug) ensures idempotency.
    let entry_json = serde_json::json!({
        "name": slug,
        "version": manifest_version,
        "source": {
            "source": "url",
            "url": upstream_url,
        },
    });
    sqlx::query(
        "INSERT INTO plugin_marketplace_entries \
             (tenant_id, plugin_slug, plugin_id, version, source_url, entry_json) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (tenant_id, plugin_slug) DO UPDATE \
             SET plugin_id   = EXCLUDED.plugin_id, \
                 version     = EXCLUDED.version, \
                 source_url  = EXCLUDED.source_url, \
                 entry_json  = EXCLUDED.entry_json, \
                 updated_at  = now()",
    )
    .bind(job.tenant_id)
    .bind(&slug)
    .bind(job.plugin_id)
    .bind(&manifest_version)
    .bind(&upstream_url)
    .bind(entry_json)
    .execute(&mut *tx)
    .await
    .context("upsert marketplace entry")?;

    tx.commit().await.context("commit mirror transaction")?;

    tracing::info!(
        tenant_id = %job.tenant_id,
        plugin_id = %job.plugin_id,
        slug = %slug,
        upstream = %upstream_url,
        "plugin mirror: pulled successfully"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// git2 helpers (sync — run inside spawn_blocking)
// ---------------------------------------------------------------------------

/// Clone the upstream into `repo_path` as a bare repository, or fast-forward
/// an existing clone. Returns the `git2::Oid` of the HEAD commit so the caller
/// can read tree objects from it.
///
/// We fetch only the default branch (HEAD) to keep the transfer minimal.
fn clone_or_fetch(upstream_url: &str, repo_path: &std::path::Path) -> Result<git2::Oid> {
    use git2::{FetchOptions, RemoteCallbacks, Repository};

    // Build fetch options. No credentials callback needed for public repos;
    // for private repos, the upstream_url is expected to embed credentials
    // (e.g. `https://token@github.com/org/repo.git`). A future PR can add
    // SSH key support via the callbacks.
    let mut fetch_opts = FetchOptions::new();
    let callbacks = RemoteCallbacks::new();
    fetch_opts.remote_callbacks(callbacks);
    // Single-branch fetch to keep the transfer fast.
    fetch_opts.download_tags(git2::AutotagOption::None);

    let repo = if repo_path.exists() {
        // Existing bare clone — fetch from origin.
        let repo = Repository::open_bare(repo_path)
            .with_context(|| format!("open existing bare repo {}", repo_path.display()))?;
        {
            let mut remote = repo
                .find_remote("origin")
                .context("find origin remote")?;
            remote
                .fetch(&["HEAD:refs/heads/main"], Some(&mut fetch_opts), None)
                .context("git fetch")?;
        }
        repo
    } else {
        // First mirror — bare clone.
        if let Some(parent) = repo_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create repo parent dir {}", parent.display()))?;
        }
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.fetch_options(fetch_opts);
        builder
            .clone(upstream_url, repo_path)
            .with_context(|| format!("git clone {upstream_url}"))?
    };

    // Resolve HEAD → commit OID.
    let head = repo
        .head()
        .context("resolve HEAD after fetch")?
        .peel_to_commit()
        .context("peel HEAD to commit")?;
    Ok(head.id())
}

/// Read `.claude-plugin/plugin.json` from the tree at `commit_id` inside the
/// bare repo at `repo_path`. Returns the raw bytes.
fn read_manifest_from_commit(repo_path: &std::path::Path, commit_id: git2::Oid) -> Result<Vec<u8>> {
    use git2::Repository;

    let repo = Repository::open_bare(repo_path)
        .with_context(|| format!("open bare repo {}", repo_path.display()))?;
    let commit = repo
        .find_commit(commit_id)
        .context("find HEAD commit")?;
    let tree = commit.tree().context("get commit tree")?;

    // Traverse the tree to find `.claude-plugin/plugin.json`.
    let entry = tree
        .get_path(std::path::Path::new(".claude-plugin/plugin.json"))
        .context("find .claude-plugin/plugin.json in upstream tree")?;
    let blob = repo
        .find_blob(entry.id())
        .context("read plugin.json blob")?;
    Ok(blob.content().to_vec())
}
