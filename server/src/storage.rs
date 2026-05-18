//! Bundle storage. opendal abstracts the backend so the same code paths work
//! against a local filesystem, S3, GCS, Azure Blob, or MinIO. Phase 1 wires FS
//! and documents the URI scheme for swapping in S3 without code changes.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use opendal::Operator;
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
}
