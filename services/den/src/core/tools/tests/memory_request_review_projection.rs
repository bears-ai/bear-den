use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::DEN_MEMORY_REQUEST_REVIEW,
        session::{invoke_den_tool_for_origin, DenToolInvocationContext},
    },
    user::db::create_user,
};
use den_core::{ArmatureAvailability, Governance, TurnExecutionOrigin};
use den_service::bears::{db, db::grant_membership, db::BearParams, RuntimeContextLabel};

async fn seed_pair_agent(
    pool: &PgPool,
    bear_id: Uuid,
    agent_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        r"
        INSERT INTO bear_profile_bindings (bear_id, profile, binding_id)
        VALUES ($1, 'pair', $2)
        ON CONFLICT (bear_id, profile)
        DO NOTHING
        ",
    )
    .bind(bear_id)
    .bind(agent_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[sqlx::test]
async fn memory_request_review_projects_typed_conversation_records(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-memory-review-tool-bear",
            name: "Test Memory Review Tool Bear",
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
        &format!("mr-{}@ex.com", &suffix[..8]),
        &format!("mr{}", &suffix[..12]),
        "Memory Review Tester",
        "test-hash",
    )
    .await?;

    grant_membership(&pool, user_id, bear_id, Some("admin")).await?;

    let agent_id = format!("agent-{}", Uuid::new_v4());
    seed_pair_agent(&pool, bear_id, &agent_id).await?;

    let conversation = den_service::conversation::persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        "conv-memory-review-tool-test",
        None,
        None,
    )
    .await?;

    let mut context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-memory-review-tool-bear".to_string(),
        binding_id: agent_id,
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-memory-review-tool-test".to_string(),
        session_id: "client-memory-review-tool-session".to_string(),
        work_run_id: None,
        client_session_id: Some("client-memory-review-tool-session".to_string()),
        conversation_selection: Some("conv-memory-review-tool-test".to_string()),
        runtime_target: None,
        workspace_roots: vec!["/workspace".to_string()],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(Uuid::new_v4().to_string()),
        channel: DenToolChannelContext::default(),
    };

    crate::core::tools::tests::source_fixture::admit_tool_source(&pool, &mut context).await?;
    let hat = den_service::bears::hats::bindings::conversation_hat(
        &pool,
        bear_id.into(),
        conversation.id,
    )
    .await?
    .expect("explicit fixture hat");
    den_service::bears::hats::manage::set_auto_curate_enabled(
        &pool,
        bear_id.into(),
        hat,
        true,
        true,
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
        "Candidate memory summary",
        &json!({}),
    )
    .await?;
    for action in ["promote_to_core", "summarize_into_core"] {
        let retired = invoke_den_tool_for_origin(
            &pool,
            &config,
            &stores,
            DEN_MEMORY_REQUEST_REVIEW,
            json!({
                "source_paths": ["pair/notes/test.md"],
                "title": "Retired promotion",
                "summary": "Candidate memory summary",
                "suggested_action": action,
            }),
            context.clone(),
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            Governance::Interactive,
        )
        .await;
        assert!(
            matches!(retired, Err(crate::errors::CustomError::ValidationError(_))),
            "retired action {action}: {retired:?}"
        );
    }
    let payload = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        DEN_MEMORY_REQUEST_REVIEW,
        json!({
            "source_memory_id": source.memory_id,
            "title": "Promote memory",
            "summary": "Candidate memory summary",
            "suggested_action": "propose_hat"
        }),
        context,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        Governance::Interactive,
    )
    .await?;
    let proposal_id = payload["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .parse::<Uuid>()?;
    let context = den_service::conversation::events::canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(user_id),
        "conv-memory-review-tool-test".to_string(),
        None,
        None,
        "client-memory-review-tool-session".to_string(),
        false,
    );
    den_service::conversation::events::persist_projection(
        &context,
        &den_service::conversation::events::Projection {
            provenance: den_service::conversation::events::ProjectionProvenance {
                source: den_service::conversation::events::ProjectionSource::DenTools,
                scope_id: "client-memory-review-tool-session".to_string(),
            },
            event: den_service::conversation::events::ProjectionEvent::MemoryReviewRequested(
                den_service::conversation::events::MemoryReviewRequestedPayload {
                    proposal_id,
                    source_profile: "pair".to_string(),
                    title: "Promote memory".to_string(),
                    suggested_action: "propose_hat".to_string(),
                    status: "pending".to_string(),
                    source_paths: vec![],
                },
            ),
            workflow_text: "Memory review requested: Promote memory".to_string(),
            visible_summary: Some(
                "Review requested for memory proposal 'Promote memory' from pair.".to_string(),
            ),
        },
    )
    .await?;

    assert_eq!(payload["proposal"]["title"], "Promote memory");

    let messages = den_service::conversation::persistence::list_messages_page(
        &pool,
        conversation.id,
        None,
        20,
    )
    .await?;
    assert!(messages.iter().any(|m| m
        .content_text
        .contains("Memory review requested: Promote memory")));
    assert!(messages.iter().any(|m| m
        .content_text
        .contains("Review requested for memory proposal 'Promote memory' from pair.")));
    assert!(messages.iter().any(|m| m.message_type == "workflow_event"));
    Ok(())
}
