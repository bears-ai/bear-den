use crate::{
    config::Config,
    core::tools::{
        arguments::DenToolChannelContext, memory_read::memory_status_value,
        prompt_memory::DenPromptMemoryStore, session::DenToolInvocationContext,
    },
    errors::CustomError,
};
use den_service::bears::RuntimeContextLabel;
use den_service::prompt_memory_block_store::{upsert_prompt_memory_block, PromptMemoryBlockWrite};
use den_service::prompt_memory_blocks::{
    PromptMemoryBlockScope, PromptMemoryBlockState, PromptMemoryBlockType,
};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

async fn bind_prompt_source(pool: &PgPool, bear_id: Uuid) -> (i32, String) {
    let nonce = Uuid::new_v4().simple().to_string();
    let user_id = crate::core::user::db::create_user(
        pool,
        &format!("prompt-{nonce}@example.test"),
        &format!("prompt{}", &nonce[..12]),
        "Prompt fixture",
        "test-hash",
    )
    .await
    .expect("create prompt owner");
    den_service::bears::db::grant_membership(pool, user_id, bear_id, Some("admin"))
        .await
        .expect("grant prompt owner membership");
    let conversation = den_service::conversation::persistence::ensure_conversation_for_external_id(
        pool,
        bear_id,
        Some(user_id),
        "conv-test",
        Some("sess-test"),
        None,
    )
    .await
    .expect("create owned canonical conversation");
    let hat = den_service::bears::hats::create_hat(
        pool,
        bear_id.into(),
        user_id.into(),
        "Prompt memory",
        "Test session prompt memory",
    )
    .await
    .expect("create explicit hat");
    den_service::bears::hats::bindings::bind_conversation_hat(
        pool,
        bear_id.into(),
        conversation.id,
        hat.id,
    )
    .await
    .expect("bind prompt hat");
    (
        user_id,
        den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(conversation.id)
            .binding_id(bear_id.into()),
    )
}

