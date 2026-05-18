//! Bundle storage. opendal abstracts the backend so the same code paths work
//! against a local filesystem, S3, GCS, Azure Blob, or MinIO. Phase 1 wires FS
//! and documents the URI scheme for swapping in S3 without code changes.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use opendal::{ErrorKind, Operator};
use uuid::Uuid;

const PRESIGN_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct Storage {
    op: Operator,
}

impl Storage {
    /// Build a storage backend from a URI.
    ///
    /// Supported schemes:
    /// - `fs:///absolute/path` — local filesystem
    /// - `s3://bucket?endpoint=...&access_key=...&secret_key=...&region=...` — S3-compatible
    pub fn from_uri(uri: &str) -> Result<Self> {
        let parsed = url::Url::parse(uri).with_context(|| format!("parse storage URI: {uri}"))?;

        let op = match parsed.scheme() {
            "fs" => {
                let root = parsed.path();
                if root.is_empty() || root == "/" {
                    return Err(anyhow!("fs:// storage URI must include a root path"));
                }
                std::fs::create_dir_all(root)
                    .with_context(|| format!("create storage root {root}"))?;
                let builder = opendal::services::Fs::default().root(root);
                Operator::new(builder)?.finish()
            }
            other => {
                return Err(anyhow!(
                    "storage scheme `{other}` not supported in Phase 1 (use `fs://...`); \
                     S3 and friends come online when CI wires real credentials"
                ));
            }
        };

        Ok(Self { op })
    }

    /// Object key for a bundle. Tenant-namespaced so even a leaked URL can't
    /// enumerate another tenant's bundles.
    pub fn bundle_key(tenant_id: Uuid, slug: &str, version: &str) -> String {
        format!("{tenant_id}/{slug}/{version}.tar.gz")
    }

    /// Object key for a draft bundle. Drafts live under a separate prefix so
    /// (a) they don't compete with published skill versioning and (b) the
    /// promote-to-skill path copies into the canonical key without juggling
    /// overwrites. `draft_id` is the row UUID from `skill_drafts`.
    pub fn draft_bundle_key(tenant_id: Uuid, draft_id: Uuid) -> String {
        format!("{tenant_id}/drafts/{draft_id}.tar.gz")
    }

    pub async fn put_bundle(&self, key: &str, bytes: Bytes) -> Result<()> {
        self.op
            .write(key, bytes)
            .await
            .with_context(|| format!("write bundle {key}"))?;
        Ok(())
    }

    pub async fn read_bundle(&self, key: &str) -> Result<Bytes> {
        let buf = self
            .op
            .read(key)
            .await
            .with_context(|| format!("read bundle {key}"))?;
        Ok(buf.to_bytes())
    }

    /// Returns a short-lived signed URL when the backend supports it,
    /// otherwise `None` — caller streams via `read_bundle`.
    #[allow(dead_code)] // wired into get_bundle once S3 lands
    pub async fn presign_read(&self, key: &str) -> Result<Option<String>> {
        match self.op.presign_read(key, PRESIGN_TTL).await {
            Ok(req) => Ok(Some(req.uri().to_string())),
            Err(e) if e.kind() == opendal::ErrorKind::Unsupported => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Cheap liveness probe for the health endpoint.
    ///
    /// Issues `stat("")` — a directory/root entry check — which resolves to
    /// an `lstat(2)` on fs backends and a `HeadObject` on S3. Both are
    /// read-only and do not enumerate or modify any objects. Returns the
    /// round-trip latency in milliseconds on success, or an `opendal::Error`
    /// on failure (including `ErrorKind::Unsupported` which the health handler
    /// folds into `"off"`).
    pub async fn probe(&self) -> Result<u64, opendal::Error> {
        let t = Instant::now();
        // stat("") resolves to the root of the operator — always present on
        // any writable backend. An Unsupported error means the backend has no
        // stat capability (e.g. a hypothetical write-only adapter); the caller
        // surfaces that as "off" rather than "down".
        self.op.stat("").await?;
        Ok(t.elapsed().as_millis() as u64)
    }

    /// Expose the backend's error kind so the health handler can distinguish
    /// "truly broken" from "stat not supported on this adapter".
    pub fn unsupported_kind() -> ErrorKind {
        ErrorKind::Unsupported
    }
}
