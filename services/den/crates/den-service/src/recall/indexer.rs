//! Recall indexer orchestration (ADR-0038 Phase 1): turn a canonical memory record into
//! Qdrant passages, with content-hash dedup and delete-on-supersede, tracked in the
//! Postgres `recall_passages` registry.
//!
//! The indexer is embedder-agnostic via [`PassageEmbedder`] so it can run against the live
//! Bifrost embedding client in production and a deterministic stub in tests (no API key).

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use den_core::{ids::BearId, DenError};
use den_llm::EmbeddingClient;

use super::chunking::chunk_text;
use super::policy::{build_payload, point_id, IndexRequest, SOURCE_CLASS_BEAR_MEMORY};
use super::qdrant::{QdrantPoint, QdrantRecall};
use super::registry;

/// Abstraction over the embedding backend so the indexer can be tested without an API key.
// Native async fn in trait: workspace-internal, consumed via generic bounds /
// concrete impls only (never `dyn`), so Send flows through monomorphization.
#[allow(async_fn_in_trait)]
pub trait PassageEmbedder: Send + Sync {
    fn dimensions(&self) -> u32;
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, DenError>;
}

impl PassageEmbedder for EmbeddingClient {
    fn dimensions(&self) -> u32 {
        EmbeddingClient::dimensions(self)
    }
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, DenError> {
        EmbeddingClient::embed(self, inputs).await
    }
}

/// What an `index_record` call did (for diagnostics + tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexOutcome {
    pub embedded_chunks: usize,
    pub reused_chunks: usize,
    pub removed_points: usize,
    pub skipped_not_indexable: bool,
}

/// Indexes canonical memory records into the active recall collection.
pub struct RecallIndexer<'a, E: PassageEmbedder> {
    pool: &'a PgPool,
    qdrant: &'a QdrantRecall,
    embedder: &'a E,
    embedding_standard: String,
}

impl<'a, E: PassageEmbedder> RecallIndexer<'a, E> {
    pub fn new(
        pool: &'a PgPool,
        qdrant: &'a QdrantRecall,
        embedder: &'a E,
        embedding_standard: impl Into<String>,
    ) -> Self {
        Self {
            pool,
            qdrant,
            embedder,
            embedding_standard: embedding_standard.into(),
        }
    }

