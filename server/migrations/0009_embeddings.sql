-- skill-pool 0009_embeddings
-- Phase 5: embedding-based near-duplicate detection for drafts.
--
-- Requires the pgvector extension. On a self-hosted Postgres without
-- pgvector, the server's NullEmbedder leaves the embedding columns NULL
-- and the dedup query never returns a hit — the schema gracefully
-- degrades. To use dedup, build the server with `--features fastembed`
-- (or wire another Embedder impl) AND install pgvector.
--
-- 384 dimensions matches BGE-small-en-v1.5 (the default FastembedEmbedder
-- model). If you wire a different embedder, build a migration to ALTER
-- the column shape.

CREATE EXTENSION IF NOT EXISTS vector;

-- Embedding of `description` (server-side, computed at insert time).
-- NULL when no embedder is configured, or when an existing row predates
-- the column.
ALTER TABLE skills
    ADD COLUMN description_embedding vector(384);

ALTER TABLE skill_drafts
    ADD COLUMN description_embedding vector(384);

-- When a draft is judged a near-duplicate of an existing skill, the
-- server records the target slug + similarity score so curators can
-- decide whether to merge or publish as a new version.
ALTER TABLE skill_drafts
    ADD COLUMN merge_proposal_skill_id UUID REFERENCES skills(id) ON DELETE SET NULL,
    ADD COLUMN merge_proposal_similarity REAL;

-- HNSW index on skills for fast cosine similarity queries. Drafts are
-- inserted once and queried once at create time — no need to index them.
-- `m=16, ef_construction=64` is the pgvector default and works well for
-- catalogs in the hundreds-of-thousands range.
CREATE INDEX idx_skills_description_embedding_hnsw
    ON skills USING hnsw (description_embedding vector_cosine_ops)
    WHERE description_embedding IS NOT NULL;
