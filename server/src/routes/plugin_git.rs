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
    // `shallow <sha>` ACK lines + flush. This is the section the
    // client parses to update its `.git/shallow` file.
    //
    // Smart-HTTP stateless-RPC two-phase deepening: git's client sends
    // a FIRST POST with `want + deepen + flush + flush` (no `done`)
    // expecting ONLY the shallow section, then a SECOND POST with
    // `want + deepen + flush + done + flush` expecting shallow + NAK
    // + pack. If we ship the full response on the first POST, the
    // client reads shallow+flush and disconnects, the unread bytes
    // (NAK + pack) sit in the kernel's send buffer / TCP RST, and
    // the second POST gets confused with "expected shallow list".
    //
    // Detection: deepen is set AND done is unset → first POST,
    // return only the shallow section.
    if let Some(depth) = req.deepen {
        let bounds = compute_shallow_boundaries(&repo, &req.wants, depth)?;
        for oid in &bounds {
            pkt_line(&mut out, format!("shallow {oid}\n").as_bytes());
        }
        pkt_flush(&mut out);
    }

    // First-phase deepening: client hasn't sent `done` yet — return
    // only the shallow section. The client will follow up with a
    // second POST that includes `done`, which we'll handle below.
    if req.deepen.is_some() && !req.done {
        return Ok(out);
    }

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

    if req.deepen.is_some() {
        // Shallow path: build the explicit commit set up to `depth`, then
        // insert each commit individually via `pb.insert_commit`. libgit2
        // auto-follows the commit's tree + blobs.
        //
        // We do NOT use `revwalk.hide(parent)` for boundary trimming —
        // hide transitively hides the parent's blobs, which the tip's
        // tree may reference (unchanged files). Result was "bad tree
        // object: remote did not send all necessary objects" on
        // single-commit-or-shallow-depth packs.
        let included = collect_commits_up_to_depth(&repo, &req.wants, req.deepen.unwrap())?;
        for oid in &included {
            pb.insert_commit(*oid)
                .map_err(|e| AppError::BadRequest(format!("packbuilder insert commit {oid}: {e}")))?;
        }
    } else {
        // Full-clone path: revwalk + insert_walk is fine because no
        // shallow boundary means no parent-hiding artefacts.
        let mut revwalk = repo
            .revwalk()
            .map_err(|e| AppError::BadRequest(format!("create revwalk: {e}")))?;
        for want in &req.wants {
            revwalk
                .push(*want)
                .map_err(|e| AppError::BadRequest(format!("revwalk push: {e}")))?;
        }
        for have in &req.haves {
            // Hide ancestors of haves so we don't re-send objects the
            // client already has. Missing objects (client claimed
            // something we don't have) are silently ignored — same
            // behaviour as a real git server.
            let _ = revwalk.hide(*have);
        }
        pb.insert_walk(&mut revwalk)
            .map_err(|e| AppError::BadRequest(format!("packbuilder insert walk: {e}")))?;
    }

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

/// BFS from each `want` up to `depth` commits deep. Returns every commit
/// visited (those within the depth window). The packfile must contain
/// every one of these commits plus their trees and blobs.
///
/// Used in place of `revwalk + insert_walk` for shallow packs — see
/// `run_upload_pack` for why we can't just hide parents.
fn collect_commits_up_to_depth(
    repo: &git2::Repository,
    wants: &[git2::Oid],
    depth: u32,
) -> AppResult<Vec<git2::Oid>> {
    use std::collections::{HashSet, VecDeque};
    let mut seen: HashSet<git2::Oid> = HashSet::new();
    let mut included: Vec<git2::Oid> = Vec::new();
    let mut queue: VecDeque<(git2::Oid, u32)> = VecDeque::new();
    for w in wants {
        if seen.insert(*w) {
            queue.push_back((*w, 1));
        }
    }
    while let Some((oid, d)) = queue.pop_front() {
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        included.push(oid);
        if d < depth {
            for p in commit.parents() {
                let p_oid = p.id();
                if seen.insert(p_oid) {
                    queue.push_back((p_oid, d + 1));
                }
            }
        }
    }
    Ok(included)
}

