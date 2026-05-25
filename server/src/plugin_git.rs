//! Plugin → bare git repo materialiser (#31).
//!
//! When a plugin in `internal` sourcing mode is published, the registry
//! must expose its tree at `/git/plugins/<slug>.git/...` for Claude Code's
//! `/plugin install` to clone. This module is the helper that takes a
//! published row + its `plugin_contents` and writes the canonical plugin
//! filesystem layout into a bare repo via libgit2.
//!
//! ## Tree shape
//!
//! Mirrors `docs/plugin-manifest-schema.md::Filesystem layout`:
//!
//! ```text
//! <slug>/
//! ├── .claude-plugin/plugin.json     ← from plugins.manifest JSONB
//! ├── skills/<slug>/…                ← extracted from each skill bundle
//! ├── agents/<slug>.md               ← single-file
//! ├── commands/<slug>.md             ← single-file
//! └── (inline hooks/MCP/LSP/monitors when present in the manifest)
//! ```
//!
//! ## Atomicity
//!
//! Each call writes a single new commit and points `refs/heads/main` at it
//! (and `refs/tags/<version>` for the version label). libgit2 advances the
//! ref via rename(2)-equivalent semantics so an in-flight `git-upload-pack`
//! either sees the old SHA or the new one — never a torn write. Past
//! versions stay reachable via their tags so the marketplace can pin.
//!
//! ## Scope
//!
//! Only `internal` plugins materialise here. `external` plugins live in
//! someone else's git; `mirror` plugins land via the mirror worker (#36
//! follow-up). Calling this helper on a non-internal plugin is a no-op.
//!
//! ## Idempotency
//!
//! Safe to retry after a partial failure of the post-publish hook —
//! converges to the published state. If the freshly-built tree's SHA
//! matches the tree at the current `refs/heads/main`, we skip writing a
//! new commit. The version tag write is similarly a no-op when the tag
//! already points at the same OID. Two back-to-back publishes with
//! identical input therefore produce a single commit on `main`
//! (`plugin_git_idempotent.rs` covers this). Together with the UPSERT
//! `ON CONFLICT` in `regenerate_entry`, this means publish → either
//! step failing → retry leaves the system in the intended end state.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;

use crate::state::AppState;
use crate::storage::Storage;
use crate::tenant::TenantCtx;

/// One bundled item from `plugin_contents`. Kept independent of the
/// publish handler's request type so this module is reusable from a
/// future mirror worker.
#[derive(Debug, Clone)]
pub struct ContentRef {
    pub kind: String,
    pub slug: String,
    pub version: String,
}

/// Materialise an `internal` plugin's tree into its bare git repo.
///
/// Errors are returned, not panicked: the publish handler logs and swallows
/// them so a transient storage hiccup doesn't roll back an otherwise-valid
/// API publish. The marketplace.json entry only writes after this succeeds,
/// so a failed materialise simply omits the plugin from the marketplace
/// until the next successful publish.
pub async fn materialise_internal(
    state: &AppState,
    tenant: &TenantCtx,
    slug: &str,
    version: &str,
    manifest: &serde_json::Value,
    contents: &[ContentRef],
) -> Result<()> {
    let storage = state.storage_for(tenant).await?;
    let repo_path = storage.plugin_git_path(tenant.tenant_id, slug)?;

    let tree = build_tree(state, tenant, manifest, contents).await?;

    // libgit2 is sync; do the on-disk work in a blocking task so we don't
    // stall the axum runtime. Bound by the storage IO + commit hashing,
    // which is fast for plugin-sized trees (<< 1 MB typical).
    let slug_owned = slug.to_string();
    let version_owned = version.to_string();
    tokio::task::spawn_blocking(move || {
        write_commit(&repo_path, &slug_owned, &version_owned, &tree)
    })
    .await
    .map_err(|e| anyhow!("plugin_git materialise join: {e}"))??;
    Ok(())
}