// Sibling test helpers: these mirror the dispatcher's `DenToolContext` wiring
// (concrete `DenPromptMemoryStore` + the relocated `den-tools` executors) so the
// store-backed round-trips below stay covered without a production-side wrapper.
async fn prompt_memory_upsert(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, CustomError> {
    den_core::tools::prompt_memory::prompt_memory_upsert(
        &DenPromptMemoryStore::new(pool),
        context,
        role,
        arguments,
    )
    .await
    .map_err(CustomError::from)
}

async fn prompt_memory_list(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, CustomError> {
    den_core::tools::prompt_memory::prompt_memory_list(
        &DenPromptMemoryStore::new(pool),
        context,
        role,
        arguments,
    )
    .await
    .map_err(CustomError::from)
}

async fn prompt_memory_patch(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, CustomError> {
    den_core::tools::prompt_memory::prompt_memory_patch(
        &DenPromptMemoryStore::new(pool),
        context,
        role,
        arguments,
    )
    .await
    .map_err(CustomError::from)
}

#[tokio::test]
async fn prompt_memory_tools_round_trip_through_store() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("Postgres URL for prompt-memory round trip");
    let pool = PgPoolOptions::new()
        .connect(&database_url)
        .await
        .expect("connect Postgres");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate Postgres");
    let slug = format!("prompt-round-trip-{}", Uuid::new_v4().simple());
    let bear_id = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: &slug,
            name: "Prompt Round Trip Test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let (user_id, binding_id) = bind_prompt_source(&pool, bear_id).await;
    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-bear".to_string(),
        binding_id,
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("sess-test".to_string()),
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
        request_id: Some("req-test".to_string()),
        channel: DenToolChannelContext {
            family: Some("armature".to_string()),
            client: Some("zed".to_string()),
            protocol: Some("armature".to_string()),
        },
    };
    let upsert = prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": format!("pm-{}", Uuid::new_v4()),
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "Current focus",
            "body": "Prioritize persisted prompt memory runtime wiring.",
            "priority": 7
        }),
    )
    .await
    .expect("upsert prompt memory block");
    assert_eq!(upsert["status"], "ok");
    let block_id = upsert["block_id"].as_str().unwrap().to_string();
    let listed = prompt_memory_list(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({}),
    )
    .await
    .expect("list prompt memory blocks");
    assert!(listed["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["id"] == block_id));
    let patched = prompt_memory_patch(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": block_id,
            "state": "archived",
            "title": "Current focus (archived)",
            "body": "Archived prompt memory block.",
            "priority": 1
        }),
    )
    .await
    .expect("patch prompt memory block");
    assert_eq!(patched["state"], "archived");
    let listed_active = prompt_memory_list(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({}),
    )
    .await
    .expect("list active prompt memory blocks");
    assert!(!listed_active["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["id"] == patched["block_id"]));
    let listed_all = prompt_memory_list(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({"include_archived": true}),
    )
    .await
    .expect("list all prompt memory blocks");
    assert!(listed_all["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["id"] == patched["block_id"]));
}

#[tokio::test]
async fn prompt_memory_runtime_selection_prefers_session_then_surface_then_role_then_bear() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("Postgres URL for prompt-memory selection test");
    let pool = PgPoolOptions::new()
        .connect(&database_url)
        .await
        .expect("connect Postgres");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate Postgres");
    let slug = format!("prompt-selection-{}", Uuid::new_v4().simple());
    let bear_id = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: &slug,
            name: "Prompt Selection Test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let profile_slug = RuntimeContextLabel::ArmatureConversation.as_str();
    let session_id = format!("sess-{}", Uuid::new_v4());
    let work_surface = format!("ws-{}", Uuid::new_v4());
    let ids = [
        format!("pm-bear-{}", Uuid::new_v4()),
        format!("pm-role-{}", Uuid::new_v4()),
        format!("pm-surface-{}", Uuid::new_v4()),
        format!("pm-session-{}", Uuid::new_v4()),
    ];
    let writes = vec![
        PromptMemoryBlockWrite {
            block_id: ids[0].clone(),
            bear_id: Some(bear_id),
            profile_slug: Some(profile_slug.to_string()),
            scope: PromptMemoryBlockScope::BearWide,
            block_type: PromptMemoryBlockType::RoleGuidance,
            state: PromptMemoryBlockState::Active,
            work_surface: None,
            session_id: None,
            title: "Bear".to_string(),
            body: "Bear-wide guidance".to_string(),
            priority: 1,
            created_by_user_id: Some(1),
            supersedes_block_id: None,
            metadata: json!({}),
        },
        PromptMemoryBlockWrite {
            block_id: ids[1].clone(),
            bear_id: Some(bear_id),
            profile_slug: Some(profile_slug.to_string()),
            scope: PromptMemoryBlockScope::RoleLocal,
            block_type: PromptMemoryBlockType::RoleGuidance,
            state: PromptMemoryBlockState::Active,
            work_surface: None,
            session_id: None,
            title: "Role".to_string(),
            body: "Role guidance".to_string(),
            priority: 1,
            created_by_user_id: Some(1),
            supersedes_block_id: None,
            metadata: json!({}),
        },
        PromptMemoryBlockWrite {
            block_id: ids[2].clone(),
            bear_id: Some(bear_id),
            profile_slug: Some(profile_slug.to_string()),
            scope: PromptMemoryBlockScope::WorkSurface,
            block_type: PromptMemoryBlockType::WorkSurfaceContext,
            state: PromptMemoryBlockState::Active,
            work_surface: Some(work_surface.clone()),
            session_id: None,
            title: "Surface".to_string(),
            body: "Work-surface context".to_string(),
            priority: 1,
            created_by_user_id: Some(1),
            supersedes_block_id: None,
            metadata: json!({}),
        },
        PromptMemoryBlockWrite {
            block_id: ids[3].clone(),
            bear_id: Some(bear_id),
            profile_slug: Some(profile_slug.to_string()),
            scope: PromptMemoryBlockScope::Session,
            block_type: PromptMemoryBlockType::SessionFocus,
            state: PromptMemoryBlockState::Active,
            work_surface: None,
            session_id: Some(session_id.clone()),
            title: "Session".to_string(),
            body: "Session focus".to_string(),
            priority: 1,
            created_by_user_id: Some(1),
            supersedes_block_id: None,
            metadata: json!({}),
        },
    ];
    for write in &writes {
        upsert_prompt_memory_block(&pool, write)
            .await
            .expect("seed prompt memory block");
    }
    let selection =
        den_service::prompt_memory_block_store::select_prompt_memory_blocks_for_runtime(
            &pool,
            den_service::prompt_memory_block_store::PromptMemoryBlockQuery {
                bear_id: Some(bear_id),
                profile_slug,
                session_id: &session_id,
                work_surfaces: std::slice::from_ref(&work_surface),
                visibility: den_service::prompt_memory_block_store::PromptMemoryVisibility::Legacy,
            },
        )
        .await
        .expect("runtime selection");
    let compiled = den_service::prompt_memory_blocks::compile_prompt_memory_blocks(
        &selection.blocks,
        den_service::prompt_memory_blocks::PromptMemoryCompilationInput {
            role: profile_slug,
            work_surfaces: std::slice::from_ref(&work_surface),
            session_id: &session_id,
            max_blocks: 4,
        },
    );
    let included_ids = compiled
        .included_blocks
        .iter()
        .map(|block| block.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        included_ids,
        vec![
            ids[3].clone(),
            ids[2].clone(),
            ids[1].clone(),
            ids[0].clone()
        ]
    );
    assert_eq!(selection.diagnostic["matched_count"], 4);
    for (visibility, expected) in [
        (
            den_service::prompt_memory_block_store::PromptMemoryVisibility::BoundSession,
            vec![ids[0].clone(), ids[3].clone()],
        ),
        (
            den_service::prompt_memory_block_store::PromptMemoryVisibility::SharedOnly,
            vec![ids[0].clone()],
        ),
    ] {
        let selection =
            den_service::prompt_memory_block_store::select_prompt_memory_blocks_for_runtime(
                &pool,
                den_service::prompt_memory_block_store::PromptMemoryBlockQuery {
                    bear_id: Some(bear_id),
                    profile_slug,
                    session_id: &session_id,
                    work_surfaces: std::slice::from_ref(&work_surface),
                    visibility,
                },
            )
            .await
            .expect("bound prompt selection");
        let chosen: Vec<String> = selection.blocks.into_iter().map(|block| block.id).collect();
        assert_eq!(chosen.len(), expected.len());
        for id in expected {
            assert!(chosen.contains(&id), "missing {id}");
        }
    }
}

#[tokio::test]
async fn prompt_memory_upsert_archives_superseded_block() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("Postgres URL for prompt-memory archive test");
    let pool = match PgPoolOptions::new().connect(&database_url).await {
        Ok(pool) => pool,
        Err(_) => return,
    };
    if sqlx::migrate!("./migrations").run(&pool).await.is_err() {
        return;
    }
    let slug = format!("prompt-archive-{}", Uuid::new_v4().simple());
    let bear_id = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: &slug,
            name: "Prompt archive fixture",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let (user_id, binding_id) = bind_prompt_source(&pool, bear_id).await;
    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-bear".to_string(),
        binding_id,
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("sess-test".to_string()),
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
        request_id: Some("req-test".to_string()),
        channel: DenToolChannelContext {
            family: Some("armature".to_string()),
            client: Some("zed".to_string()),
            protocol: Some("armature".to_string()),
        },
    };
    let original_block_id = format!("pm-original-{}", Uuid::new_v4());
    prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": original_block_id,
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "Original focus",
            "body": "Original body",
            "priority": 3
        }),
    )
    .await
    .expect("upsert original block");
    let replacement = prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": format!("pm-replacement-{}", Uuid::new_v4()),
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "Replacement focus",
            "body": "Replacement body",
            "priority": 5,
            "supersedes_block_id": original_block_id
        }),
    )
    .await
    .expect("upsert replacement block");
    assert_eq!(
        replacement["conflicting_archived"].as_u64().unwrap()
            + replacement["superseded_archived"].as_u64().unwrap(),
        1,
    );
    let listed_all = prompt_memory_list(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({"include_archived": true}),
    )
    .await
    .expect("list prompt memory blocks");
    let original = listed_all["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["id"] == original_block_id)
        .expect("original block should still be listed when archived included");
    assert_eq!(original["state"], "archived");
}