/// BFS from each `want` up to `depth` commits deep. Commits at exactly
/// level `depth` are the new shallow boundaries — the client will record
/// them in `.git/shallow` and refuse to walk past them on subsequent ops.
///
/// `depth` semantics match `git clone --depth=N`:
///   * `depth=1`: include the wanted commit only. Wanted commits are
///     boundaries.
///   * `depth=N`: include N commits per chain. The Nth commit is the
///     boundary.
///
/// Root commits at the boundary still get reported. The git protocol
/// requires the shallow list to contain every commit at the boundary
/// depth regardless of whether parents exist; clients send
/// "fatal: git fetch-pack: expected shallow list" if the list is empty
/// when `deepen` was requested but at least one want exists.
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
            // Boundary — emit unconditionally. Root commits at the
            // boundary are valid shallow markers; the client treats
            // `shallow <root>` as "stop walking past this commit",
            // which is a no-op for a root, but the protocol requires
            // the line.
            boundaries.push(oid);
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
    fn boundaries_depth_one_is_the_tip() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let bounds = compute_shallow_boundaries(&repo, &[tip], 1).unwrap();
        // depth=1 means "include just the tip". Tip is the boundary
        // regardless of whether it has parents — the client needs the
        // line either way.
        assert_eq!(bounds, vec![tip]);
    }

    #[test]
    fn boundaries_depth_two_is_first_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let parent = repo.find_commit(tip).unwrap().parent(0).unwrap().id();
        let bounds = compute_shallow_boundaries(&repo, &[tip], 2).unwrap();
        // depth=2 means "include tip + tip.parent". The parent is the
        // boundary.
        assert_eq!(bounds, vec![parent]);
    }

    #[test]
    fn boundaries_depth_one_on_single_commit_repo_is_the_root() {
        // Regression for the post-#58 bug: a 1-commit repo cloned with
        // --depth=1 must still emit `shallow <tip>` — otherwise the
        // client errors with "fatal: git fetch-pack: expected shallow
        // list". The tip happens to be the root, and that's fine.
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = make_linear_repo(tmp.path(), 1);
        let bounds = compute_shallow_boundaries(&repo, &[tip], 1).unwrap();
        assert_eq!(bounds, vec![tip]);
    }

    /// Build a minimal upload-pack request body with the given options.
    fn make_request(tip: git2::Oid, deepen: Option<u32>, send_done: bool) -> Vec<u8> {
        let mut body = Vec::new();
        let first_want = format!(
            "want {} multi_ack_detailed no-done side-band-64k thin-pack ofs-delta agent=test/1.0\n",
            tip
        );
        pkt_line(&mut body, first_want.as_bytes());
        if let Some(d) = deepen {
            pkt_line(&mut body, format!("deepen {d}\n").as_bytes());
        }
        pkt_flush(&mut body);
        if send_done {
            pkt_line(&mut body, b"done\n");
        }
        pkt_flush(&mut body);
        body
    }

    #[test]
    fn first_phase_deepening_returns_only_shallow_section() {
        // Regression for the post-#65 bug: git's smart-HTTP stateless-RPC
        // makes two POSTs for `--depth=N`. The first POST has deepen but
        // no `done` and expects ONLY the shallow section + flush. If we
        // ship the pack on this round, the client's second POST corrupts
        // with "fatal: git fetch-pack: expected shallow list".
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, tip) = make_linear_repo(tmp.path(), 1);
        let body = make_request(tip, Some(1), /* send_done= */ false);
        let resp = run_upload_pack(tmp.path(), &body).unwrap();

        // Expected: pkt_line("shallow <tip>\n") + pkt_flush. NOTHING else.
        let mut expected = Vec::new();
        pkt_line(&mut expected, format!("shallow {tip}\n").as_bytes());
        pkt_flush(&mut expected);
        assert_eq!(
            resp, expected,
            "first-phase deepening must return only the shallow section"
        );
    }

    /// Build a 2-commit bare repo where the tip's tree SHARES a blob
    /// with its parent (an unchanged file). Regression material for the
    /// "bad tree object" bug — if we hide parent via revwalk, the
    /// shared blob gets hidden too and the pack is incomplete.
    fn make_shared_blob_repo(dir: &std::path::Path) -> (git2::Repository, git2::Oid) {
        let repo = git2::Repository::init_bare(dir).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let stable_blob = repo.blob(b"stable\n").unwrap();
        let new_blob = repo.blob(b"added\n").unwrap();

        // Commit 1: tree contains keep.txt = "stable"
        let t1 = {
            let mut b = repo.treebuilder(None).unwrap();
            b.insert("keep.txt", stable_blob, 0o100644).unwrap();
            b.write().unwrap()
        };
        let c1 = {
            let tree = repo.find_tree(t1).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "c1", &tree, &[])
                .unwrap()
        };

        // Commit 2: tree contains keep.txt (same blob) + new.txt (new blob)
        let t2 = {
            let mut b = repo.treebuilder(None).unwrap();
            b.insert("keep.txt", stable_blob, 0o100644).unwrap();
            b.insert("new.txt", new_blob, 0o100644).unwrap();
            b.write().unwrap()
        };
        let c2 = {
            let tree = repo.find_tree(t2).unwrap();
            let parent = repo.find_commit(c1).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "c2", &tree, &[&parent])
                .unwrap()
        };
        (repo, c2)
    }

    /// Extract the raw packfile bytes from a side-band-64k framed response,
    /// stripping the shallow section + NAK pkt-line first.
    fn extract_pack(resp: &[u8]) -> Vec<u8> {
        // Skip shallow section: read pkt-lines until we hit a flush.
        let mut cursor = 0;
        loop {
            let len_hex = std::str::from_utf8(&resp[cursor..cursor + 4]).unwrap();
            let len = u32::from_str_radix(len_hex, 16).unwrap();
            cursor += 4;
            if len == 0 {
                break; // flush pkt
            }
            cursor += (len - 4) as usize;
        }
        // Read the NAK pkt-line.
        let len_hex = std::str::from_utf8(&resp[cursor..cursor + 4]).unwrap();
        let len = u32::from_str_radix(len_hex, 16).unwrap();
        cursor += 4 + (len - 4) as usize; // skip "NAK\n"

        // Concatenate sideband-1 frames into raw pack bytes.
        let mut pack = Vec::new();
        while cursor < resp.len() {
            let len_hex = std::str::from_utf8(&resp[cursor..cursor + 4]).unwrap();
            let len = u32::from_str_radix(len_hex, 16).unwrap();
            cursor += 4;
            if len == 0 {
                break; // final flush
            }
            let payload = &resp[cursor..cursor + (len - 4) as usize];
            cursor += (len - 4) as usize;
            if !payload.is_empty() && payload[0] == 0x01 {
                pack.extend_from_slice(&payload[1..]);
            }
        }
        pack
    }

    #[test]
    fn shallow_pack_includes_blobs_shared_with_hidden_parent() {
        // Regression for the "bad tree object" bug: with a 2-commit
        // history where the tip's tree references blobs that ALSO exist
        // in the parent's tree (unchanged files), the pack must include
        // every blob the tip's tree points at. The previous
        // `revwalk.hide(parent)` strategy hid the shared blob and the
        // client errored with "bad tree object: remote did not send all
        // necessary objects".
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, tip) = make_shared_blob_repo(tmp.path());

        let body = make_request(tip, Some(1), /* send_done= */ true);
        let resp = run_upload_pack(tmp.path(), &body).unwrap();
        let pack = extract_pack(&resp);

        // Index the pack against a fresh repo and verify the tip's tree
        // resolves end-to-end (commit -> tree -> "keep.txt" blob).
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = git2::Repository::init_bare(dst_dir.path()).unwrap();
        let odb = dst.odb().unwrap();
        let mut writer = odb.packwriter().unwrap();
        std::io::Write::write_all(&mut writer, &pack).unwrap();
        writer.commit().unwrap();

        let commit = dst.find_commit(tip).expect("tip commit present in pack");
        let tree = commit.tree().expect("tip tree present in pack");
        let entry = tree
            .get_name("keep.txt")
            .expect("keep.txt entry in tree");
        let blob = dst
            .find_blob(entry.id())
            .expect("keep.txt blob present in pack (shared with parent)");
        assert_eq!(blob.content(), b"stable\n");
    }

    #[test]
    fn second_phase_deepening_returns_shallow_nak_pack() {
        // Second POST: deepen + done + flush → must include shallow + flush
        // + NAK + pack. We don't assert pack contents here (covered by
        // plugin_git_clone.rs integration test); just that the response is
        // strictly longer than the shallow-only first-phase response.
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, tip) = make_linear_repo(tmp.path(), 1);
        let body = make_request(tip, Some(1), /* send_done= */ true);
        let resp = run_upload_pack(tmp.path(), &body).unwrap();

        // Has the shallow header...
        let shallow_prefix = format!("0035shallow {tip}\n0000", tip = tip);
        assert!(
            resp.starts_with(shallow_prefix.as_bytes()),
            "expected shallow + flush prefix, got first 60 bytes: {:?}",
            String::from_utf8_lossy(&resp[..resp.len().min(60)])
        );
        // ...then NAK pkt-line at expected offset (53 bytes after shallow + flush).
        let nak_offset = shallow_prefix.len();
        assert_eq!(
            &resp[nak_offset..nak_offset + 8],
            b"0008NAK\n",
            "expected NAK pkt-line after shallow section"
        );
        // ...then sideband-framed pack data (band 0x01).
        assert!(
            resp.len() > nak_offset + 8 + 4,
            "expected pack frames after NAK, response only {} bytes",
            resp.len()
        );
    }

    #[test]
    fn boundaries_walking_past_root_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // 3-commit linear history; depth=10 walks past the root — there
        // is no commit at depth=10, so no boundary needed (and no error).
        let (repo, tip) = make_linear_repo(tmp.path(), 3);
        let bounds = compute_shallow_boundaries(&repo, &[tip], 10).unwrap();
        assert!(
            bounds.is_empty(),
            "no commit exists at depth=10, expected empty boundary list, got {:?}",
            bounds
        );
    }
}
