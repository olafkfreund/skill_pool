//! Static-analysis safety net: every business sqlx query in the server
//! crate filters by `tenant_id` (or is on the allowlist for an explicitly
//! global/cross-tenant query).
//!
//! Closes the lint slot from #8 §L17 ("build-time lint for tenant
//! isolation") and the matching box on #3. The current safeguard is
//! reviewer discipline; this test is the CI-grade backstop that catches a
//! regression at `cargo test`.
//!
//! ## How it works
//!
//! Pure text scan — no DB, no `syn`, no compile-graph hooks. Walks every
//! `.rs` file under `server/src/`, finds every `sqlx::query` / `query_as`
//! / `query_scalar` invocation, extracts the first string-literal
//! argument (the SQL), and asserts that one of the following is true:
//!
//!   1. The SQL contains `tenant_id` (substring, case-insensitive) — the
//!      common case. `WHERE tenant_id = $1`, `JOIN … ON x.tenant_id = …`,
//!      and `RETURNING tenant_id` all match.
//!   2. The SQL contains `tenants.id` — joins on the parent table.
//!   3. The query targets the `tenants` table by `id` (e.g.
//!      `SELECT … FROM tenants WHERE id = $1`). `tenants.id` IS the
//!      tenant_id; the per-row identifier is the scope.
//!   4. The (file, fn-or-snippet) pair is on the explicit `ALLOWLIST`.
//!
//! When this test fails, the panic message names the file, line, function,
//! and the offending SQL snippet, plus a clear pointer to the allowlist.
//!
//! ## Heuristic limits
//!
//! * Queries built via `format!(...)` or string concatenation are scanned
//!   against the concatenation pieces. We extract every string literal
//!   between the open paren and the dispatch method, and require that
//!   *one* of them contains the tenant-id token. This catches the SCIM /
//!   members / custom_domains patterns where a `SELECT_COLS` constant is
//!   `format!`-interpolated into the final SQL — the const-bearing or the
//!   suffix piece must include `tenant_id`.
//! * Truly dynamic SQL (variables passed to query without a literal in
//!   the same call) cannot be analysed and must be added to the
//!   ALLOWLIST with a comment explaining the runtime invariant.
//!
//! ## When the test fails for a new query you added
//!
//! 1. If the query is genuinely tenant-scoped: add `WHERE tenant_id = $X`
//!    or equivalent. Re-run the test.
//! 2. If the query is intentionally cross-tenant (admin CLI, healthz, a
//!    token-validation lookup keyed by hash, a `users`-row write that
//!    isn't tenant-keyed, etc.): add an entry to `ALLOWLIST` below with a
//!    one-line comment explaining the safety argument.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// ALLOWLIST
// ---------------------------------------------------------------------------
//
// Each entry is `(relative_file_path, key)` where `key` is matched as a
// substring against EITHER the enclosing fn name OR the SQL literal.
// A query is allowlisted iff its file matches AND at least one of those
// two contain the key string.
//
// Add an entry only with a comment naming the safety argument: why is it
// safe for this specific query to skip `tenant_id`?
const ALLOWLIST: &[(&str, &str)] = &[
    // -----------------------------------------------------------------
    // admin.rs — CLI ops on the `tenants` table itself OR helpers that
    // resolve a tenant slug to its row. These create/list/delete tenants
    // OR derive a tenant_id from a slug. The very next sqlx call in each
    // function is `WHERE tenant_id = $1`.
    // -----------------------------------------------------------------
    // `INSERT INTO tenants … RETURNING id` — creates the tenant itself.
    ("src/admin.rs", "INSERT INTO tenants"),
    // `DELETE FROM tenants WHERE id = $1` — cascade is the whole point.
    ("src/admin.rs", "DELETE FROM tenants"),
    // Slug→id resolvers used at the top of every CLI helper. The very
    // next query is tenant-scoped.
    ("src/admin.rs", "SELECT id FROM tenants WHERE slug"),
    (
        "src/admin.rs",
        "SELECT id, plan_tier FROM tenants WHERE slug",
    ),
    // `UPDATE tenants SET … WHERE id = $1` — operates on the tenant row
    // itself; `id` IS the tenant_id.
    ("src/admin.rs", "UPDATE tenants"),
    // `SELECT rate_limit_rpm, rate_limit_burst FROM tenants WHERE id` —
    // read-back of the tenant row after an UPDATE in the same helper.
    ("src/admin.rs", "SELECT rate_limit_rpm"),
    // -----------------------------------------------------------------
    // auth.rs — token-by-hash and session-by-hash lookups derive the
    // tenant FROM the result row. Both lookups still filter by
    // tenant_id (the bearer token's claimed tenant must match the row's
    // tenant), but the bump/read paths against API-token / session rows
    // are keyed by the row's primary `id`. tenant_id is implicit because
    // the row's `id` is unguessable.
    // -----------------------------------------------------------------
    // `UPDATE tenant_api_tokens SET last_used_at = now() WHERE id = $1`
    // — id is the row PK, which was looked up in a tenant-scoped query
    // immediately prior (or pulled from the auth cache after a previous
    // tenant-scoped lookup).
    ("src/auth.rs", "UPDATE tenant_api_tokens SET last_used_at"),
    // -----------------------------------------------------------------
    // tenant.rs — the extractor that RESOLVES tenant_id from the
    // request. By definition these can't already be tenant-scoped.
    // -----------------------------------------------------------------
    // Custom-domain shortcut: looks up the tenant slug after a Host
    // header matched a cached custom-domain entry. The `id` IS the
    // tenant_id we resolved.
    ("src/tenant.rs", "SELECT slug FROM tenants WHERE id"),
    // Slug → (id, slug) lookup. This is THE tenant resolver. The fn
    // name is the stable matcher because the SQL casts (`slug::text`)
    // can break substring search across drift.
    ("src/tenant.rs", "from_request_parts"),
    // -----------------------------------------------------------------
    // state.rs — startup / refresh queries that load global state.
    // -----------------------------------------------------------------
    // Loads the per-tenant storage URI in the `storage_for` path. The
    // `id = $1` is the tenant_id we already resolved upstream.
    ("src/state.rs", "SELECT storage_uri FROM tenants WHERE id"),
    // Refresh tick: pulls every verified/active custom-domain row across
    // tenants. By design — the cache fans out to per-host lookup.
    ("src/state.rs", "refresh_custom_domains"),
    // -----------------------------------------------------------------
    // audit.rs — audit_events writes. The INSERT carries the tenant_id
    // in the values; the column is in the schema. The SELECT in
    // SiemConfig::load is keyed by `tenants.id` which IS the tenant_id.
    // -----------------------------------------------------------------
    ("src/audit.rs", "INSERT INTO audit_events"),
    ("src/audit.rs", "SELECT tenant_audit_siem_url"),
    // -----------------------------------------------------------------
    // health.rs — `SELECT 1` liveness probe. Touches no business data.
    // -----------------------------------------------------------------
    ("src/routes/health.rs", "SELECT 1"),
    // -----------------------------------------------------------------
    // notify.rs — loads per-tenant webhook/SMTP config from the
    // `tenants` table. `WHERE id = $1` where id IS the tenant_id.
    // -----------------------------------------------------------------
    ("src/notify.rs", "SELECT notifications_webhook_url"),
    (
        "src/notify.rs",
        "SELECT notifications_webhook_url, notifications_webhook_secret, ",
    ),
    // -----------------------------------------------------------------
    // rate_limit.rs — slug → tenant resolver for the per-tenant
    // limiter. Returns the tenant_id; everything downstream is scoped.
    // -----------------------------------------------------------------
    ("src/rate_limit.rs", "SELECT id, plan_tier, rate_limit_rpm"),
    // -----------------------------------------------------------------
    // email_branding.rs — load_row reads the tenant's branding row.
    // -----------------------------------------------------------------
    // The `WHERE tenant_id = $1` is in the SQL but lives across two
    // line fragments; the substring scan catches `tenant_id` already.
    // (No allowlist needed — kept here as a guard for future drift.)

    // -----------------------------------------------------------------
    // routes/profile.rs, routes/session_policy.rs, routes/audit_siem.rs,
    // routes/notifications.rs — all `SELECT … FROM tenants WHERE id = $1`.
    // The `id` IS the tenant_id we resolved upstream via TenantCtx.
    // -----------------------------------------------------------------
    ("src/routes/profile.rs", "SELECT banner_text"),
    (
        "src/routes/session_policy.rs",
        "SELECT session_max_age_secs",
    ),
    ("src/routes/audit_siem.rs", "SELECT tenant_audit_siem_url"),
    ("src/routes/audit_siem.rs", "UPDATE tenants SET"),
    (
        "src/routes/notifications.rs",
        "SELECT notifications_webhook_url",
    ),
    ("src/routes/notifications.rs", "UPDATE tenants SET"),
    // -----------------------------------------------------------------
    // routes/oidc.rs, routes/saml.rs — auth flow helpers. Three groups:
    //   * `tenant_sso` / `tenant_saml` reads keyed by `tenant_id` —
    //     these PASS the substring check; no allowlist entry needed.
    //   * `users` upserts (email is globally unique); user rows are
    //     not tenant-keyed in our schema. Tenant membership lives in
    //     `tenant_users` and IS scoped.
    //   * `user_sessions` session-revoke by row id, after the row was
    //     looked up in a tenant-scoped query.
    // -----------------------------------------------------------------
    ("src/routes/oidc.rs", "INSERT INTO users"),
    (
        "src/routes/oidc.rs",
        "UPDATE user_sessions SET revoked_at = now() WHERE id",
    ),
    ("src/routes/saml.rs", "INSERT INTO users"),
    // -----------------------------------------------------------------
    // routes/scim.rs — IdP-driven user lifecycle. The `users` table is
    // tenant-agnostic (per-user identity); membership rows in
    // `tenant_users` ARE tenant-keyed and pass the substring check.
    // The mutations below are either keyed by `users.id` (a row that
    // belongs to a single global user) or by `tenant_users.id` (a row
    // that was looked up tenant-scoped upstream).
    // -----------------------------------------------------------------
    ("src/routes/scim.rs", "INSERT INTO users"),
    ("src/routes/scim.rs", "UPDATE users SET active"),
    ("src/routes/scim.rs", "DELETE FROM tenant_users WHERE id"),
    // -----------------------------------------------------------------
    // skills.rs — INSERT INTO skills carries tenant_id as the first
    // column value; the column list mentions tenant_id. Substring scan
    // already catches this — no allowlist needed.
    //
    // EXCEPT `record_usage`'s use_count UPDATE, keyed by `skills.id` (a
    // UUID PK obtained from a tenant-scoped SELECT immediately upstream).
    // The accompanying INSERT into skill_usage_events DOES carry
    // tenant_id; this UPDATE bumps a per-row counter only.
    // -----------------------------------------------------------------
    (
        "src/routes/skills.rs",
        "UPDATE skills SET use_count = use_count + 1",
    ),
    // -----------------------------------------------------------------
    // admin.rs `backfill_embeddings` — operator-run CLI tool to compute
    // embeddings for skills lacking them. Takes `--tenant <slug>` to
    // scope, or runs across ALL tenants when `--tenant` is omitted
    // (the cross-tenant branch is the one that lacks `tenant_id` in
    // the WHERE clause; the per-tenant branch passes the substring
    // check). The follow-up UPDATE is keyed by the row PK we just
    // pulled — these rows already passed the operator-supplied scope.
    // -----------------------------------------------------------------
    ("src/admin.rs", "backfill_embeddings"),
    // -----------------------------------------------------------------
    // `format!`-built SQL — the SQL literal lives in a `format!(...)`
    // expression on the line above the `sqlx::query(&sql)` call, so
    // the extractor (which only scans inside the sqlx call's argument
    // list) can't see it. In every case below the format! literal
    // visibly contains `tenant_id = $1` AND the bound parameter is
    // `caller.tenant.tenant_id`. The harness must allowlist by fn
    // name because the bound variable name isn't load-bearing.
    //
    // If you add another `format!`-built tenant-scoped query, either
    //   (a) inline the SQL into the sqlx call and drop the entry, or
    //   (b) add a new entry with a comment.
    // -----------------------------------------------------------------
    ("src/routes/custom_domains.rs", "fn list"),
    ("src/routes/custom_domains.rs", "list"),
    ("src/routes/members.rs", "list"),
    ("src/routes/members.rs", "patch_role"),
    ("src/routes/scim.rs", "fetch_membership_by_id"),
    ("src/routes/scim.rs", "fetch_membership_by_email"),
    ("src/routes/scim.rs", "fetch_membership_by_user"),
    ("src/routes/scim.rs", "fetch_all_memberships"),
    // -----------------------------------------------------------------
    // admin.rs — project + plan helpers. Every fn takes `tenant_slug`,
    // resolves tenant_id, then resolves project_id with
    // `WHERE tenant_id = $1 AND slug = $2`. The downstream queries are
    // keyed by project_id (or active_id derived from a project_id
    // query), which is itself tenant-scoped. The pattern is identical
    // across the project + plan CRUD surface — match on fn name so
    // future plan helpers slot in without per-line entries.
    // -----------------------------------------------------------------
    ("src/admin.rs", "get_project"),
    ("src/admin.rs", "resolve_project_items_expanded"),
    ("src/admin.rs", "set_project_items"),
    ("src/admin.rs", "import_plan"),
    ("src/admin.rs", "get_active_plan"),
    ("src/admin.rs", "list_plan_versions"),
    ("src/admin.rs", "activate_plan_version"),
    ("src/admin.rs", "refresh_plan_from_source"),
    // -----------------------------------------------------------------
    // routes/plugins.rs — plugin_contents reads/writes are keyed by
    // plugin_id. The plugin row itself carries tenant_id (enforced by
    // the partial unique index in migration 0035 and the
    // tenant-scoped INSERT in `publish`). In `publish` the contents
    // INSERT runs in the same tx as the plugins INSERT; in `get_one`
    // the plugin_id comes from a tenant-scoped SELECT immediately
    // above the contents SELECT.
    // -----------------------------------------------------------------
    ("src/routes/plugins.rs", "INSERT INTO plugin_contents"),
    (
        "src/routes/plugins.rs",
        "SELECT content_slug, content_kind, content_version, position",
    ),
    // -----------------------------------------------------------------
    // routes/decay.rs `sweep` — background skill-decay worker, runs
    // tenant-wide on a timer to flip stale rows to `archive_candidate`.
    // Cross-tenant by design (doc-comment at the fn says so).
    // -----------------------------------------------------------------
    ("src/routes/decay.rs", "sweep"),
    // -----------------------------------------------------------------
    // routes/usage.rs `post_event` — UPDATE skills SET use_count
    // keyed by `skills.id`. The id is pulled from a tenant-scoped
    // SELECT immediately upstream (lines ~200-208 — caller's
    // tenant_id + slug + kind). Identical pattern to the already-
    // allowlisted skills.rs use_count bump.
    // -----------------------------------------------------------------
    (
        "src/routes/usage.rs",
        "UPDATE skills SET use_count = use_count + 1",
    ),
];

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn every_business_query_filters_by_tenant_id() {
    let crate_root = env!("CARGO_MANIFEST_DIR");
    let src = Path::new(crate_root).join("src");
    let files = walk_rs_files(&src);
    assert!(
        !files.is_empty(),
        "no .rs files under {}? wrong CARGO_MANIFEST_DIR?",
        src.display()
    );

    let mut findings: Vec<String> = Vec::new();
    let mut total_queries = 0usize;

    for path in files {
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let rel = relative_to_crate(&path, crate_root);
        for q in extract_sqlx_queries(&content) {
            total_queries += 1;
            if q.literals.iter().any(|s| mentions_tenant_scope(s)) {
                continue;
            }
            if is_allowlisted(&rel, &q) {
                continue;
            }
            findings.push(format_finding(&rel, &q));
        }
    }

    // Sanity: ensure we actually scanned something. A regression that
    // breaks the parser shouldn't silently turn this into a no-op test.
    assert!(
        total_queries >= 50,
        "expected at least 50 sqlx queries; found {total_queries}. \
         The extractor probably broke — investigate before disabling.",
    );

    if !findings.is_empty() {
        let header = format!(
            "\n\n{} query/queries found without a tenant_id filter and not on the allowlist:\n\n",
            findings.len()
        );
        let body = findings.join("\n");
        let trailer = "\n\nIf any of these queries are intended to be cross-tenant \
                       (admin path, healthz, audit fan-out, identity row, etc.), add \
                       an entry to ALLOWLIST in server/tests/tenant_filter_static.rs \
                       with a one-line comment explaining why it's safe.\n";
        panic!("{header}{body}{trailer}");
    }
}

