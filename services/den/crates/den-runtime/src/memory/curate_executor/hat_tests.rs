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
async fn retired_core_proposals_are_rejected_without_publication_or_human_queue(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('curatehat@example.test', 'curatehat') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("curate-hat-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);

    for has_hat in [false, true] {
        let bear_id = db::create_bear(
            &pool,
            BearParams {
                slug: if has_hat {
                    "curatehatbear"
                } else {
                    "curatenohatbear"
                },
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
        if has_hat {
            hats::create_hat(
                &pool,
                BearId::new(bear_id),
                UserId::new(user),
                "Review",
                "Review facts",
            )
            .await
            .unwrap();
        }
        for action in ["promote_to_core", "summarize_into_core"] {
            let proposal = crate::memory::create_proposal(
                &pool,
                &config,
                &stores,
                CreateMemoryProposal {
                    bear_id,
                    source_profile: RuntimeContextLabel::ArmatureConversation,
                    source_agent_id: None,
                    source_paths: vec!["pair/private.md".into()],
                    source_refs: json!({}),
                    suggested_action: action,
                    target_ref: Some("core/knowledge.md"),
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
            let run = execute_memory_curate_proposals(
                &pool,
                &config,
                &stores,
                bear_id,
                None,
                &[proposal.id],
            )
            .await
            .unwrap();
            assert_eq!(run.outcomes.len(), 1);
            assert_eq!(run.outcomes[0].status, "rejected");
            assert_eq!(run.outcomes[0].triage, "reject");
            assert!(run.briefing.is_empty(), "no human or deferred review queue");
            let saved = crate::memory::get_proposal(&pool, &config, &stores, bear_id, proposal.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.status, "rejected");
            let store = stores.store_for_bear(bear_id).await.unwrap();
            let sqlite = den_memory::get_memory_proposal(&store, &proposal.id.to_string())
                .await
                .unwrap()
                .unwrap();
            assert!(sqlite.payload_json["review_notes"]
                .as_str()
                .unwrap_or("")
                .contains("retired"));
            assert!(sqlite.payload_json["result_path"].is_null());
        }
        let store = stores.store_for_bear(bear_id).await.unwrap();
        let shared: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'shared'",
        )
        .bind(bear_id.to_string())
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(
            shared, 0,
            "no-hat and hat Bears must both block publication"
        );
    }
}
