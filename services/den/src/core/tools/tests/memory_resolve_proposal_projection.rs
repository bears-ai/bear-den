use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    tools::{
        arguments::DenToolChannelContext, context::DenToolContext,
        session::DenToolInvocationContext,
    },
    user::db::create_user,
};
use den_core::tools::review::resolve_memory_proposal;
use den_service::bears::{db, db::grant_membership, db::BearParams, BearProfile};
use den_service::memory_proposals::CreateMemoryProposal;

async fn seed_curate_agent(
    pool: &PgPool,
    bear_id: Uuid,
    agent_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        r"
        INSERT INTO bear_profile_bindings (bear_id, profile, binding_id)
        VALUES ($1, 'curate', $2)
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
    seed_curate_agent(&pool, bear_id, &agent_id).await?;

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
    let proposal = den_runtime::memory::create_proposal(
        &pool,
        &config,
        &stores,
        CreateMemoryProposal {
            bear_id,
            source_profile: BearProfile::Pair,
            source_agent_id: Some("agent-pair".to_string()),
            source_paths: vec!["pair/notes/test.md".to_string()],
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

    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-memory-resolve-tool-bear".to_string(),
        binding_id: agent_id,
        profile: Some(BearProfile::Curate),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-memory-resolve-tool-test".to_string(),
        session_id: "client-memory-resolve-tool-session".to_string(),
        work_run_id: None,
        client_session_id: Some("client-memory-resolve-tool-session".to_string()),
        conversation_selection: Some("conv-memory-resolve-tool-test".to_string()),
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

    let tool_context = DenToolContext::new(&pool, &config, &stores);
    let payload = resolve_memory_proposal(
        &tool_context,
        &context,
        BearProfile::Curate,
        json!({
            "proposal_id": proposal.id,
            "status": "rejected",
            "decision_summary": "Not suitable"
        }),
    )
    .await?;

    assert_eq!(payload["proposal"]["status"], "rejected");

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
