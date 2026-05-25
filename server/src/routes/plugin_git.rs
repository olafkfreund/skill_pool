//! Per-plugin dumb-HTTP **smart** git endpoint (#31).
//!
//! Two routes Claude Code's `/plugin install` calls during a clone:
//!
//! - `GET  /git/plugins/<slug>.git/info/refs?service=git-upload-pack`
//!   → smart-protocol ref advertisement (pkt-lines).
//! - `POST /git/plugins/<slug>.git/git-upload-pack`
//!   → upload-pack request: client sends `want`/`have` pkt-lines, server
//!   streams back a packfile sideband-multiplexed inside pkt-lines.
//!
//! ## Why hand-rolled
//!
//! libgit2 does not ship a complete HTTP-transport server. It does ship
//! everything we need for the object store (`PackBuilder`, `Revwalk`,
//! `Repository`); the protocol framing is small enough — and important
//! enough to test — that wrapping it ourselves is clearer than wiring
//! through a half-supported transport API. The pkt-line format is well
//! documented (Git's [Documentation/technical/pack-protocol.txt][1]).
//!
//! ## Scope
//!
//! - Read-only. We don't expose `git-receive-pack`; pushes through this
//!   endpoint would bypass `/v1/plugins`'s validation.
//! - Clone-optimised: when no `have`s are sent we ship the full reachable
//!   set in one pack. With `have`s we walk a hide-revwalk so common
//!   ancestors are excluded — same semantics as a real git server but
//!   without `git-pack-objects`' clever cutoff heuristics.
//!
//! ## Public, rate-limited
//!
//! No `AuthedCaller` — `git clone` is unauthenticated by design. The
//! per-tenant rate limiter still applies (not in `SKIP_PATHS`).
//!
//! [1]: https://git-scm.com/docs/protocol-v2

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Deserialize)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

/// `GET /git/plugins/{slug}.git/info/refs?service=git-upload-pack`
pub async fn info_refs(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug_git): Path<String>,
    Query(q): Query<InfoRefsQuery>,
) -> AppResult<Response> {
    let service = q.service.as_deref().unwrap_or("");
    if service != "git-upload-pack" {
        // We only serve clone/fetch — receive-pack would let clients push
        // commits past `/v1/plugins`' validation.
        return Err(AppError::BadRequest(format!(
            "unsupported service `{service}` — only git-upload-pack is served"
        )));
    }

    let slug = strip_git_suffix(&slug_git)?;
    let repo_path = resolve_repo_path(&state, &tenant, slug).await?;
    let body = tokio::task::spawn_blocking(move || advertise_refs(&repo_path))
        .await
        .map_err(|e| AppError::BadRequest(format!("info/refs join: {e}")))??;

    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    let _ = h.insert(
        axum::http::header::CONTENT_TYPE,
        "application/x-git-upload-pack-advertisement"
            .parse()
            .unwrap(),
    );
    let _ = h.insert(
        axum::http::header::CACHE_CONTROL,
        "no-cache".parse().unwrap(),
    );
    Ok(resp)
}

/// `POST /git/plugins/{slug}.git/git-upload-pack`
pub async fn upload_pack(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug_git): Path<String>,
    _headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let slug = strip_git_suffix(&slug_git)?;
    let repo_path = resolve_repo_path(&state, &tenant, slug).await?;
    let body_vec = body.to_vec();
    let response_body = tokio::task::spawn_blocking(move || run_upload_pack(&repo_path, &body_vec))
        .await
        .map_err(|e| AppError::BadRequest(format!("upload-pack join: {e}")))??;

    let mut resp = (StatusCode::OK, response_body).into_response();
    let h = resp.headers_mut();
    let _ = h.insert(
        axum::http::header::CONTENT_TYPE,
        "application/x-git-upload-pack-result".parse().unwrap(),
    );
    let _ = h.insert(
        axum::http::header::CACHE_CONTROL,
        "no-cache".parse().unwrap(),
    );
    Ok(resp)
}