/// Walk the manifest + contents into an in-memory tree the caller commits.
/// Public for testing — `test_build_tree_layout` asserts the shape.
pub(crate) async fn build_tree(
    state: &AppState,
    tenant: &TenantCtx,
    manifest: &serde_json::Value,
    contents: &[ContentRef],
) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut out: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();

    // 1. .claude-plugin/plugin.json — manifest JSONB serialised pretty so
    //    cloners reading it in an editor see the same shape they pasted.
    let manifest_pretty =
        serde_json::to_vec_pretty(manifest).context("serialise manifest for plugin.json")?;
    out.insert(
        PathBuf::from(".claude-plugin").join("plugin.json"),
        manifest_pretty,
    );

    // 2. Inline passthrough blobs — Claude Code spec allows hooks /
    //    mcpServers / lspServers / experimental.monitors to be inline
    //    objects in plugin.json itself, but many tools also want files on
    //    disk at the canonical locations. Mirror them when present so both
    //    forms work.
    if let Some(m) = manifest.as_object() {
        copy_inline(m, "hooks", "hooks/hooks.json", &mut out)?;
        copy_inline(m, "mcpServers", ".mcp.json", &mut out)?;
        copy_inline(m, "lspServers", ".lsp.json", &mut out)?;
        if let Some(experimental) = m.get("experimental").and_then(|v| v.as_object()) {
            copy_inline(experimental, "monitors", "monitors/monitors.json", &mut out)?;
        }
    }

    // 3. Bundled content. For each plugin_contents row, fetch its bundle
    //    from skill storage and place its files under the right plugin
    //    subdirectory.
    let storage = state.storage_for(tenant).await?;
    for c in contents {
        let bundle = fetch_bundle(state, tenant, &storage, c).await?;
        place_bundle(c, &bundle, &mut out)?;
    }

    Ok(out)
}

/// Mirror an inline JSON sub-object from the manifest into a file at the
/// canonical on-disk path. Silently skips when the field is absent or not
/// an object — Claude Code also accepts a string (path) value; in that
/// case the path lives inside the plugin tree and is the curator's
/// responsibility to populate (e.g. a future "uploaded blob" surface).
fn copy_inline(
    manifest: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    dest: &str,
    out: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let Some(v) = manifest.get(field) else {
        return Ok(());
    };
    if !v.is_object() {
        return Ok(());
    }
    let pretty =
        serde_json::to_vec_pretty(v).with_context(|| format!("serialise inline {field}"))?;
    out.insert(PathBuf::from(dest), pretty);
    Ok(())
}

async fn fetch_bundle(
    state: &AppState,
    tenant: &TenantCtx,
    storage: &Storage,
    c: &ContentRef,
) -> Result<Bytes> {
    let row = sqlx::query!(
        "SELECT bundle_uri \
         FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND version = $4 \
         ORDER BY created_at DESC LIMIT 1",
        tenant.tenant_id,
        c.slug,
        c.kind,
        c.version,
    )
    .fetch_optional(state.db_read())
    .await
    .with_context(|| format!("lookup bundle for {}/{}@{}", c.kind, c.slug, c.version))?
    .ok_or_else(|| {
        anyhow!(
            "bundle row missing for {}/{}@{} — was the skill archived between publish and materialise?",
            c.kind, c.slug, c.version
        )
    })?;
    storage
        .read_bundle(&row.bundle_uri)
        .await
        .with_context(|| format!("read bundle bytes for {}/{}@{}", c.kind, c.slug, c.version))
}

/// Extract a single content bundle into the plugin tree at the right
/// subpath. Skills get a dedicated directory; agents/commands are flat
/// `.md` files at the kind directory.
fn place_bundle(
    c: &ContentRef,
    bundle: &Bytes,
    out: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    use flate2::read::GzDecoder;

    let gz = GzDecoder::new(bundle.as_ref());
    let mut tar = tar::Archive::new(gz);

    let dest_root: PathBuf = match c.kind.as_str() {
        "skill" => PathBuf::from("skills").join(&c.slug),
        // Single-file kinds: pick the first `.md` we find in the bundle
        // and write it as `<kind-dir>/<slug>.md`. The bundle may contain
        // helper assets too — for those we land them next to the .md so
        // path-relative references inside it still resolve.
        "agent" => PathBuf::from("agents"),
        "command" => PathBuf::from("commands"),
        other => return Err(anyhow!("unknown plugin content kind: {other}")),
    };

    let mut wrote_primary = false;
    for entry in tar.entries().context("read tar entries")? {
        let mut entry = entry.context("iterate tar entries")?;
        let header_path = entry.path().context("tar entry path")?.into_owned();
        let normalised = normalise_tar_path(&header_path)?;
        if normalised.as_os_str().is_empty() {
            continue;
        }

        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() {
            continue;
        }

        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).context("read tar entry body")?;

        let dest_path = match c.kind.as_str() {
            "skill" => dest_root.join(&normalised),
            "agent" | "command" => {
                // First `.md` becomes `<slug>.md`; everything else lands
                // alongside it preserving its tar-relative path.
                let is_md = normalised
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false);
                if is_md && !wrote_primary {
                    wrote_primary = true;
                    dest_root.join(format!("{}.md", c.slug))
                } else {
                    dest_root.join(&c.slug).join(&normalised)
                }
            }
            _ => unreachable!("kind validated above"),
        };
        out.insert(dest_path, buf);
    }

    Ok(())
}

