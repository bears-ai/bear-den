use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::{DEN_MEMORY_APPLY_CORE_UPDATE, DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER},
        session::{invoke_den_tool_for_origin, DenToolInvocationContext},
    },
    user::db::create_user,
};
use den_core::tools::{
    descriptor::builtin_den_tool_descriptor_for_provider_name,
    dispatch::has_native_session_executor,
};
use den_core::{ArmatureAvailability, Governance, TurnExecutionOrigin};
use den_service::bears::{db, db::grant_membership, db::BearParams, BearProfile};

#[sqlx::test]
async fn retired_core_update_tool_is_not_advertised_or_executable_without_hats(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "test-retired-memory-apply-tool-bear",
            name: "Retired Memory Apply Tool Bear",
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
    sqlx::query(
        "INSERT INTO bear_profile_bindings (bear_id, profile, binding_id)
         VALUES ($1, 'curate', $2) ON CONFLICT (bear_id, profile) DO NOTHING",
    )
    .bind(bear_id)
    .bind(&agent_id)
    .execute(&pool)
    .await?;

    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-retired-memory-apply-tool-bear".to_string(),
        binding_id: agent_id,
        profile: Some(BearProfile::Pair),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-memory-apply-tool-test".to_string(),
        session_id: "client-memory-apply-tool-session".to_string(),
        work_run_id: None,
        client_session_id: Some("client-memory-apply-tool-session".to_string()),
        conversation_selection: None,
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
    assert!(builtin_den_tool_descriptor_for_provider_name(DEN_MEMORY_APPLY_CORE_UPDATE).is_none());
    assert!(
        builtin_den_tool_descriptor_for_provider_name(DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER)
            .is_none()
    );
    assert!(!has_native_session_executor(DEN_MEMORY_APPLY_CORE_UPDATE));

    for has_hat in [false, true] {
        if has_hat {
            den_service::bears::hats::create_hat(
                &pool,
                den_core::ids::BearId::new(bear_id),
                den_core::ids::UserId::new(user_id),
                "Review",
                "Review source provenance",
            )
            .await?;
        }
        for tool_name in [
            DEN_MEMORY_APPLY_CORE_UPDATE,
            DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER,
        ] {
            let blocked = invoke_den_tool_for_origin(
                &pool,
                &config,
                &stores,
                tool_name,
                json!({
                    "proposal_id": Uuid::new_v4(),
                    "target_path": "core/notes.md",
                    "mode": "create_file",
                    "body": "This proposal must not become Bear-wide memory",
                }),
                context.clone(),
                TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                Governance::Interactive,
            )
            .await;
            assert!(
                matches!(blocked, Err(crate::errors::CustomError::NotFound(_))),
                "retired tool {tool_name} unexpectedly accepted: {blocked:?}"
            );
            let mut internal_context = context.clone();
            internal_context.profile = Some(BearProfile::Curate);
            let internal = invoke_den_tool_for_origin(
                &pool,
                &config,
                &stores,
                tool_name,
                json!({}),
                internal_context,
                TurnExecutionOrigin::InternalCuration,
                Governance::AutonomousContinuation,
            )
            .await;
            assert!(
                matches!(internal, Err(crate::errors::CustomError::Authorization(_))),
                "internal route for {tool_name} unexpectedly accepted: {internal:?}"
            );
        }
    }
    let store = stores.store_for_bear(bear_id).await?;
    let core_records: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'shared'",
    )
    .bind(bear_id.to_string())
    .fetch_one(store.pool())
    .await?;
    assert_eq!(core_records, 0);
    Ok(())
}