#[tokio::test]
async fn prompt_memory_upsert_archives_conflicting_active_block_in_same_scope() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("Postgres URL for prompt-memory archive test");
    let pool = match PgPoolOptions::new().connect(&database_url).await {
        Ok(pool) => pool,
        Err(_) => return,
    };
    if sqlx::migrate!("./migrations").run(&pool).await.is_err() {
        return;
    }
    let slug = format!("prompt-archive-{}", Uuid::new_v4().simple());
    let bear_id = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: &slug,
            name: "Prompt archive fixture",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let (user_id, binding_id) = bind_prompt_source(&pool, bear_id).await;
    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-bear".to_string(),
        binding_id,
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("sess-test".to_string()),
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
        request_id: Some("req-test".to_string()),
        channel: DenToolChannelContext {
            family: Some("armature".to_string()),
            client: Some("zed".to_string()),
            protocol: Some("armature".to_string()),
        },
    };
    prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": format!("pm-conflict-a-{}", Uuid::new_v4()),
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "First focus",
            "body": "First body",
            "priority": 3
        }),
    )
    .await
    .expect("upsert first block");
    let replacement = prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": format!("pm-conflict-b-{}", Uuid::new_v4()),
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "Second focus",
            "body": "Second body",
            "priority": 8
        }),
    )
    .await
    .expect("upsert second block");
    assert_eq!(replacement["conflicting_archived"], 1);
    let active = prompt_memory_list(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test"
        }),
    )
    .await
    .expect("list active session prompt memory blocks");
    assert_eq!(active["count"], 1);
}