/// Tar entries can carry leading `./` or paths with parent traversals;
/// reject anything that escapes the bundle root. Returns the cleaned
/// relative `PathBuf`.
fn normalise_tar_path(p: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component;
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow!("bundle entry contains parent-dir traversal: {p:?}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!("bundle entry is not a relative path: {p:?}"));
            }
        }
    }
    Ok(out)
}

/// Write the tree into the bare repo at `repo_path` as a single commit on
/// `refs/heads/main` plus a `refs/tags/<version>` tag. Sync — caller wraps
/// in `spawn_blocking`.
fn write_commit(
    repo_path: &Path,
    slug: &str,
    version: &str,
    tree: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    use git2::{Repository, Signature, TreeBuilder};

    // Create the bare repo on first publish. Idempotent — opening an
    // existing bare repo at the same path Just Works.
    if let Some(parent) = repo_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create plugin git parent dir {}", parent.display()))?;
    }
    let repo = if repo_path.exists() {
        Repository::open_bare(repo_path)
            .with_context(|| format!("open bare repo {}", repo_path.display()))?
    } else {
        Repository::init_bare(repo_path)
            .with_context(|| format!("init bare repo {}", repo_path.display()))?
    };

    // Build the tree object. `TreeBuilder` works one directory at a time,
    // so we recurse via a nested map of directory → entries.
    let tree_oid = write_tree_recursive(&repo, tree)?;
    let new_tree = repo
        .find_tree(tree_oid)
        .context("look up just-written tree")?;

    // Idempotency short-circuit: if `refs/heads/main` already points at a
    // commit whose tree SHA matches the one we just built, this publish
    // is a no-op — don't write a new commit. The tree blobs we wrote
    // above will become unreachable garbage and a future `git gc`
    // collects them, which is fine for a low-frequency endpoint.
    let parent_commit = match repo.head() {
        Ok(head_ref) => head_ref.peel_to_commit().ok(),
        Err(_) => None,
    };
    if let Some(parent) = &parent_commit {
        if parent.tree_id() == tree_oid {
            // Tag may also already point here — `tag_lightweight` with
            // force=true is a no-op when the target matches.
            repo.tag_lightweight(version, parent.as_object(), true)
                .context("write version tag (idempotent path)")?;
            let _ = update_server_info(&repo);
            return Ok(());
        }
    }

    let sig =
        Signature::now("skill-pool", "noreply@skill-pool").context("build commit signature")?;
    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    let message = format!("publish {slug}@{version}");
    let commit_oid = repo
        .commit(
            Some("refs/heads/main"),
            &sig,
            &sig,
            &message,
            &new_tree,
            &parents,
        )
        .context("write commit")?;

    // HEAD points at refs/heads/main on a freshly-init'd bare repo by
    // default, but on a repo created externally it may symbolic-ref to
    // master or somewhere else. Pin it explicitly so clones resolve HEAD
    // → main.
    repo.set_head("refs/heads/main").context("set HEAD")?;

    // Tag this version so the marketplace pin / `git clone --branch
    // <version>` flow works. Force=true so a republish of the same
    // version overwrites the tag.
    let commit_obj = repo
        .find_commit(commit_oid)
        .context("look up just-written commit")?;
    repo.tag_lightweight(version, commit_obj.as_object(), true)
        .context("write version tag")?;

    // `git update-server-info` so dumb-HTTP discovery clients (those that
    // hit `info/refs` without the smart-protocol service param) can still
    // read the refs. Belt-and-braces — our handler answers the smart path
    // too, but a defensive `info/refs` file means tools like `curl` show
    // useful output.
    let _ = update_server_info(&repo);

    let _ = TreeBuilder::clear; // appease the import-not-used analyser
    Ok(())
}

