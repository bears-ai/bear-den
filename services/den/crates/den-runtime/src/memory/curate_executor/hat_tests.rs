use super::*;
use den_core::ids::{BearId, UserId};
use den_service::{
    bears::{
        db::{self, BearParams},
        hats,
    },
    memory_proposals::CreateMemoryProposal,
};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn autonomous_curate_does_not_publish_legacy_proposals_to_core_for_configured_bears(
    pool: PgPool,
) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('curatehat@example.test', 'curatehat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "curatehatbear",
            name: "Hat curation",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Review",
        "Review facts",
    )
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("curate-hat-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let proposal = crate::memory::create_proposal(
        &pool,
        &config,
        &stores,
        CreateMemoryProposal {
            bear_id,
            source_profile: BearProfile::Pair,
            source_agent_id: None,
            source_paths: vec!["pair/private.md".into()],
            source_refs: json!({}),
            suggested_action: "promote_to_core",
            target_ref: Some("core/knowledge.md".into()),
            title: "Potentially private candidate",
            summary: "A summary long enough to trigger legacy automatic curation",
            rationale: "not verified",
            proposed_content: Some("Do not disclose this to Work"),
            proposed_patch: None,
            refs: json!({}),
            sensitivity: "normal",
            requires_human: false,
            project_to_conversation: false,
        },
    )
    .await
    .unwrap();
    let run =
        execute_memory_curate_proposals(&pool, &config, &stores, bear_id, None, &[proposal.id])
            .await
            .unwrap();
    assert_eq!(run.outcomes.len(), 1);
    assert_eq!(run.outcomes[0].status, "needs_human_review");
    assert_eq!(run.outcomes[0].triage, "escalate_human");
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let shared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'shared'",
    )
    .bind(bear_id.to_string())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(shared, 0);
}