    /// Index (or re-index) a single canonical memory record. Idempotent: unchanged chunks
    /// are reused (no re-embed), changed chunks overwrite their point, and a record that is
    /// no longer indexable (policy or empty) has its passages removed.
    pub async fn index_record(&self, req: &IndexRequest) -> Result<IndexOutcome, DenError> {
        let std = &self.embedding_standard;

        let owned_hat = match req.scope_hat_id {
            Some(id) if req.is_indexable() => {
                crate::bears::hats::list_hats(self.pool, BearId::new(req.bear_id))
                    .await?
                    .iter()
                    .any(|hat| hat.id == id)
            }
            Some(_) => false,
            None => true,
        };
        if !req.is_indexable() || !owned_hat {
            let removed = self.remove_record(req.bear_id, &req.memory_id).await?;
            return Ok(IndexOutcome {
                skipped_not_indexable: true,
                removed_points: removed,
                ..IndexOutcome::default()
            });
        }

        let chunks = chunk_text(&req.content_text);
        if chunks.is_empty() {
            let removed = self.remove_record(req.bear_id, &req.memory_id).await?;
            return Ok(IndexOutcome {
                removed_points: removed,
                ..IndexOutcome::default()
            });
        }

        let existing = registry::list_passages(self.pool, req.bear_id, &req.memory_id, std).await?;
        let existing_by_chunk: HashMap<i32, String> = existing
            .into_iter()
            .map(|p| (p.chunk_index, p.content_hash))
            .collect();

        // Decide which chunks need (re)embedding.
        let mut to_embed: Vec<usize> = Vec::new();
        let mut reused = 0usize;
        for (pos, chunk) in chunks.iter().enumerate() {
            match existing_by_chunk.get(&chunk.index) {
                Some(hash) if hash == &chunk.content_hash => reused += 1,
                _ => to_embed.push(pos),
            }
        }

        let mut embedded = 0usize;
        if !to_embed.is_empty() {
            let texts: Vec<String> = to_embed.iter().map(|&p| chunks[p].text.clone()).collect();
            let vectors = self.embedder.embed(&texts).await?;
            if vectors.len() != texts.len() {
                return Err(DenError::System(format!(
                    "embedder returned {} vectors for {} chunks",
                    vectors.len(),
                    texts.len()
                )));
            }

            let mut points = Vec::with_capacity(to_embed.len());
            for (&chunk_pos, vector) in to_embed.iter().zip(vectors) {
                let chunk = &chunks[chunk_pos];
                let pid = point_id(req.bear_id, &req.memory_id, chunk.index, std);
                points.push(QdrantPoint {
                    id: pid,
                    vector,
                    payload: build_payload(req, chunk, std),
                });
            }
            self.qdrant.upsert_points(&points).await?;

            for (vec_pos, &chunk_pos) in to_embed.iter().enumerate() {
                let chunk = &chunks[chunk_pos];
                registry::upsert_passage(
                    self.pool,
                    registry::NewPassage {
                        bear_id: req.bear_id,
                        memory_id: &req.memory_id,
                        logical_path: req.logical_path.as_deref(),
                        chunk_index: chunk.index,
                        content_hash: &chunk.content_hash,
                        embedding_standard: std,
                        source_class: SOURCE_CLASS_BEAR_MEMORY,
                        point_id: &points[vec_pos].id,
                    },
                )
                .await?;
            }
            embedded = to_embed.len();
        }

        // Prune chunks that no longer exist (content shrank).
        let new_count = chunks.len() as i32;
        let stale = registry::delete_passages_for_chunks_ge(
            self.pool,
            req.bear_id,
            &req.memory_id,
            std,
            new_count,
        )
        .await?;
        let removed_points = stale.len();
        if !stale.is_empty() {
            self.qdrant.delete_points(&stale).await?;
        }

        Ok(IndexOutcome {
            embedded_chunks: embedded,
            reused_chunks: reused,
            removed_points,
            skipped_not_indexable: false,
        })
    }

    /// Remove all passages for a memory record (on supersede/delete). Returns points removed.
    pub async fn remove_record(&self, bear_id: Uuid, memory_id: &str) -> Result<usize, DenError> {
        let points: Vec<String> =
            registry::list_passages(self.pool, bear_id, memory_id, &self.embedding_standard)
                .await?
                .into_iter()
                .map(|passage| passage.point_id)
                .collect();
        let removed = points.len();
        if !points.is_empty() {
            // Keep the registry live until Qdrant acknowledges deletion, so a
            // transport failure leaves reconciliation able to retry cleanup.
            self.qdrant.delete_points(&points).await?;
            registry::delete_passages_for_memory(
                self.pool,
                bear_id,
                memory_id,
                &self.embedding_standard,
            )
            .await?;
        }
        Ok(removed)
    }
}

/// Deterministic embedder for tests/reconcile-without-LLM: maps text → a fixed-dimension
/// vector seeded by a content hash. Not for production semantic quality.
#[cfg(any(test, feature = "test-util"))]
#[derive(Clone)]
pub struct DeterministicEmbedder {
    dimensions: u32,
}

#[cfg(any(test, feature = "test-util"))]
impl DeterministicEmbedder {
    pub fn new(dimensions: u32) -> Self {
        Self { dimensions }
    }

    fn vector_for(&self, text: &str) -> Vec<f32> {
        let hash = super::chunking::content_hash(text);
        let seed_bytes = hash.as_bytes();
        (0..self.dimensions as usize)
            .map(|i| {
                let b = seed_bytes[i % seed_bytes.len()];
                (f32::from(b) / 255.0) - 0.5
            })
            .collect()
    }
}

#[cfg(any(test, feature = "test-util"))]
impl PassageEmbedder for DeterministicEmbedder {
    fn dimensions(&self) -> u32 {
        self.dimensions
    }
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, DenError> {
        Ok(inputs.iter().map(|t| self.vector_for(t)).collect())
    }
}