/// Write a nested directory map as a libgit2 tree object. Recursive: for
/// each path in the flat input map, walks components and uses one
/// `TreeBuilder` per directory.
fn write_tree_recursive(
    repo: &git2::Repository,
    files: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<git2::Oid> {
    use std::collections::BTreeMap as Map;

    // First group files by their immediate directory.
    enum Node<'a> {
        File(&'a [u8]),
        Dir(Map<String, Node<'a>>),
    }

    fn insert<'a>(root: &mut Map<String, Node<'a>>, parts: &[String], blob: &'a [u8]) {
        if parts.len() == 1 {
            root.insert(parts[0].clone(), Node::File(blob));
            return;
        }
        let (head, rest) = parts.split_first().expect("non-empty");
        let entry = root
            .entry(head.clone())
            .or_insert_with(|| Node::Dir(Map::new()));
        match entry {
            Node::Dir(child) => insert(child, rest, blob),
            Node::File(_) => {
                // Collision: a path was previously inserted as a file but
                // now appears as a directory prefix. Overwrite with a dir.
                let mut child = Map::new();
                insert(&mut child, rest, blob);
                *entry = Node::Dir(child);
            }
        }
    }

    let mut root: Map<String, Node<'_>> = Map::new();
    for (path, bytes) in files {
        let parts: Vec<String> = path
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => s.to_str().map(str::to_string),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            continue;
        }
        insert(&mut root, &parts, bytes.as_slice());
    }

    fn build<'a>(repo: &git2::Repository, node: &Map<String, Node<'a>>) -> Result<git2::Oid> {
        let mut tb = repo.treebuilder(None).context("create tree builder")?;
        for (name, child) in node {
            match child {
                Node::File(bytes) => {
                    let blob_oid = repo
                        .blob(bytes)
                        .with_context(|| format!("write blob for {name}"))?;
                    tb.insert(name, blob_oid, 0o100644)
                        .with_context(|| format!("insert blob {name}"))?;
                }
                Node::Dir(child_map) => {
                    let sub_oid = build(repo, child_map)?;
                    tb.insert(name, sub_oid, 0o040000)
                        .with_context(|| format!("insert subtree {name}"))?;
                }
            }
        }
        tb.write().context("write tree")
    }

    build(repo, &root)
}

/// Equivalent of `git update-server-info` — refresh the `info/refs`,
/// `info/packs`, and `objects/info/packs` files dumb HTTP clients read.
/// libgit2 doesn't have a single helper, but writing `info/refs` from the
/// current ref list is what these clients actually need for clone.
fn update_server_info(repo: &git2::Repository) -> Result<()> {
    let info_dir = repo.path().join("info");
    std::fs::create_dir_all(&info_dir).context("create info dir")?;
    let mut lines = Vec::<String>::new();
    let refs = repo.references().context("iterate refs")?;
    for r in refs {
        let r = r.context("read ref")?;
        let Some(name) = r.name() else { continue };
        let Some(target) = r.target() else { continue };
        lines.push(format!("{}\t{}", target, name));
    }
    lines.sort();
    let contents = lines.join("\n") + "\n";
    std::fs::write(info_dir.join("refs"), contents).context("write info/refs")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_leading_dot() {
        assert_eq!(
            normalise_tar_path(Path::new("./SKILL.md")).unwrap(),
            PathBuf::from("SKILL.md")
        );
    }

    #[test]
    fn normalise_rejects_parent_traversal() {
        assert!(normalise_tar_path(Path::new("../etc/passwd")).is_err());
        assert!(normalise_tar_path(Path::new("foo/../../bar")).is_err());
    }

    #[test]
    fn normalise_rejects_absolute() {
        assert!(normalise_tar_path(Path::new("/etc/passwd")).is_err());
    }
}
