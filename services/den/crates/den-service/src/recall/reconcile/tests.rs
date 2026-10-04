use super::*;
use den_core::{config::Config, ids::UserId};
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn zero_hat_and_first_hat_bears_never_index_legacy_profile_heads(pool: PgPool) {
    use crate::bears::{
        db::{self, BearParams},
        hats,
    };
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('recallhat@example.test', 'recallhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "recallhatbear",
            name: "Recall Hat",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("recall-hat-cutover-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let old = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "note",
        "pair",
        None,
        "old profile text must never leave the Bear",
        &json!({}),
    )
    .await
    .unwrap();
    let shared = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("note"),
        "note",
        "curate",
        None,
        "reviewed Bear text",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "raw"),
        "note",
        "pair",
        None,
        "raw source never exported",
        &json!({}),
    )
    .await
    .unwrap();
    let unowned = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "missing"),
        "note",
        "curate",
        None,
        "a path cannot create hat ownership",
        &json!({}),
    )
    .await
    .unwrap();
    let before = list_authorized_indexable_heads(&pool, &store)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].memory_id, shared.memory_id);
    assert!(!before.iter().any(|head| head.memory_id == old.memory_id));
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Review",
        "Review changes",
    )
    .await
    .unwrap();
    let curated = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "note"),
        "note",
        "curate",
        None,
        "reviewed hat text",
        &json!({}),
    )
    .await
    .unwrap();
    let other_bear = test_bear(&pool, "other-recall-hat-bear").await;
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(other_bear),
        UserId::new(user),
        "Foreign",
        "Another Bear's responsibility",
    )
    .await
    .unwrap();
    let foreign = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(other_hat.id, "foreign"),
        "note",
        "curate",
        None,
        "foreign Bear hat must not be embedded",
        &json!({}),
    )
    .await
    .unwrap();
    let after = list_authorized_indexable_heads(&pool, &store)
        .await
        .unwrap();
    assert!(!after.iter().any(|head| head.memory_id == old.memory_id));
    assert!(after.iter().any(|head| head.memory_id == shared.memory_id));
    assert!(after.iter().any(|head| head.memory_id == curated.memory_id));
    assert_eq!(after.len(), 2);
    assert!(!after.iter().any(|head| head.memory_id == foreign.memory_id));
    assert!(!after
        .iter()
        .any(|head| head.memory_id == raw.memory_id || head.memory_id == unowned.memory_id));

    config.qdrant_url = Some("http://127.0.0.1:1".into());
    let watermark = super::super::watermark::recall_watermark(&pool, &config, &store)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        watermark.lag_count, 2,
        "only the eligible core and owned hat heads are pending"
    );
    assert_eq!(watermark.indexed_seq, shared.sequence_no - 1);
    assert_eq!(
        den_memory::fetch_record_by_id(&store, &old.memory_id)
            .await
            .unwrap()
            .unwrap()
            .content_text,
        old.content_text
    );
}

#[tokio::test]
async fn reconcile_indexes_curated_hat_heads_but_not_private_source_notes() {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("index-hat-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(Uuid::new_v4()).await.unwrap();
    let hat = HatId::new(Uuid::new_v4());
    let current = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "note"),
        "note",
        "curate",
        None,
        "safe reviewed hat note",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note"),
        "note",
        "pair",
        None,
        "private source note",
        &json!({}),
    )
    .await
    .unwrap();
    let _archive_candidate = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "candidate"),
        "note",
        "curate",
        None,
        "archive candidate must not be embedded",
        &json!({"lifecycle": {"status": "archive-candidate"}}),
    )
    .await
    .unwrap();
    let _archived = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "old"),
        "old",
        "curate",
        None,
        "archived note",
        &json!({"lifecycle": {"status": "archived"}}),
    )
    .await
    .unwrap();
    let gated = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "restricted"),
        "restricted",
        "curate",
        None,
        "restricted hat entry must not be embedded",
        &json!({}),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memory_access_rules (link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, author_profile, created_at)
         VALUES (?, ?, ?, ?, ?, 'confined_to', 'curate', '2026-09-30T00:00:00Z')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(store.bear_id().to_string())
    .bind(store.next_sequence().await.unwrap())
    .bind(&gated.memory_id)
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap();
    let heads = list_indexable_heads(&store).await.unwrap();
    assert!(heads.iter().any(|head| {
        head.memory_id == current.memory_id && head.scope_hat_id == Some(hat) && head.is_indexable()
    }));
    assert!(!heads.iter().any(|head| head.memory_id == raw.memory_id));
    assert!(!heads.iter().any(|head| head.memory_id == gated.memory_id));
    assert!(!heads
        .iter()
        .any(|head| head.content_text == "archived note"));
    assert!(!heads
        .iter()
        .any(|head| head.content_text.contains("archive candidate")));
}

struct NoEmbedding;

impl PassageEmbedder for NoEmbedding {
    fn dimensions(&self) -> u32 {
        8
    }

    async fn embed(&self, _inputs: &[String]) -> Result<Vec<Vec<f32>>, DenError> {
        panic!("zero-hat legacy/source memory must never reach an embedding provider")
    }
}

