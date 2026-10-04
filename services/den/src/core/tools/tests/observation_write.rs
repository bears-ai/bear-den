use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{config::Config, core::tools::memory_review::DenMemoryReviewStore};
use den_core::tools::review::{MemoryReviewStore, ObservationWriteRequest};
use den_memory::MemoryStoreManager;
use den_service::bears::{db, db::BearParams};

#[sqlx::test]
async fn observation_store_persists_and_enqueues_memory_curate(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-observation-write-bear",
            name: "Test Observation Write Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let review = DenMemoryReviewStore::new(&pool, &config, &stores);
    let observation_id = "deploy-failure-001";
    let source = json!({ "intake_event_id": Uuid::new_v4(), "origin": "test_worker" });
    let record = review
        .record_observation(ObservationWriteRequest {
            bear_id,
            binding_id: "observation-worker".to_string(),
            observation_id: observation_id.to_string(),
            summary: "Deployment pipeline failed on main.".to_string(),
            salience: "normal".to_string(),
            payload_ref: None,
            source: source.clone(),
            conversation_id: None,
            session_id: None,
            request_id: None,
        })
        .await?;
    assert_eq!(record.observation_id, observation_id);
    assert_eq!(record.status, "review_queued");
    let proposal_id = record.proposal_id.expect("queued proposal");
    let proposal = review.get_proposal(bear_id, proposal_id).await?.unwrap();
    assert_eq!(proposal["requires_human"], false);

    let queued = sqlx::query_scalar::<_, i64>(
        r"
        SELECT COUNT(*)::bigint
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane = 'memory_curate'
          AND trigger = 'watch_observation'
        ",
    )
    .bind(bear_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(queued, 1);

    let replay = review
        .find_observation(bear_id, observation_id)
        .await?
        .unwrap();
    assert_eq!(replay.proposal_id, Some(proposal_id));
    assert_eq!(replay.status, "review_queued");
    let persisted =
        den_runtime::memory::get_observation(&pool, &config, &stores, bear_id, observation_id)
            .await?
            .unwrap();
    assert_eq!(persisted.source, source);
    assert!(review
        .find_observation(Uuid::new_v4(), observation_id)
        .await?
        .is_none());
    Ok(())
}
