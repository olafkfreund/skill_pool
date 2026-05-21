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
    "multi_ack_detailed no-done side-band-64k thin-pack ofs-delta agent=skill-pool/0.1"
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
        } else if line == "done" {
            req.done = true;
        }
        // Other lines (shallow / deepen / etc) silently ignored — v1
        // scope is clone-and-shallow-fetch; full shallow support is a
        // followup if a tenant ever surfaces a complaint.
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

    // Acknowledgement / NAK section.
    //
    // With `multi_ack_detailed`, real servers would emit `ACK <sha> common`
    // and `ACK <sha> ready` lines tracking the negotiation. For a
    // clone-only endpoint where the client sends no `have`s and we always
    // ship the full reachable set, the protocol-correct minimal response
    // is a single `NAK` pkt-line — both `git` and libgit2 clients accept
    // this and proceed to read the packfile.
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
}
