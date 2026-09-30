use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::DEN_MEMORY_APPLY_CORE_UPDATE,
        session::{invoke_den_tool, DenToolInvocationContext},
    },
    user::db::create_user,
};
use den_service::bears::{db, db::grant_membership, db::BearParams, BearProfile};
use den_service::memory_proposals::{create, CreateMemoryProposal};

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
async fn memory_apply_core_update_projects_typed_conversation_records(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-memory-apply-tool-bear",
            name: "Test Memory Apply Tool Bear",
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
        &format!("ac-{}@ex.com", &suffix[..8]),
        &format!("ac{}", &suffix[..12]),
        "Memory Apply Tester",
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
        "conv-memory-apply-tool-test",
        None,
        None,
    )
    .await?;

    let proposal = create(
        &pool,
        CreateMemoryProposal {
            bear_id,
            source_profile: BearProfile::Pair,
            source_agent_id: Some("agent-pair".to_string()),
            source_paths: vec!["pair/notes/test.md".to_string()],
            source_refs: json!({
                "conversation_id": "conv-memory-apply-tool-test",
                "session_id": "client-memory-apply-tool-session"
            }),
            suggested_action: "promote_to_core",
            target_ref: None,
            title: "Apply me",
            summary: "candidate",
            rationale: "because",
            proposed_content: Some("Body text"),
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
        bear_slug: "test-memory-apply-tool-bear".to_string(),
        binding_id: agent_id,
        profile: Some(BearProfile::Curate),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-memory-apply-tool-test".to_string(),
        session_id: "client-memory-apply-tool-session".to_string(),
        work_run_id: None,
        client_session_id: Some("client-memory-apply-tool-session".to_string()),
        conversation_selection: Some("conv-memory-apply-tool-test".to_string()),
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
    den_service::bears::hats::create_hat(
        &pool,
        den_core::ids::BearId::new(bear_id),
        den_core::ids::UserId::new(user_id),
        "Review",
        "Review source provenance",
    )
    .await?;
    let blocked = invoke_den_tool(
        &pool,
        &config,
        &stores,
        DEN_MEMORY_APPLY_CORE_UPDATE,
        json!({
            "proposal_id": proposal.id,
            "target_path": "core/notes.md",
            "mode": "create_file",
            "body": "This unverified proposal must not become Bear-wide memory",
        }),
        context.clone(),
    )
    .await;
    assert!(matches!(
        blocked,
        Err(crate::errors::CustomError::Authorization(ref message))
            if message.contains("canonical hat knowledge")
    ), "configured hats must require canonical admin review: {blocked:?}");
    let store = stores.store_for_bear(bear_id).await?;
    let core_records: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'shared'",
    )
    .bind(bear_id.to_string())
    .fetch_one(store.pool())
    .await?;
    assert_eq!(core_records, 0);
    let payload = invoke_den_tool(
        &pool,
        &config,
        &stores,
        DEN_MEMORY_APPLY_CORE_UPDATE,
        json!({
            "proposal_id": proposal.id,
            "target_path": "core/notes.md",
            "mode": "append",
            "title": "Apply me"
        }),
        context,
    )
    .await;

    assert!(payload.is_err());

    let projection_context = den_service::conversation::events::canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(user_id),
        "conv-memory-apply-tool-test".to_string(),
        None,
        None,
        "client-memory-apply-tool-session".to_string(),
        false,
    );
    den_service::conversation::events::persist_projection(
        &projection_context,
        &den_service::conversation::events::memory_proposal_resolved_projection(
            den_service::conversation::events::ProjectionProvenance {
                source: den_service::conversation::events::ProjectionSource::DenTools,
                scope_id: "client-memory-apply-tool-session".to_string(),
            },
            proposal.id,
            "pair",
            "promote_to_core",
            "Apply me",
            "approved",
            Some("curate".to_string()),
            Some("core/notes.md".to_string()),
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

    assert!(messages.iter().any(|m| m.content_text.contains("Memory proposal resolved: Apply me (approved)")));
    assert!(messages.iter().any(|m| m.content_text.contains("Memory proposal 'Apply me' was approved and applied at core/notes.md.")));
    Ok(())
}
