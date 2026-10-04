use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{tools::memory_review::DenMemoryReviewStore, user::db::create_user};
use den_core::tools::review::{
    MemoryProposalResolution, MemoryReviewStore, ProposalProjection, ResolveProposalRequest,
};
use den_service::bears::{db, db::grant_membership, db::BearParams, RuntimeContextLabel};
use den_service::memory_proposals::CreateMemoryProposal;

#[sqlx::test]
async fn memory_resolve_proposal_projects_typed_conversation_records(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-memory-resolve-tool-bear",
            name: "Test Memory Resolve Tool Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;

    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = create_user(
        &pool,
        &format!("rs-{}@ex.com", &suffix[..8]),
        &format!("rs{}", &suffix[..12]),
        "Memory Resolve Tester",
        "test-hash",
    )
    .await?;
    grant_membership(&pool, user_id, bear_id, Some("admin")).await?;

    let agent_id = format!("agent-{}", Uuid::new_v4());

    let conversation = den_service::conversation::persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        "conv-memory-resolve-tool-test",
        None,
        None,
    )
    .await?;

    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await?;
    let source = den_memory::append_memory_record(
        &store,
        &den_memory::LogicalMemoryPath::source_local(
            den_memory::MemorySource::Conversation(conversation.id),
            "note",
        ),
        "note",
        "pair",
        None,
        "Candidate finding",
        &json!({}),
    )
    .await?;
    let proposal = den_runtime::memory::create_proposal(
        &pool,
        &config,
        &stores,
        CreateMemoryProposal {
            bear_id,
            source_profile: RuntimeContextLabel::ArmatureConversation,
            source_agent_id: None,
            source_paths: vec![source.logical_path.clone().expect("source-local path")],
            source_refs: json!({
                "conversation_id": "conv-memory-resolve-tool-test",
                "session_id": "client-memory-resolve-tool-session"
            }),
            suggested_action: "promote_to_core",
            target_ref: None,
            title: "Resolve me",
            summary: "candidate",
            rationale: "because",
            proposed_content: None,
            proposed_patch: None,
            refs: json!({}),
            sensitivity: "normal",
            requires_human: false,
            project_to_conversation: false,
        },
    )
    .await?;

    let review = DenMemoryReviewStore::new(&pool, &config, &stores);
    let resolved = review
        .resolve_proposal(ResolveProposalRequest {
            bear_id,
            reviewer_profile: RuntimeContextLabel::Curation,
            binding_id: agent_id,
            proposal_id: proposal.id,
            status: MemoryProposalResolution::Rejected,
            review_notes: None,
            decision_summary: Some("Not suitable".to_string()),
            projection: ProposalProjection {
                user_id,
                conversation_id: Some("conv-memory-resolve-tool-test".to_string()),
                scope_id: "client-memory-resolve-tool-session".to_string(),
            },
        })
        .await?;
    assert_eq!(resolved["status"], "rejected");
    let persisted = review.get_proposal(bear_id, proposal.id).await?.unwrap();
    assert_eq!(persisted["status"], "rejected");
    let canonical = den_memory::get_memory_proposal(&store, &proposal.id.to_string())
        .await?
        .expect("canonical proposal");
    assert_eq!(canonical.payload_json["decision_summary"], "Not suitable");

    let projection_context = den_service::conversation::events::canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(user_id),
        "conv-memory-resolve-tool-test".to_string(),
        None,
        None,
        "client-memory-resolve-tool-session".to_string(),
        false,
    );
    den_service::conversation::events::persist_projection(
        &projection_context,
        &den_service::conversation::events::memory_proposal_resolved_projection(
            den_service::conversation::events::ProjectionProvenance {
                source: den_service::conversation::events::ProjectionSource::DenTools,
                scope_id: "client-memory-resolve-tool-session".to_string(),
            },
            proposal.id,
            "pair",
            "promote_to_core",
            "Resolve me",
            "rejected",
            Some("curate".to_string()),
            None,
            None,
        ),
    )
    .await?;

    let messages = den_service::conversation::persistence::list_messages_page(
        &pool,
        conversation.id,
        None,
        20,
    )
    .await?;

    assert!(messages.iter().any(|m| m
        .content_text
        .contains("Memory proposal resolved: Resolve me (rejected)")));
    assert!(messages.iter().any(|m| m
        .content_text
        .contains("Memory proposal 'Resolve me' was rejected.")));
    Ok(())
}
