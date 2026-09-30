//! Opt-in real-Qdrant boundary test. Vectors are deterministic and synthetic;
//! neither Bifrost nor an external embedding provider is contacted.

use super::*;
use crate::{
    bears::{db, hats},
    recall::{
        indexer::{DeterministicEmbedder, PassageEmbedder},
        qdrant::QdrantPoint,
        reconcile::reconcile_bear,
        registry,
    },
};
use den_core::ids::{BearId, UserId};
use den_memory::{
    append_memory_record, mark_memory_record_lifecycle, LogicalMemoryPath, MemorySource,
    MemoryStoreManager,
};
use serde_json::json;
use sqlx::PgPool;

async fn note(
    store: &den_memory::BearMemoryStore,
    path: LogicalMemoryPath,
    content: &str,
) -> Result<den_memory::MemoryRecordRow, DenError> {
    append_memory_record(store, &path, "note", "curate", None, content, &json!({})).await
}

fn memory_filter(bear_id: Uuid, memory_id: &str) -> Value {
    json!({"must": [
        {"key": "bear_id", "match": {"value": bear_id.to_string()}},
        {"key": "memory_id", "match": {"value": memory_id}}
    ]})
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires BEARS_RECALL_TEST_QDRANT_URL and a running Qdrant"]
async fn real_qdrant_reconciles_first_hat_and_rechecks_member_hits(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let qdrant_url = std::env::var("BEARS_RECALL_TEST_QDRANT_URL")
        .expect("set BEARS_RECALL_TEST_QDRANT_URL to run the live Qdrant test");
    let mut config = Config::test_stub();
    config.qdrant_url = Some(qdrant_url);
    config.embedding_dimensions = 8;
    config.embedding_standard = format!("recall-test-{}", Uuid::new_v4().simple());
    let data_dir = std::env::temp_dir().join(format!("recall-qdrant-test-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = data_dir.to_string_lossy().into_owned();
    let qdrant = QdrantRecall::from_config(&config).expect("explicit test URL");
    assert!(
        qdrant.readyz().await?,
        "Qdrant must be ready for this opt-in test"
    );
    let stores = MemoryStoreManager::new(&config);
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "real-qdrant-hat-recall",
            name: "Recall test Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('qdrant-hat@example.test', 'qdranthat') RETURNING id"
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        qdrant.ensure_collection().await?,
        "use a new isolated collection"
    );
    let store = stores.store_for_bear(bear_id).await?;
    let embedder = DeterministicEmbedder::new(config.embedding_dimensions);
    let standard = &config.embedding_standard;

    let result: Result<(), Box<dyn std::error::Error>> = async {
        let legacy = note(
            &store,
            LogicalMemoryPath::profile_local("pair", "old"),
            "Synthetic legacy note; remove its derived point at first hat",
        )
        .await?;
        let shared = note(
            &store,
            LogicalMemoryPath::shared_core("shared"),
            "Synthetic Bear-wide canonical fact",
        )
        .await?;
        let raw = note(
            &store,
            LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "raw"),
            "Synthetic private conversation note; never index",
        )
        .await?;
        let before = reconcile_bear(&pool, &qdrant, &embedder, &store, standard).await?;
        assert_eq!(before.indexed_records, 2);
        assert_eq!(
            qdrant
                .count_with_filter(memory_filter(bear_id, &legacy.memory_id))
                .await?,
            1
        );
        assert_eq!(
            qdrant
                .count_with_filter(memory_filter(bear_id, &raw.memory_id))
                .await?,
            0
        );

        let hat = hats::create_hat(
            &pool,
            BearId::new(bear_id),
            UserId::new(user_id),
            "Review",
            "Synthetic tested memory",
        )
        .await?;
        let other = hats::create_hat(
            &pool,
            BearId::new(bear_id),
            UserId::new(user_id),
            "Other",
            "Another synthetic hat",
        )
        .await?;
        let content = "Synthetic reviewed security note for this hat";
        let curated = note(&store, LogicalMemoryPath::hat(hat.id, "reviewed"), content).await?;
        let foreign = note(
            &store,
            LogicalMemoryPath::hat(other.id, "reviewed"),
            "Synthetic reviewed note for another hat",
        )
        .await?;
        let after = reconcile_bear(&pool, &qdrant, &embedder, &store, standard).await?;
        assert_eq!(after.indexed_records, 3);
        assert_eq!(
            after.removed_records, 1,
            "first hat removes the old profile head"
        );
        assert_eq!(
            qdrant
                .count_with_filter(memory_filter(bear_id, &legacy.memory_id))
                .await?,
            0
        );
        assert!(
            registry::list_passages(&pool, bear_id, &legacy.memory_id, standard)
                .await?
                .is_empty()
        );
        assert_eq!(
            qdrant
                .count_with_filter(memory_filter(bear_id, &curated.memory_id))
                .await?,
            1
        );
        assert_eq!(
            qdrant
                .count_with_filter(memory_filter(bear_id, &raw.memory_id))
                .await?,
            0
        );

        let grant = CuratedMemoryGrant::new(vec![hat.id]);
        let filter = curated_scope_filter(bear_id, standard, &grant);
        let vector = embedder.embed(&[content.to_string()]).await?.remove(0);
        let hits = qdrant.search(&vector, filter.clone(), 10).await?;
        assert!(hits
            .iter()
            .any(|hit| hit.payload["memory_id"] == curated.memory_id));
        assert!(hits
            .iter()
            .any(|hit| hit.payload["memory_id"] == shared.memory_id));
        assert!(!hits
            .iter()
            .any(|hit| hit.payload["memory_id"] == foreign.memory_id));
        assert!(!hits
            .iter()
            .any(|hit| hit.payload["memory_id"] == legacy.memory_id));
        assert!(!hits
            .iter()
            .any(|hit| hit.payload["memory_id"] == raw.memory_id));

        // Poison the *derived* Qdrant payload without touching canonical SQLite.
        let mut poisoned = hits
            .iter()
            .find(|hit| hit.payload["memory_id"] == curated.memory_id)
            .expect("curated point")
            .payload
            .clone();
        poisoned["text"] = json!("INJECTED_VECTOR_PAYLOAD");
        poisoned["logical_path"] = json!("source_memory/forged.md");
        qdrant
            .upsert_points(&[QdrantPoint {
                id: hits
                    .iter()
                    .find(|hit| hit.payload["memory_id"] == curated.memory_id)
                    .expect("curated point")
                    .id
                    .clone(),
                vector: vector.clone(),
                payload: poisoned,
            }])
            .await?;
        let mut projection =
            search_passages(&qdrant, &embedder, filter.clone(), standard, content, 10).await?;
        assert!(projection
            .passages
            .iter()
            .any(|p| p.text.contains("INJECTED_VECTOR_PAYLOAD")));
        retain_curated_candidates(&store, &grant, &mut projection, 10).await?;
        assert!(projection
            .passages
            .iter()
            .any(|p| p.memory_id == curated.memory_id && p.text == content));
        assert!(!projection.passages.iter().any(
            |p| p.text.contains("INJECTED_VECTOR_PAYLOAD") || p.memory_id == foreign.memory_id
        ));

        // A stale Qdrant point is not authority after canonical archive, even if
        // reconciliation has not yet deleted that point.
        mark_memory_record_lifecycle(&store, &curated.memory_id, "archived", Some("test")).await?;
        let mut stale = search_passages(&qdrant, &embedder, filter, standard, content, 10).await?;
        assert!(stale
            .passages
            .iter()
            .any(|p| p.memory_id == curated.memory_id));
        retain_curated_candidates(&store, &grant, &mut stale, 10).await?;
        assert!(!stale
            .passages
            .iter()
            .any(|p| p.memory_id == curated.memory_id));
        assert!(stale
            .passages
            .iter()
            .any(|p| p.memory_id == shared.memory_id));
        Ok(())
    }
    .await;

    let cleanup = reqwest::Client::new()
        .delete(format!(
            "{}/collections/{}",
            qdrant.base_url(),
            qdrant.collection_name()
        ))
        .send()
        .await?
        .error_for_status();
    drop(store);
    drop(stores);
    std::fs::remove_dir_all(data_dir)?;
    result?;
    cleanup?;
    Ok(())
}