/// `<slug>.git` → `<slug>`. Anything else (missing suffix, empty slug) is
/// a 400. Same shape as a real git host so a misspelled clone URL gets
/// a sensible error rather than 404.
fn strip_git_suffix(raw: &str) -> AppResult<&str> {
    let slug = raw
        .strip_suffix(".git")
        .ok_or_else(|| AppError::BadRequest(format!("path segment must end with .git: {raw}")))?;
    if slug.is_empty() {
        return Err(AppError::BadRequest("empty plugin slug in URL".into()));
    }
    Ok(slug)
}

async fn resolve_repo_path(
    state: &AppState,
    tenant: &TenantCtx,
    slug: &str,
) -> AppResult<PathBuf> {
    // Verify the plugin exists for this tenant + is internal/mirror (external
    // plugins live elsewhere; we don't proxy them). 404 otherwise.
    let row = sqlx::query!(
        "SELECT sourcing_mode \
         FROM plugins \
         WHERE tenant_id = $1 AND slug = $2 \
         ORDER BY created_at DESC LIMIT 1",
        tenant.tenant_id,
        &slug,
    )
    .fetch_optional(state.db_read())
    .await?
    .ok_or(AppError::NotFound)?;
    if row.sourcing_mode == "external" {
        return Err(AppError::NotFound);
    }

    let storage = state
        .storage_for(tenant)
        .await
        .map_err(AppError::Anyhow)?;
    let path = storage
        .plugin_git_path(tenant.tenant_id, slug)
        .map_err(AppError::Anyhow)?;
    if !path.exists() {
        // Plugin row exists but its bare repo isn't on disk yet — happens
        // when materialisation failed (logged at publish time). Surface
        // 404 rather than 500; admin can republish to recover.
        return Err(AppError::NotFound);
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Smart-HTTP framing
// ---------------------------------------------------------------------------

/// Write a single pkt-line (4-hex length prefix + payload). Length includes
/// the prefix itself.
fn pkt_line(out: &mut Vec<u8>, payload: &[u8]) {
    let len = payload.len() + 4;
    let header = format!("{:04x}", len);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(payload);
}

/// Flush pkt — terminates a section.
fn pkt_flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

/// Build the `info/refs` advertisement body. Lists each ref as a pkt-line;
/// the first ref carries the server capability set (NUL-separated).
fn advertise_refs(repo_path: &std::path::Path) -> AppResult<Vec<u8>> {
    let repo = git2::Repository::open_bare(repo_path)
        .map_err(|e| AppError::BadRequest(format!("open repo: {e}")))?;

    let mut out = Vec::new();
    // Smart-HTTP prelude — service banner pkt-line + flush.
    pkt_line(&mut out, b"# service=git-upload-pack\n");
    pkt_flush(&mut out);

    let refs = collect_refs(&repo)?;
    if refs.is_empty() {
        // Empty repo — protocol still requires a capabilities pkt-line.
        // Send a single "capabilities^{}" sentinel per the spec.
        let line = format!(
            "{} capabilities^{{}}\0{}\n",
            "0".repeat(40),
            server_capabilities()
        );
        pkt_line(&mut out, line.as_bytes());
        pkt_flush(&mut out);
        return Ok(out);
    }

    for (i, (oid, name)) in refs.iter().enumerate() {
        let line = if i == 0 {
            format!("{oid} {name}\0{}\n", server_capabilities())
        } else {
            format!("{oid} {name}\n")
        };
        pkt_line(&mut out, line.as_bytes());
    }
    pkt_flush(&mut out);
    Ok(out)
}

/// Capability string advertised on the first ref. Kept narrow:
///   * `multi_ack_detailed` + `no-done` — modern negotiation
///   * `side-band-64k` — pack data multiplexed inside pkt-lines
///   * `agent=` — best-practice identification.
fn server_capabilities() -> &'static str {
    "multi_ack_detailed no-done side-band-64k thin-pack ofs-delta shallow agent=skill-pool/0.1"
}

/// Refs in advertisement order: HEAD first, then refs alphabetically. The
/// SHAs are 40-hex lowercase per spec.
fn collect_refs(repo: &git2::Repository) -> AppResult<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();

    // HEAD comes first when it resolves.
    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target() {
            out.push((oid.to_string(), "HEAD".to_string()));
        }
    }

    let mut others: Vec<(String, String)> = Vec::new();
    let iter = repo
        .references()
        .map_err(|e| AppError::BadRequest(format!("iterate refs: {e}")))?;
    for r in iter {
        let r = r.map_err(|e| AppError::BadRequest(format!("read ref: {e}")))?;
        let Some(name) = r.name() else { continue };
        // Skip symbolic / non-direct refs without targets.
        let Some(target) = r.target() else { continue };
        // Don't double-emit HEAD if it appeared above.
        if name == "HEAD" {
            continue;
        }
        others.push((target.to_string(), name.to_string()));
    }
    others.sort_by(|a, b| a.1.cmp(&b.1));
    out.extend(others);
    Ok(out)
}