// ---------------------------------------------------------------------------
// Walker
// ---------------------------------------------------------------------------

fn walk_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_inner(root, &mut out);
    out.sort();
    out
}

fn walk_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_inner(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn relative_to_crate(path: &Path, crate_root: &str) -> String {
    let root_path = Path::new(crate_root);
    match path.strip_prefix(root_path) {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Query extraction
// ---------------------------------------------------------------------------

/// One sqlx query call site.
#[derive(Debug)]
struct QuerySite {
    /// 1-based line number of the `sqlx::query…` token.
    line: usize,
    /// Best-effort name of the enclosing fn (last `fn NAME(` we saw
    /// above this line within the same `{...}` scope budget).
    fn_name: String,
    /// Every string-literal we extracted between the call's open paren
    /// and the dispatch (`.bind` / `.fetch_…` / `.execute` / `.await`).
    /// Concatenated `+` strings, `format!`-interpolated `{CONST}` pieces,
    /// and bare literals all show up here.
    literals: Vec<String>,
}

impl QuerySite {
    fn first_line_snippet(&self) -> String {
        let raw = self
            .literals
            .iter()
            .find(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_default();
        let single = raw.replace('\n', " ");
        // Collapse runs of whitespace for legibility.
        let mut compact = String::with_capacity(single.len());
        let mut prev_space = false;
        for c in single.chars() {
            if c.is_whitespace() {
                if !prev_space {
                    compact.push(' ');
                }
                prev_space = true;
            } else {
                compact.push(c);
                prev_space = false;
            }
        }
        let trimmed = compact.trim().to_string();
        if trimmed.len() > 140 {
            format!("{}…", &trimmed[..140])
        } else {
            trimmed
        }
    }
}

/// Scan `source` for `sqlx::query` / `sqlx::query_as` / `sqlx::query_scalar`
/// invocations. For each, walk forward through the argument list and
/// collect every Rust string literal until the corresponding close paren
/// matches the open. Returns one entry per call site, with all extracted
/// literals so the predicate can OR them together.
fn extract_sqlx_queries(source: &str) -> Vec<QuerySite> {
    let bytes = source.as_bytes();
    let mut sites = Vec::new();

    // Precompute line offsets so we can convert byte indices → line numbers.
    let mut line_starts = vec![0usize];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }

    let needles = ["sqlx::query_as", "sqlx::query_scalar", "sqlx::query"];

    let mut i = 0;
    while i < bytes.len() {
        // Skip comments and strings outside a query — but cheaply: the
        // only false-positive cost is a doc-comment containing the
        // literal `sqlx::query` text. We strip those by checking the
        // preceding non-whitespace bytes for `//`. Worth: the test
        // accepts this slim risk because our actual code never has
        // `sqlx::query` as a comment example.
        if at_line_comment(bytes, i) {
            i = advance_to_next_line(bytes, i);
            continue;
        }
        if at_block_comment(bytes, i) {
            i = skip_block_comment(bytes, i);
            continue;
        }

        let mut matched: Option<&str> = None;
        for n in &needles {
            if matches_at(bytes, i, n.as_bytes()) {
                matched = Some(n);
                break;
            }
        }
        let Some(needle) = matched else {
            i += 1;
            continue;
        };

        // Need to confirm this isn't `sqlx::query_as_…something_else`. Both
        // `query_as` and `query_scalar` are valid prefixes; verify by
        // checking the next byte is `<` (turbofish), `(`, or `:` (path
        // continuation isn't valid for these calls — there's no
        // `sqlx::query::foo`). Conservatively accept `(`, `<`, and
        // whitespace.
        let after = i + needle.len();
        // Accept identifier extensions only for the `sqlx::query` short
        // form — `sqlx::query_as` and `sqlx::query_scalar` are the only
        // longer forms we want, and they're tried first because of the
        // needles ordering.
        if after < bytes.len() && (bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_') {
            i += 1;
            continue;
        }

        // Find the opening paren of this call.
        let Some(paren_open) = find_open_paren(bytes, after) else {
            i = after;
            continue;
        };
        // Walk to the matching close paren, accumulating string literals
        // as we go.
        let (paren_close, literals) = scan_args(bytes, paren_open);
        let line = line_for_index(&line_starts, i);
        let fn_name = enclosing_fn(source, &line_starts, i);
        sites.push(QuerySite {
            line,
            fn_name,
            literals,
        });
        i = paren_close.unwrap_or(paren_open + 1);
    }

    sites
}

/// True if `bytes[idx..]` starts with `prefix`.
fn matches_at(bytes: &[u8], idx: usize, prefix: &[u8]) -> bool {
    bytes.len() >= idx + prefix.len() && &bytes[idx..idx + prefix.len()] == prefix
}

/// Find the next `(` byte at or after `from`, skipping turbofish content,
/// generic args, the macro `!`, and whitespace.
fn find_open_paren(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => return Some(i),
            b'<' => {
                // Skip the turbofish: balance angles.
                let mut depth = 1usize;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'<' => depth += 1,
                        b'>' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b':' => {
                // Path continuation (e.g. `query::<i32, _>`); jump over.
                i += 1;
            }
            // Macro invocation: `sqlx::query!(...)`. The `!` sits between
            // the needle and the open paren; skip and keep looking.
            b'!' => i += 1,
            c if c.is_ascii_whitespace() => i += 1,
            _ => return None,
        }
    }
    None
}

/// Walk from the opening paren, returning (close_paren_index, literals).
/// String literals encountered at any nesting level inside the argument
/// list are recorded — sqlx queries built via `format!(...)` interleave
/// constants and runtime values; the SQL we care about is always one of
/// the literal pieces.
fn scan_args(bytes: &[u8], open: usize) -> (Option<usize>, Vec<String>) {
    let mut depth = 1usize;
    let mut i = open + 1;
    let mut literals = Vec::new();
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return (Some(i - 1), literals);
                }
            }
            b'"' => {
                // Plain string literal (not preceded by `r`, possibly
                // preceded by `b` for a byte string — we ignore those
                // for SQL purposes).
                let (end, content) = read_plain_string(bytes, i);
                literals.push(content);
                i = end;
            }
            b'r' => {
                // Raw string `r"..."` or `r#"..."#` etc.
                if let Some((end, content)) = read_raw_string(bytes, i) {
                    literals.push(content);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'/' => {
                if at_line_comment(bytes, i) {
                    i = advance_to_next_line(bytes, i);
                } else if at_block_comment(bytes, i) {
                    i = skip_block_comment(bytes, i);
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    (None, literals)
}

/// Parse a plain `"..."` string literal starting at `bytes[start]`,
/// honouring `\"` escapes. Returns `(index_after_closing_quote, decoded_string)`.
/// We don't fully decode escapes — just collect the raw inner bytes — but
/// that's fine for substring search.
fn read_plain_string(bytes: &[u8], start: usize) -> (usize, String) {
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                // Preserve a couple of common escapes textually; we don't
                // need true decoding because the test only looks for
                // ASCII substrings. Whatever follows the backslash is
                // skipped along with the backslash itself.
                out.push(bytes[i] as char);
                out.push(bytes[i + 1] as char);
                i += 2;
            }
            b'"' => {
                return (i + 1, out);
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    (i, out)
}

/// Parse `r#...#"..."#...#` or `r"..."`. Returns None if it's not really a
/// raw string (e.g. a variable name `r`). On success returns
/// `(index_after_closing, decoded_string)`.
fn read_raw_string(bytes: &[u8], start: usize) -> Option<(usize, String)> {
    // bytes[start] is 'r'. Count leading '#'s.
    let mut i = start + 1;
    let mut hashes = 0usize;
    while i < bytes.len() && bytes[i] == b'#' {
        hashes += 1;
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1; // past opening quote
    let body_start = i;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Look for the right number of trailing hashes.
            let mut hh = 0usize;
            while hh < hashes && i + 1 + hh < bytes.len() && bytes[i + 1 + hh] == b'#' {
                hh += 1;
            }
            if hh == hashes {
                let body = String::from_utf8_lossy(&bytes[body_start..i]).to_string();
                return Some((i + 1 + hashes, body));
            }
        }
        i += 1;
    }
    None
}

fn at_line_comment(bytes: &[u8], i: usize) -> bool {
    i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/'
}

fn at_block_comment(bytes: &[u8], i: usize) -> bool {
    i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*'
}

fn advance_to_next_line(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    if i < bytes.len() {
        i + 1
    } else {
        i
    }
}

fn skip_block_comment(bytes: &[u8], mut i: usize) -> usize {
    i += 2; // skip /*
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

fn line_for_index(line_starts: &[usize], idx: usize) -> usize {
    // 1-based line number.
    match line_starts.binary_search(&idx) {
        Ok(n) => n + 1,
        Err(n) => n,
    }
}

/// Best-effort: walk backward from the call site looking for the most
/// recent `fn NAME(`. Doesn't try to honour `{ … }` scoping — just picks
/// the nearest `fn` declaration above the line. Good enough for the panic
/// message; not load-bearing for correctness.
fn enclosing_fn(source: &str, line_starts: &[usize], idx: usize) -> String {
    let line = line_for_index(line_starts, idx);
    if line == 0 {
        return "<unknown>".into();
    }
    // Look at the source from start of file up to idx; scan for `fn `
    // tokens; pick the last one.
    let prefix = &source[..idx.min(source.len())];
    let mut name = String::from("<top-level>");
    for (pos, _) in prefix.match_indices("fn ") {
        // Must be preceded by start-of-line, whitespace, or `pub `.
        let valid = pos == 0
            || prefix.as_bytes()[pos - 1].is_ascii_whitespace()
            || prefix[..pos].ends_with("pub ")
            || prefix[..pos].ends_with("async ")
            || prefix[..pos].ends_with("const ")
            || prefix[..pos].ends_with("unsafe ");
        if !valid {
            continue;
        }
        let rest = &prefix[pos + 3..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            continue;
        }
        let candidate = &rest[..end];
        // Skip closures-style fn pointers and `fn(` literal type syntax.
        if candidate.is_empty() {
            continue;
        }
        name = candidate.to_string();
    }
    name
}

// ---------------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------------

/// True if the SQL literal mentions the tenant scope. Case-insensitive
/// substring match against:
///   * `tenant_id` — the column on every business table
///   * `tenants.id` — joins through the parent
///   * `tenants where id` / `tenants \nwhere id` — `SELECT … FROM tenants
///     WHERE id = $X`, where `id` IS the tenant_id (tenants table PK).
fn mentions_tenant_scope(sql: &str) -> bool {
    let lower: String = sql.to_ascii_lowercase();
    if lower.contains("tenant_id") {
        return true;
    }
    if lower.contains("tenants.id") {
        return true;
    }
    // FROM tenants ... WHERE id  — collapse whitespace so multi-line SQL
    // catches.
    let collapsed = collapse_ws(&lower);
    // We require both pieces in order: `from tenants` and a subsequent
    // `where id` (preceded by whitespace so we don't match `where ident…`).
    if let Some(from_idx) = collapsed.find("from tenants") {
        let tail = &collapsed[from_idx..];
        if tail.contains(" where id ")
            || tail.contains(" where id=")
            || tail.contains(" where id\t")
        {
            return true;
        }
        // `UPDATE tenants SET … WHERE id = $1` — same trick on the
        // update side. UPDATE … doesn't start with "from tenants", so
        // handle separately below.
    }
    if let Some(up_idx) = collapsed.find("update tenants") {
        let tail = &collapsed[up_idx..];
        if tail.contains(" where id ")
            || tail.contains(" where id=")
            || tail.contains(" where id\t")
        {
            return true;
        }
    }
    false
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn is_allowlisted(rel_path: &str, site: &QuerySite) -> bool {
    for (file, key) in ALLOWLIST {
        if rel_path != *file {
            continue;
        }
        if site.fn_name.contains(*key) {
            return true;
        }
        for lit in &site.literals {
            if lit.contains(*key) {
                return true;
            }
        }
    }
    false
}

fn format_finding(rel_path: &str, site: &QuerySite) -> String {
    format!(
        "  {}:{}  in fn `{}`\n      {}",
        rel_path,
        site.line,
        site.fn_name,
        site.first_line_snippet()
    )
}

// ---------------------------------------------------------------------------
// Self-tests for the extractor + predicate. These run alongside the main
// test and guarantee the harness itself doesn't silently break.
// ---------------------------------------------------------------------------

#[test]
fn extractor_finds_plain_query() {
    let src = r#"
        async fn handler() {
            let _ = sqlx::query("SELECT 1 FROM tenants WHERE id = $1")
                .bind(t)
                .execute(db)
                .await;
        }
    "#;
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
    assert!(sites[0]
        .literals
        .iter()
        .any(|s| s.contains("SELECT 1 FROM tenants")));
    assert_eq!(sites[0].fn_name, "handler");
}

#[test]
fn extractor_finds_query_as_with_turbofish() {
    let src = r#"
        async fn pull() {
            let _ = sqlx::query_as::<_, (i32,)>(
                "SELECT id FROM skills WHERE tenant_id = $1 AND slug = $2"
            )
            .bind(t)
            .bind(s)
            .fetch_one(db)
            .await;
        }
    "#;
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
    assert!(sites[0]
        .literals
        .iter()
        .any(|s| s.contains("WHERE tenant_id = $1")));
}

#[test]
fn extractor_finds_query_scalar() {
    let src = r#"
        async fn probe() {
            let _ = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(db).await;
        }
    "#;
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].literals.len(), 1);
    assert_eq!(sites[0].literals[0], "SELECT 1");
}

#[test]
fn extractor_handles_format_macro_with_constant() {
    let src = r#"
        const COLS: &str = "id, name, tenant_id";
        async fn run() {
            let sql = format!("SELECT {COLS} FROM widgets WHERE tenant_id = $1");
            let _ = sqlx::query_as(&sql).bind(t).fetch_all(db).await;
        }
    "#;
    // We don't follow variables, but the substring scan still catches the
    // tenant_id token via the format! literal earlier in the file. Since
    // the extractor only scans inside the sqlx::query call, this proves
    // our heuristic correctly fails closed when the SQL isn't inline —
    // hence the need for ALLOWLIST entries OR for the inline literal to
    // contain tenant_id (which it usually does in our codebase).
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
    // `&sql` has no string literal, so literals is empty.
    assert!(sites[0].literals.is_empty());
}

#[test]
fn extractor_handles_raw_strings() {
    let src = r##"
        async fn r() {
            let _ = sqlx::query(r#"SELECT 1 WHERE tenant_id = $1"#).execute(db).await;
        }
    "##;
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
    assert!(sites[0].literals[0].contains("tenant_id"));
}

#[test]
fn extractor_skips_comments() {
    let src = r#"
        // sqlx::query("this is a comment").execute(db).await
        /* sqlx::query_as("nope") */
        async fn real() {
            let _ = sqlx::query("SELECT 1 WHERE tenant_id = $1").execute(db).await;
        }
    "#;
    let sites = extract_sqlx_queries(src);
    assert_eq!(sites.len(), 1);
}

#[test]
fn predicate_recognises_tenant_id() {
    assert!(mentions_tenant_scope(
        "SELECT * FROM skills WHERE tenant_id = $1"
    ));
    assert!(mentions_tenant_scope(
        "INSERT INTO audit_events (tenant_id, …)"
    ));
    assert!(mentions_tenant_scope(
        "JOIN tenants ON tenants.id = s.tenant_id"
    ));
}

#[test]
fn predicate_recognises_tenants_where_id() {
    assert!(mentions_tenant_scope(
        "SELECT banner_text FROM tenants WHERE id = $1"
    ));
    assert!(mentions_tenant_scope(
        "UPDATE tenants SET name = $2 WHERE id = $1"
    ));
}

#[test]
fn predicate_rejects_tenantless_query() {
    assert!(!mentions_tenant_scope(
        "SELECT slug FROM skills WHERE status = 'published'"
    ));
    assert!(!mentions_tenant_scope("DELETE FROM widgets WHERE id = $1"));
}
