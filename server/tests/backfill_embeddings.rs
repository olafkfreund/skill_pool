//! Phase 5+ integration test: embedding backfill admin task.
//!
//! Publishes a skill with NullEmbedder (so the column is left NULL),
//! then runs `admin::backfill_embeddings` with a deterministic stub
//! embedder, and asserts the column is now populated.

use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::admin;
use skill_pool_server::embedding::{Embedder, NullEmbedder};

struct StubEmbedder;
impl Embedder for StubEmbedder {
    fn embed(&self, text: &str) -> anyhow::Result<Option<Vec<f32>>> {
        // Match the dim of vector(384) — but the content can be tiny.
        // Use lowercase length as a seed so different descriptions
        // produce different vectors.
        let mut v = vec![0.0_f32; 384];
        let lc = text.to_lowercase();
        for (i, b) in lc.bytes().take(384).enumerate() {
            v[i] = (b as f32) / 255.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut v {
            *x /= norm;
        }
        Ok(Some(v))
    }
    fn dimension(&self) -> Option<usize> {
        Some(384)
    }
}

#[tokio::test]
async fn backfill_populates_null_description_embeddings() -> Result<()> {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    // Insert two skills directly with NULL embedding — simulating
    // rows published before Phase 5.
    let (tenant_id,): (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = 'acme'")
            .fetch_one(&pool)
            .await?;
    for (slug, version, desc) in [
        ("axum-handler", "1.0.0", "Pattern for axum extractors"),
        ("kafka-consumer", "1.0.0", "Kafka consumer with backpressure"),
    ] {
        sqlx::query(
            "INSERT INTO skills (tenant_id, slug, version, description, when_to_use, tags, \
                                 bundle_uri, bundle_sha256) \
             VALUES ($1, $2, $3, $4, NULL, '{}', '/fake/key', 'fakehash')",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(version)
        .bind(desc)
        .execute(&pool)
        .await?;
    }

    // Confirm baseline: both rows have NULL embedding.
    let (n_null_before,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skills WHERE description_embedding IS NULL",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(n_null_before, 2);

    // 1. dry-run reports rows but doesn't write.
    let stub: Arc<dyn Embedder> = Arc::new(StubEmbedder);
    admin::backfill_embeddings(&pool, stub.as_ref(), Some("acme"), 10, true).await?;
    let (n_null_after_dry,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skills WHERE description_embedding IS NULL",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(n_null_after_dry, 2, "dry-run must not write");

    // 2. real run populates both rows.
    admin::backfill_embeddings(&pool, stub.as_ref(), Some("acme"), 10, false).await?;
    let (n_null_after,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skills WHERE description_embedding IS NULL",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(n_null_after, 0, "all rows should now have an embedding");

    // 3. NullEmbedder is rejected up front so operators don't waste
    //    cycles on an obvious misconfiguration.
    let null: Arc<dyn Embedder> = Arc::new(NullEmbedder);
    let err = admin::backfill_embeddings(&pool, null.as_ref(), None, 10, false)
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("no embedder configured"), "{msg}");

    Ok(())
}
