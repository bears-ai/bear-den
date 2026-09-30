use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::DEN_MEMORY_REQUEST_REVIEW,
        session::{invoke_den_tool, DenToolInvocationContext},
    },
    user::db::create_user,
};
use den_service::bears::{db, db::grant_membership, db::BearParams, BearProfile};

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

    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-memory-review-tool-bear".to_string(),
        binding_id: agent_id,
        profile: Some(BearProfile::Pair),
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

    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    for action in ["promote_to_core", "summarize_into_core"] {
        let retired = invoke_den_tool(
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
        )
        .await;
        assert!(matches!(retired, Err(crate::errors::CustomError::ValidationError(_))), "retired action {action}: {retired:?}");
    }
    let payload = invoke_den_tool(
        &pool,
        &config,
        &stores,
        DEN_MEMORY_REQUEST_REVIEW,
        json!({
            "source_paths": ["pair/notes/test.md"],
            "title": "Promote memory",
            "summary": "Candidate memory summary",
            "suggested_action": "unspecified"
        }),
        context,
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
                    suggested_action: "unspecified".to_string(),
                    status: "pending".to_string(),
                    source_paths: vec!["pair/notes/test.md".to_string()],
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
    assert!(messages.iter().any(|m| m.content_text.contains("Memory review requested: Promote memory")));
    assert!(messages.iter().any(|m| m.content_text.contains("Review requested for memory proposal 'Promote memory' from pair.")));
    assert!(messages.iter().any(|m| m.message_type == "workflow_event"));
    Ok(())
}