// ---------------------------------------------------------------------------
// upload-pack request parsing + pack generation
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct UploadRequest {
    wants: Vec<git2::Oid>,
    haves: Vec<git2::Oid>,
    /// Commits the client already has as shallow boundaries. Currently
    /// recorded for diagnostic completeness; depth-bounded packs don't
    /// need them because we always rebuild from scratch on each clone.
    #[allow(dead_code)]
    shallows: Vec<git2::Oid>,
    /// Requested depth from `deepen <n>`. None = full clone.
    deepen: Option<u32>,
    done: bool,
}

/// Parse a client `git-upload-pack` request body into wants/haves.
///
/// Body grammar (simplified):
///   want SP <40-hex> [SP <cap-list>] LF
///   want SP <40-hex> LF
///   ...
///   flush
///   have SP <40-hex> LF ...
///   ...
///   done LF
fn parse_upload_request(body: &[u8]) -> AppResult<UploadRequest> {
    let mut req = UploadRequest::default();
    let mut cursor = 0;
    while cursor < body.len() {
        if cursor + 4 > body.len() {
            return Err(AppError::BadRequest(
                "upload-pack request truncated in pkt-line header".into(),
            ));
        }
        let len_hex = std::str::from_utf8(&body[cursor..cursor + 4])
            .map_err(|_| AppError::BadRequest("non-utf8 pkt-line length".into()))?;
        let len = u32::from_str_radix(len_hex, 16)
            .map_err(|_| AppError::BadRequest(format!("invalid pkt-line length `{len_hex}`")))?;
        cursor += 4;
        if len == 0 {
            // Flush pkt — section separator.
            continue;
        }
        if len < 4 {
            return Err(AppError::BadRequest(format!(
                "pkt-line length {len} < 4"
            )));
        }
        let payload_len = (len - 4) as usize;
        if cursor + payload_len > body.len() {
            return Err(AppError::BadRequest(
                "upload-pack request truncated in pkt-line body".into(),
            ));
        }
        let payload = &body[cursor..cursor + payload_len];
        cursor += payload_len;
        let line = std::str::from_utf8(payload)
            .map_err(|_| AppError::BadRequest("non-utf8 pkt-line body".into()))?
            .trim_end_matches('\n');

        if let Some(rest) = line.strip_prefix("want ") {
            // `want <40-hex>` optionally followed by space-separated caps
            // on the first want line.
            let sha = rest.split_whitespace().next().unwrap_or("");
            let oid = git2::Oid::from_str(sha).map_err(|_| {
                AppError::BadRequest(format!("invalid want SHA `{sha}`"))
            })?;
            req.wants.push(oid);
        } else if let Some(sha) = line.strip_prefix("have ") {
            let oid = git2::Oid::from_str(sha).map_err(|_| {
                AppError::BadRequest(format!("invalid have SHA `{sha}`"))
            })?;
            req.haves.push(oid);
        } else if let Some(sha) = line.strip_prefix("shallow ") {
            let oid = git2::Oid::from_str(sha).map_err(|_| {
                AppError::BadRequest(format!("invalid shallow SHA `{sha}`"))
            })?;
            req.shallows.push(oid);
        } else if let Some(n) = line.strip_prefix("deepen ") {
            let depth: u32 = n.trim().parse().map_err(|_| {
                AppError::BadRequest(format!("invalid deepen value `{n}`"))
            })?;
            // Reject 0 — git's protocol treats `deepen 0` as "no depth",
            // which is ambiguous. Clients send `deepen 1` for `--depth=1`.
            if depth == 0 {
                return Err(AppError::BadRequest("deepen 0 not supported".into()));
            }
            req.deepen = Some(depth);
        } else if line == "done" {
            req.done = true;
        }
        // Other lines (deepen-since / deepen-not / no-progress / etc) silently
        // ignored — narrow capability set keeps the protocol surface small.
    }
    Ok(req)
}

