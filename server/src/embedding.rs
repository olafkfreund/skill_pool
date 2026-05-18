//! Embedding service (Phase 5).
//!
//! `Embedder` is the pluggable seam. Two production-side implementations:
//!
//!  - [`NullEmbedder`] — default; returns `None` for every input. The
//!    schema columns stay NULL and dedup queries never return a hit.
//!    Builds and tests work without pgvector or HuggingFace network.
//!  - [`FastembedEmbedder`] — gated behind the `fastembed` Cargo feature.
//!    Lazy-loads `bge-small-en-v1.5` on first call (auto-downloads ~30MB
//!    to the OS cache). Produces 384-dim vectors that match the
//!    `vector(384)` column in the `0009_embeddings` migration.
//!
//! Tests inject their own deterministic impl (see `EmbeddingProvider`
//! examples in `server/tests/embedding_dedup.rs`).

use std::sync::Arc;

use anyhow::Result;

/// Object-safe trait so we can erase the concrete embedder into the
/// shared `AppState`. Implementations should be cheap to clone (typically
/// `Arc` wrappers around a backing client).
pub trait Embedder: Send + Sync {
    /// Embed a single text. Returns `None` if no embedder is configured
    /// (`NullEmbedder`), in which case callers MUST treat dedup as
    /// disabled — they should not block the path.
    fn embed(&self, text: &str) -> Result<Option<Vec<f32>>>;

    /// Vector dimension when embedding is enabled. Used by the dedup
    /// query to assert the schema matches; returns `None` for the null
    /// implementation.
    fn dimension(&self) -> Option<usize>;
}

/// Default — no-op. Dedup short-circuits to "no match found".
#[derive(Debug, Default, Clone)]
pub struct NullEmbedder;

impl Embedder for NullEmbedder {
    fn embed(&self, _text: &str) -> Result<Option<Vec<f32>>> {
        Ok(None)
    }
    fn dimension(&self) -> Option<usize> {
        None
    }
}

/// Cosine similarity threshold above which a draft is treated as a
/// merge proposal pointing at the matching skill. From the master plan.
pub const DEDUP_SIMILARITY_THRESHOLD: f32 = 0.85;

/// Convenience wrapper so `AppState` holds a `SharedEmbedder` without
/// requiring callers to write the trait-object syntax every time.
pub type SharedEmbedder = Arc<dyn Embedder>;

/// Construct the default embedder for a given config. With the
/// `fastembed` feature off, this is always `NullEmbedder`. With it on,
/// honours `config.embedding.enabled`.
pub fn from_config(cfg: &crate::config::EmbeddingConfig) -> Result<SharedEmbedder> {
    if !cfg.enabled {
        return Ok(Arc::new(NullEmbedder));
    }
    #[cfg(feature = "fastembed")]
    {
        let e = fastembed_impl::FastembedEmbedder::new()?;
        return Ok(Arc::new(e));
    }
    // Feature off but operator asked for it — be loud, then fall back.
    #[allow(unreachable_code)]
    {
        tracing::warn!(
            "embedding.enabled=true but the server was built without --features fastembed; \
             dedup will be disabled"
        );
        Ok(Arc::new(NullEmbedder))
    }
}

// ---------------------------------------------------------------------------
// Fastembed impl (feature-gated to keep default builds light)
// ---------------------------------------------------------------------------

#[cfg(feature = "fastembed")]
mod fastembed_impl {
    use super::*;
    use std::sync::Mutex;

    /// 384-dim BGE-small-en-v1.5. Lazy-loaded on first call so server
    /// startup stays fast and the model download only happens when the
    /// feature is actually exercised.
    pub struct FastembedEmbedder {
        // Mutex because the underlying fastembed::TextEmbedding holds
        // ONNX session state that isn't Sync. Embedding is fast enough
        // (~10ms on CPU) that contention isn't a real concern.
        model: Mutex<Option<fastembed::TextEmbedding>>,
    }

    impl FastembedEmbedder {
        pub fn new() -> Result<Self> {
            // Defer the actual download to first use.
            Ok(Self {
                model: Mutex::new(None),
            })
        }

        fn ensure_loaded(&self) -> Result<()> {
            let mut guard = self.model.lock().expect("embedder mutex poisoned");
            if guard.is_some() {
                return Ok(());
            }
            tracing::info!("loading fastembed BGE-small-en-v1.5 (first use, ~30MB)…");
            let model = fastembed::TextEmbedding::try_new(
                fastembed::InitOptions::new(fastembed::EmbeddingModel::BGESmallENV15),
            )?;
            *guard = Some(model);
            Ok(())
        }
    }

    impl Embedder for FastembedEmbedder {
        fn embed(&self, text: &str) -> Result<Option<Vec<f32>>> {
            self.ensure_loaded()?;
            let guard = self.model.lock().expect("embedder mutex poisoned");
            let model = guard.as_ref().expect("model loaded above");
            let mut vecs = model.embed(vec![text], None)?;
            Ok(vecs.pop())
        }
        fn dimension(&self) -> Option<usize> {
            Some(384)
        }
    }
}

#[cfg(feature = "fastembed")]
pub use fastembed_impl::FastembedEmbedder;

// ---------------------------------------------------------------------------
// Serialization helpers for pgvector
// ---------------------------------------------------------------------------

/// Render a vector in the `[v1,v2,…]` literal syntax accepted by pgvector
/// when binding as `text` and casting `::vector` in the query. We use this
/// form rather than pulling in the `pgvector` crate as a dependency — it's
/// one less moving part and the query plan is identical.
pub fn vector_to_pg_literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // pgvector accepts standard float syntax. Use {:.6} to keep the
        // string small while preserving more than enough precision for
        // cosine similarity.
        s.push_str(&format!("{x:.6}"));
    }
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_embedder_returns_none() {
        let e = NullEmbedder;
        assert!(e.embed("hello").unwrap().is_none());
        assert_eq!(e.dimension(), None);
    }

    #[test]
    fn vector_literal_round_shape() {
        let lit = vector_to_pg_literal(&[0.1, -0.5, 1.0]);
        assert!(lit.starts_with('['));
        assert!(lit.ends_with(']'));
        assert_eq!(lit.matches(',').count(), 2);
    }
}