#[tokio::test]
async fn memory_status_includes_prompt_memory_diagnostic_summary() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("Postgres URL for memory-status test");
    let pool = match PgPoolOptions::new().connect(&database_url).await {
        Ok(pool) => pool,
        Err(_) => return,
    };
    if sqlx::migrate!("./migrations").run(&pool).await.is_err() {
        return;
    }
    let slug = format!("memory-status-{}", Uuid::new_v4().simple());
    let bear_id = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: &slug,
            name: "Memory Status Test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let (user_id, binding_id) = bind_prompt_source(&pool, bear_id).await;
    let context = DenToolInvocationContext {
        bear_id,
        bear_slug: "test-bear".to_string(),
        binding_id,
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: Some("tester".to_string()),
        membership_role: Some("owner".to_string()),
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("sess-test".to_string()),
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
        request_id: Some("req-test".to_string()),
        channel: DenToolChannelContext {
            family: Some("armature".to_string()),
            client: Some("zed".to_string()),
            protocol: Some("armature".to_string()),
        },
    };
    prompt_memory_upsert(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({
            "block_id": format!("pm-status-{}", Uuid::new_v4()),
            "scope": "session",
            "block_type": "session_focus",
            "session_id": "sess-test",
            "title": "Status focus",
            "body": "Status body",
            "priority": 4
        }),
    )
    .await
    .expect("upsert status block");
    let config = Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let status = memory_status_value(
        &config,
        &stores,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        &pool,
    )
    .await
    .expect("memory status value");
    assert_eq!(
        status["prompt_memory_diagnostic"]["source"],
        "prompt_memory_blocks"
    );
    assert_eq!(
        status["prompt_memory_diagnostic"]["active_by_scope"]["session"],
        1
    );
}