/// Drive a full upload-pack exchange and return the bytes to write back.
fn run_upload_pack(repo_path: &std::path::Path, body: &[u8]) -> AppResult<Vec<u8>> {
    let repo = git2::Repository::open_bare(repo_path)
        .map_err(|e| AppError::BadRequest(format!("open repo: {e}")))?;
    let req = parse_upload_request(body)?;

    if req.wants.is_empty() {
        return Err(AppError::BadRequest(
            "upload-pack request has no `want` lines".into(),
        ));
    }

    let mut out = Vec::new();

    // For shallow requests, compute the new boundaries and emit
    // `shallow <sha>` ACK lines + flush BEFORE the NAK. This is the
    // section the client parses to update its `.git/shallow` file.
    let boundaries = if let Some(depth) = req.deepen {
        let bounds = compute_shallow_boundaries(&repo, &req.wants, depth)?;
        for oid in &bounds {
            pkt_line(&mut out, format!("shallow {oid}\n").as_bytes());
        }
        pkt_flush(&mut out);
        bounds
    } else {
        Vec::new()
    };

    // Acknowledgement / NAK section.
    //
    // With `multi_ack_detailed`, real servers would emit `ACK <sha> common`
    // and `ACK <sha> ready` lines tracking the negotiation. For a
    // clone-only endpoint where the client sends no `have`s and we always
    // ship the full reachable set (or depth-bounded set), the
    // protocol-correct minimal response is a single `NAK` pkt-line — both
    // `git` and libgit2 clients accept this and proceed to read the
    // packfile.
    pkt_line(&mut out, b"NAK\n");

    // Build the packfile via libgit2.
    let mut pb = repo
        .packbuilder()
        .map_err(|e| AppError::BadRequest(format!("create packbuilder: {e}")))?;
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| AppError::BadRequest(format!("create revwalk: {e}")))?;
    for want in &req.wants {
        revwalk
            .push(*want)
            .map_err(|e| AppError::BadRequest(format!("revwalk push: {e}")))?;
    }
    for have in &req.haves {
        // Hide ancestors of haves so we don't re-send objects the client
        // already has. Missing objects (client claimed something we don't
        // have) are silently ignored — same behaviour as a real git server.
        let _ = revwalk.hide(*have);
    }
    // For shallow clones, hide PARENTS of each boundary commit. This
    // keeps the boundary commits themselves in the pack but stops the
    // walk from following them further back. libgit2's revwalk.hide()
    // hides the commit AND all its ancestors, so hiding parents-of-
    // boundaries is exactly what we want.
    for boundary in &boundaries {
        if let Ok(commit) = repo.find_commit(*boundary) {
            for parent in commit.parents() {
                let _ = revwalk.hide(parent.id());
            }
        }
    }
    // Mark every reachable commit, and let libgit2 follow each commit's
    // tree + blobs into the pack.
    pb.insert_walk(&mut revwalk)
        .map_err(|e| AppError::BadRequest(format!("packbuilder insert walk: {e}")))?;

    // Generate the pack to a buffer.
    let mut pack_bytes: Vec<u8> = Vec::new();
    pb.foreach(|chunk| {
        pack_bytes.extend_from_slice(chunk);
        true
    })
    .map_err(|e| AppError::BadRequest(format!("packbuilder foreach: {e}")))?;

    // Sideband-64k frame the packfile inside pkt-lines: each chunk is a
    // pkt-line starting with band 0x01 (pack data). Max payload 65515 to
    // stay under the 65520-byte pkt-line cap.
    const MAX_CHUNK: usize = 65515;
    for chunk in pack_bytes.chunks(MAX_CHUNK) {
        let mut payload = Vec::with_capacity(chunk.len() + 1);
        payload.push(0x01); // band 1 = pack data
        payload.extend_from_slice(chunk);
        pkt_line(&mut out, &payload);
    }
    pkt_flush(&mut out);

    Ok(out)
}