async fn test_bear(pool: &PgPool, slug: &str) -> Uuid {
    crate::bears::db::create_bear(
        pool,
        crate::bears::db::BearParams {
            slug,
            name: "Recall isolation",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

async fn canonical_snapshot(
    store: &BearMemoryStore,
) -> Vec<(
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
)> {
    // sqlx-dynamic: fixed test inspection of the per-Bear SQLite store.
    sqlx::query_as(
        "SELECT memory_id, scope_type, scope_profile, scope_hat_id, scope_source_kind,
                scope_source_id, content_text, metadata_json FROM memory_records ORDER BY sequence_no",
    ).fetch_all(store.pool()).await.unwrap()
}

// A tiny local Qdrant deletion boundary; no live service or embedding provider.
fn deletion_server(expected: usize) -> (String, std::thread::JoinHandle<Vec<serde_json::Value>>) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bodies = Vec::new();
        while bodies.len() < expected {
            assert!(Instant::now() < deadline, "missing Qdrant deletion request");
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("accept: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let (header_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    assert!(headers
                        .lines()
                        .next()
                        .unwrap()
                        .contains("/points/delete?wait=true"));
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            bodies.push(serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap());
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        }
        bodies
    });
    (url, handle)
}

#[sqlx::test(migrations = "../../migrations")]
async fn zero_hat_reconcile_cleans_stale_raw_points_without_embedding_or_changing_history(
    pool: PgPool,
) {
    let bear_id = test_bear(&pool, "zero-hat-recall-isolation").await;
    let mut config = Config::test_stub();
    let data_dir = std::env::temp_dir().join(format!("recall-zero-hat-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = data_dir.to_string_lossy().into_owned();
    config.embedding_dimensions = 8;
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let legacy = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "old"),
        "note",
        "pair",
        None,
        "raw historical profile secret",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "raw"),
        "note",
        "pair",
        None,
        "raw historical source secret",
        &json!({}),
    )
    .await
    .unwrap();
    let missing = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "unowned"),
        "note",
        "curate",
        None,
        "unowned hat locator",
        &json!({}),
    )
    .await
    .unwrap();
    let unowned_request = list_indexable_heads(&store)
        .await
        .unwrap()
        .into_iter()
        .find(|head| head.memory_id == missing.memory_id)
        .unwrap();
    // Even a core locator cannot claim scope; a hat path cannot create a
    // missing Bear-owned hat. SQLite itself forbids hat rows with a NULL ID.
    sqlx::query("UPDATE memory_records SET logical_path = 'core/forged.md' WHERE memory_id = ?")
        .bind(&legacy.memory_id)
        .execute(store.pool())
        .await
        .unwrap();

    let original = canonical_snapshot(&store).await;
    for record in [&legacy, &raw] {
        registry::upsert_passage(
            &pool,
            registry::NewPassage {
                bear_id,
                memory_id: &record.memory_id,
                logical_path: record.logical_path.as_deref(),
                chunk_index: 0,
                content_hash: "old-derived-hash",
                embedding_standard: &config.embedding_standard,
                source_class: super::super::policy::SOURCE_CLASS_BEAR_MEMORY,
                point_id: &record.memory_id,
            },
        )
        .await
        .unwrap();
    }
    assert!(list_authorized_indexable_heads(&pool, &store)
        .await
        .unwrap()
        .is_empty());
    // Failed deletion must leave live registry entries for the next reconcile.
    config.qdrant_url = Some("http://127.0.0.1:1".into());
    let unavailable = QdrantRecall::from_config(&config).unwrap();
    let indexer = RecallIndexer::new(
        &pool,
        &unavailable,
        &NoEmbedding,
        &config.embedding_standard,
    );
    assert!(
        indexer
            .index_record(&unowned_request)
            .await
            .unwrap()
            .skipped_not_indexable
    );
    assert!(reconcile_bear(
        &pool,
        &unavailable,
        &NoEmbedding,
        &store,
        &config.embedding_standard
    )
    .await
    .is_err());
    assert_eq!(
        registry::list_indexed_memory_ids(&pool, bear_id, &config.embedding_standard)
            .await
            .unwrap()
            .len(),
        2
    );
    let (url, server) = deletion_server(2);
    config.qdrant_url = Some(url);
    let qdrant = QdrantRecall::from_config(&config).unwrap();
    let outcome = reconcile_bear(
        &pool,
        &qdrant,
        &NoEmbedding,
        &store,
        &config.embedding_standard,
    )
    .await
    .unwrap();
    assert_eq!(outcome.indexed_records, 0);
    assert_eq!(outcome.embedded_chunks, 0);
    assert_eq!(outcome.removed_records, 2);
    assert_eq!(outcome.removed_points, 2);
    let deleted: std::collections::HashSet<String> = server
        .join()
        .unwrap()
        .into_iter()
        .flat_map(|body| {
            body["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        deleted,
        [legacy.memory_id, raw.memory_id].into_iter().collect()
    );
    assert!(
        registry::list_indexed_memory_ids(&pool, bear_id, &config.embedding_standard)
            .await
            .unwrap()
            .is_empty()
    );
    let watermark = super::super::watermark::recall_watermark(&pool, &config, &store)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(watermark.lag_count, 0);
    assert!(watermark.fully_recallable);
    assert_eq!(canonical_snapshot(&store).await, original);
    let second = reconcile_bear(
        &pool,
        &qdrant,
        &NoEmbedding,
        &store,
        &config.embedding_standard,
    )
    .await
    .unwrap();
    assert_eq!(second, ReconcileOutcome::default());
    drop(store);
    drop(stores);
    std::fs::remove_dir_all(data_dir).unwrap();
}