/// BFS from each `want` up to `depth` commits deep. Commits at exactly
/// level `depth` whose chain doesn't terminate naturally are the new
/// shallow boundaries — the client will record them in `.git/shallow`
/// and refuse to walk past them on subsequent ops.
///
/// `depth` semantics match `git clone --depth=N`:
///   * `depth=1`: include the wanted commit only. Wanted commits are
///     boundaries (if they have parents).
///   * `depth=N`: include N commits per chain. The Nth commit is the
///     boundary.
fn compute_shallow_boundaries(
    repo: &git2::Repository,
    wants: &[git2::Oid],
    depth: u32,
) -> AppResult<Vec<git2::Oid>> {
    use std::collections::{HashSet, VecDeque};
    let mut seen: HashSet<git2::Oid> = HashSet::new();
    let mut boundaries: Vec<git2::Oid> = Vec::new();
    let mut queue: VecDeque<(git2::Oid, u32)> = VecDeque::new();
    for w in wants {
        if seen.insert(*w) {
            queue.push_back((*w, 1));
        }
    }
    while let Some((oid, d)) = queue.pop_front() {
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue, // missing in repo — silently skip
        };
        let parent_ids: Vec<git2::Oid> = commit.parents().map(|p| p.id()).collect();
        if d == depth {
            // Boundary candidate — only emit a shallow line if the chain
            // continues past this commit. Root commits don't need one.
            if !parent_ids.is_empty() {
                boundaries.push(oid);
            }
        } else {
            // d < depth: enqueue parents one level deeper.
            for p in parent_ids {
                if seen.insert(p) {
                    queue.push_back((p, d + 1));
                }
            }
        }
    }
    Ok(boundaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkt_line_prefixes_hex_length() {
        let mut buf = Vec::new();
        pkt_line(&mut buf, b"hi\n");
        // length 4 (header) + 3 (body) = 7 → "0007"
        assert_eq!(&buf, b"0007hi\n");
    }

    #[test]
    fn pkt_flush_writes_four_zeros() {
        let mut buf = Vec::new();
        pkt_flush(&mut buf);
        assert_eq!(&buf, b"0000");
    }

    #[test]
    fn parse_upload_request_extracts_wants_and_haves() {
        // Build a tiny request: want X / flush / have Y / done
        let mut body = Vec::new();
        let want_line = format!(
            "want {} side-band-64k\n",
            "0123456789abcdef0123456789abcdef01234567"
        );
        pkt_line(&mut body, want_line.as_bytes());
        pkt_flush(&mut body);
        let have_line = format!(
            "have {}\n",
            "fedcba9876543210fedcba9876543210fedcba98"
        );
        pkt_line(&mut body, have_line.as_bytes());
        pkt_line(&mut body, b"done\n");

        let req = parse_upload_request(&body).unwrap();
        assert_eq!(req.wants.len(), 1);
        assert_eq!(req.haves.len(), 1);
        assert!(req.done);
    }

    #[test]
    fn parse_upload_request_rejects_bad_sha() {
        let mut body = Vec::new();
        pkt_line(&mut body, b"want notahex\n");
        assert!(parse_upload_request(&body).is_err());
    }

    // ---- #58: shallow / deepen protocol coverage -------------------------

    #[test]
    fn server_capabilities_advertises_shallow() {
        // Without this advertisement, `git clone --depth=N` aborts before
        // negotiation completes ("Server does not support shallow clients").
        assert!(
            server_capabilities().contains(" shallow "),
            "capability string missing `shallow`: {}",
            server_capabilities()
        );
    }

    #[test]
    fn parse_upload_request_captures_deepen() {
        let mut body = Vec::new();
        let want_line = format!(
            "want {} side-band-64k\n",
            "0123456789abcdef0123456789abcdef01234567"
        );
        pkt_line(&mut body, want_line.as_bytes());
        pkt_line(&mut body, b"deepen 3\n");
        pkt_flush(&mut body);
        pkt_line(&mut body, b"done\n");

        let req = parse_upload_request(&body).unwrap();
        assert_eq!(req.deepen, Some(3));
        assert_eq!(req.wants.len(), 1);
        assert!(req.done);
    }

    #[test]
    fn parse_upload_request_captures_shallow() {
        let mut body = Vec::new();
        let want_line = format!(
            "want {} side-band-64k\n",
            "0123456789abcdef0123456789abcdef01234567"
        );
        pkt_line(&mut body, want_line.as_bytes());
        let shallow_line = format!(
            "shallow {}\n",
            "fedcba9876543210fedcba9876543210fedcba98"
        );
        pkt_line(&mut body, shallow_line.as_bytes());
        pkt_flush(&mut body);

        let req = parse_upload_request(&body).unwrap();
        assert_eq!(req.shallows.len(), 1);
    }

    #[test]
    fn parse_upload_request_rejects_deepen_zero() {
        let mut body = Vec::new();
        let want_line = format!(
            "want {} side-band-64k\n",
            "0123456789abcdef0123456789abcdef01234567"
        );
        pkt_line(&mut body, want_line.as_bytes());
        pkt_line(&mut body, b"deepen 0\n");
        assert!(parse_upload_request(&body).is_err());
    }

    /// Build a tiny bare repo with N linear commits and return the tip Oid.
    fn make_linear_repo(dir: &std::path::Path, n: usize) -> (git2::Repository, git2::Oid) {
        let repo = git2::Repository::init_bare(dir).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let mut tip: Option<git2::Oid> = None;
        for i in 0..n {
            let mut idx = repo.index().unwrap();
            // Empty tree is fine for the structural test.
            let tree_id = idx.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let parents: Vec<git2::Commit> = tip
                .iter()
                .map(|p| repo.find_commit(*p).unwrap())
                .collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            let oid = repo
                .commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &format!("commit {i}"),
                    &tree,
                    &parent_refs,
                )
                .unwrap();
            tip = Some(oid);
        }
        (repo, tip.unwrap())
    }

    #[test]
    fn boundaries_depth_one_is_the_tip_when_history_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let bounds = compute_shallow_boundaries(&repo, &[tip], 1).unwrap();
        // depth=1 means "include just the tip". Tip has a parent → it's a
        // shallow boundary.
        assert_eq!(bounds, vec![tip]);
    }

    #[test]
    fn boundaries_depth_two_is_first_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let parent = repo.find_commit(tip).unwrap().parent(0).unwrap().id();
        let bounds = compute_shallow_boundaries(&repo, &[tip], 2).unwrap();
        // depth=2 means "include tip + tip.parent". The parent is the
        // boundary (it still has its own parent).
        assert_eq!(bounds, vec![parent]);
    }

    #[test]
    fn boundaries_skip_root_commits() {
        let tmp = tempfile::tempdir().unwrap();
        // 3-commit linear history; depth=10 walks past the root.
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let bounds = compute_shallow_boundaries(&repo, &[tip], 10).unwrap();
        // The root commit has no parents → no `shallow` line needed.
        assert!(
            bounds.is_empty(),
            "expected no boundaries past the root, got {:?}",
            bounds
        );
    }
}
