use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use den_core::BearProfile;
use den_docket::{
    work_runs::{
        checkout_work_run_for_session, claim_next_work_run, enqueue_work_job,
        record_work_run_provisioned, WorkExecutionTarget, WorkJobEnqueue, WorkRunProvisioned,
    },
    DocketCommitPolicy, DocketCriterionKind, DocketEffortHint, DocketExecutionAttemptAuthorize,
    DocketExecutionAttemptRelease, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionAcquire, DocketFocusedExecutionBinding,
    DocketJobCreate, DocketJobCriterionInput, DocketJobOverlapResolution, DocketService,
    DocketTaskCreate, DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskPlacement,
    DocketTaskScope, PgDocketService, RoutingStrategy, TaskListVisibility,
};
use den_http::armature_tokens;
use den_protocol::{
    ContextBudgetComponentReport, ContextBudgetEstimatePrecision, ContextBudgetReport,
    RoleRuntimeBinding, RuntimeConversationBackend, RuntimeConversationRef, RuntimeHistoryRecord,
    RuntimeSemanticEvent, RuntimeStreamEvent,
};
#[cfg(feature = "test-fixtures")]
use den_runtime::native_runtime::scripted_runtime_invocation_count;
use den_runtime::native_runtime::{set_next_scripted_runtime_streams, ScriptedRuntimeStream};
use den_runtime::{
    bearwire_events,
    native_runtime::NativeRuntimeConversationBackend,
    turn_ids::{ClientSessionId, ToolCallId, TurnRunId},
    turn_obligations, turn_runs,
};
use den_service::{
    artifacts::{self, ArtifactAccessContext, DocketArtifactTargetKind},
    bears::{db as bears_db, db::BearParams},
    client_sessions,
    conversation::events::{
        canonical_persistence_context, persist_canonical_conversation_record,
        CanonicalConversationRecord, CanonicalToolResultRecord, ConversationEventProvenance,
    },
    conversation::persistence::{
        append_message, ensure_conversation_for_external_id, list_projected_messages_page,
        update_latest_context_budget, ConversationHistoryProjection,
    },
    conversation_message_types::{
        ConversationMessageRole, ConversationMessageType, ConversationMessageVisibility,
        ConversationMessageWrite,
    },
    DenState,
};

use crate::{
    events::{events_page, EventPageQuery},
    methods::run::{normalized_workspace_roots, persist_run_failed, RunFailureReason},
    rpc::rpc,
};
use bearwire_protocol::{rpc::JsonRpcRequest, surface::SurfaceHistoryEvent, wire::BearWireEvent};

#[sqlx::test(migrations = "../../migrations")]
async fn ide_default_and_first_interaction_hat_selection_bind_one_canonical_conversation(
    pool: sqlx::PgPool,
) {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats::{self, bindings, memory_binding};
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let owner = BearId::new(bear_id);
    let default = hats::create_hat(
        &pool,
        owner,
        UserId::new(user_id),
        "General IDE",
        "Pair in the editor",
    )
    .await
    .unwrap();
    let review = hats::create_hat(
        &pool,
        owner,
        UserId::new(user_id),
        "Security review",
        "Review carefully",
    )
    .await
    .unwrap();
    hats::set_ide_default_hat(&pool, owner, default.id)
        .await
        .unwrap();
    let state = test_state(pool.clone());
    let session_id = format!("ide-{}", Uuid::new_v4());
    let opened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "client": "zed",
        }),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    let resolved = opened["result"]["session"]["resolved_conversation_id"]
        .as_str()
        .unwrap();
    assert!(resolved.starts_with("den-conv-"));
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, bear_id, resolved,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        bindings::conversation_hat(&pool, owner, canonical.id)
            .await
            .unwrap(),
        Some(default.id)
    );
    let listed = rpc_value(
        state.clone(),
        &token,
        "hats.list",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(
        listed["result"]["ide_default_hat_id"],
        default.id.to_string()
    );
    assert_eq!(listed["result"]["selected_hat_id"], default.id.to_string());
    let other_user = create_test_user(&pool).await;
    let other_token = create_member_token(&pool, other_user, bear_id).await;
    let stolen = rpc_value(
        state.clone(),
        &other_token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "hat_id": review.id,
        }),
    )
    .await;
    assert!(stolen.get("error").is_some(), "{stolen}");
    assert_eq!(
        bindings::conversation_hat(&pool, owner, canonical.id)
            .await
            .unwrap(),
        Some(default.id)
    );
    let changed = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "hat_id": review.id,
        }),
    )
    .await;
    assert_eq!(changed["result"]["ok"], true, "{changed}");
    assert_eq!(
        bindings::conversation_hat(&pool, owner, canonical.id)
            .await
            .unwrap(),
        Some(review.id)
    );
    assert!(
        matches!(memory_binding::for_conversation(&pool, owner, canonical.id).await.unwrap(),
        memory_binding::ResolvedMemoryBinding::Bound(grant) if grant.hat_id() == Some(review.id))
    );
    let reopened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "client": "zed",
        }),
    )
    .await;
    assert_eq!(reopened["result"]["ok"], true, "{reopened}");
    assert_eq!(
        bindings::conversation_hat(&pool, owner, canonical.id)
            .await
            .unwrap(),
        Some(review.id)
    );
    append_message(
        &pool,
        canonical.id,
        &ConversationMessageWrite::user_turn("A first turn", json!({"text":"A first turn"}), None),
    )
    .await
    .unwrap();
    let denied = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "hat_id": default.id,
        }),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
    assert_eq!(
        bindings::conversation_hat(&pool, owner, canonical.id)
            .await
            .unwrap(),
        Some(review.id)
    );

    let other_session = format!("ide-{}", Uuid::new_v4());
    let new_open = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": other_session, "client": "zed",
        }),
    )
    .await;
    assert_eq!(new_open["result"]["ok"], true, "{new_open}");
    let (other_bear, other_slug) = create_test_bear(&pool).await;
    let foreign = hats::create_hat(
        &pool,
        BearId::new(other_bear),
        UserId::new(user_id),
        "Foreign",
        "Other Bear",
    )
    .await
    .unwrap();
    let wrong_bear = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": other_session, "hat_id": foreign.id,
        }),
    )
    .await;
    assert!(wrong_bear.get("error").is_some(), "{wrong_bear}");
    let other_token = create_token_for_bear(&pool, user_id, other_bear).await;
    let unbound_session = format!("ide-{}", Uuid::new_v4());
    let unbound = rpc_value(
        state.clone(),
        &other_token,
        "session.open",
        json!({
            "bear_slug": other_slug, "session_id": unbound_session, "client": "zed",
        }),
    )
    .await;
    assert_eq!(unbound["result"]["ok"], true, "{unbound}");
    let unbound_id = unbound["result"]["session"]["resolved_conversation_id"]
        .as_str()
        .unwrap();
    let unbound_record = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, other_bear, unbound_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(other_bear), unbound_record.id)
            .await
            .unwrap(),
        None
    );
    let chosen = rpc_value(
        state,
        &other_token,
        "session.hat.select",
        json!({
            "bear_slug": other_slug, "session_id": unbound_session, "hat_id": foreign.id,
        }),
    )
    .await;
    assert_eq!(chosen["result"]["ok"], true, "{chosen}");
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(other_bear), unbound_record.id)
            .await
            .unwrap(),
        Some(foreign.id)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn configured_hats_reject_unbound_ide_turn_before_persisting_a_run_or_message(
    pool: sqlx::PgPool,
) {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats::{self, memory_binding};
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let owner = BearId::new(bear_id);
    let hat = hats::create_hat(&pool, owner, UserId::new(user_id), "IDE review", "Review")
        .await
        .unwrap();
    let state = test_state(pool.clone());
    let session_id = format!("ide-{}", Uuid::new_v4().simple());
    let opened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({"bear_slug": bear_slug, "session_id": session_id, "client": "zed"}),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    let external = opened["result"]["session"]["resolved_conversation_id"]
        .as_str()
        .unwrap();
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, bear_id, external,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        memory_binding::for_conversation(&pool, owner, canonical.id).await,
        Err(den_core::DenError::Authorization(_))
    ));
    let denied = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({"bear_slug": bear_slug, "session_id": session_id, "prompt": "Do not save this", "client": "zed"}),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
    let messages = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM conversation_messages WHERE conversation_id = $1",
        canonical.id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(messages, 0);
    let runs = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM turn_runs WHERE session_id = $1",
        session_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(runs, 0);
    let chosen = rpc_value(
        state,
        &token,
        "session.hat.select",
        json!({"bear_slug": bear_slug, "session_id": session_id, "hat_id": hat.id}),
    )
    .await;
    assert_eq!(chosen["result"]["ok"], true, "{chosen}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn ide_hat_selection_survives_the_first_run_and_is_fixed_afterwards(pool: sqlx::PgPool) {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats::{self, bindings};
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let bear = BearId::new(bear_id);
    let general = hats::create_hat(
        &pool,
        bear,
        UserId::new(user_id),
        "IDE general",
        "General work",
    )
    .await
    .unwrap();
    let security = hats::create_hat(
        &pool,
        bear,
        UserId::new(user_id),
        "IDE security",
        "Review work",
    )
    .await
    .unwrap();
    hats::set_ide_default_hat(&pool, bear, general.id)
        .await
        .unwrap();
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("ide-{}", Uuid::new_v4().simple());
    let opened = rpc_value(
        state.clone(),
        &token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "client": "zed",
        }),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    let durable = opened["result"]["session"]["resolved_conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "hat_id": security.id,
        }),
    )
    .await;
    assert_eq!(selected["result"]["ok"], true, "{selected}");
    let prompt = "Inspect this repository";
    let started = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "prompt": prompt, "client": "zed",
        }),
    )
    .await;
    assert_eq!(started["result"]["ok"], true, "{started}");
    let resolved = wait_for_resolved_conversation_id(&pool, user_id, &bear_slug, &session_id).await;
    assert_eq!(
        resolved, durable,
        "run must not materialize a second unbound conversation"
    );
    wait_for_user_message(&pool, bear_id, &resolved, prompt).await;
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, bear_id, &durable,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        bindings::conversation_hat(&pool, bear, canonical.id)
            .await
            .unwrap(),
        Some(security.id)
    );
    let denied = rpc_value(
        state,
        &token,
        "session.hat.select",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "hat_id": general.id,
        }),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
}

#[test]
fn normalized_workspace_roots_uses_cwd_when_roots_are_not_declared() {
    assert_eq!(
        normalized_workspace_roots(None, Some("/workspace"))
            .expect("cwd fallback should be accepted"),
        vec!["/workspace"]
    );
}

#[test]
fn normalized_workspace_roots_accepts_cwd_inside_a_declared_root() {
    assert_eq!(
        normalized_workspace_roots(
            Some(&json!({ "workspace_roots": ["/workspace"] })),
            Some("/workspace/services/den"),
        )
        .expect("containing root should be accepted"),
        vec!["/workspace"]
    );
}

#[test]
fn normalized_workspace_roots_rejects_cwd_outside_declared_roots() {
    let error = normalized_workspace_roots(
        Some(&json!({ "workspace_roots": ["/workspace"] })),
        Some("/other-workspace"),
    )
    .expect_err("outside cwd must not become a workspace root");

    assert!(format!("{error:?}").contains("outside declared workspace_roots"));
}
fn test_state(pool: sqlx::PgPool) -> DenState {
    test_state_with_config(pool, den_core::config::Config::test_stub())
}

fn test_state_with_config(pool: sqlx::PgPool, config: den_core::config::Config) -> DenState {
    let config = std::sync::Arc::new(config);
    let state = DenState::new(
        pool,
        config.clone(),
        std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
        den_memory::MemoryStoreManager::new(config.as_ref()),
    );
    let snapshot = den_service::bifrost::BifrostCatalogSnapshot::from_available_models(vec![
        den_service::bifrost::BifrostModelMetadata {
            handle: den_llm::normalize_llm_model_handle(&config.default_llm_model),
            provider: "openai".to_string(),
            model: config.default_llm_model.trim().to_string(),
            display_name: Some("BearWire test model".to_string()),
            context_window: 128_000,
            max_output_tokens: Some(4096),
            enabled: true,
            supports_tools: Some(true),
            supports_responses_api: Some(false),
            supports_vision: Some(false),
            supports_reasoning_effort: None,
        },
    ]);
    *state.bifrost_catalog.write().expect("catalog lock") = snapshot;
    state
}

async fn create_test_user(pool: &sqlx::PgPool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("bw{}", &suffix[..16]);
    let email = format!("{username}@example.test");
    let (user_id,): (i32,) = sqlx::query_as(
        r"
        INSERT INTO users (email, username, display_name, passhash)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        ",
    )
    .bind(email)
    .bind(&username)
    .bind(format!("BearWire Test {username}"))
    .bind("unused-in-bearwire-tests")
    .fetch_one(pool)
    .await
    .expect("insert test user");
    user_id
}

async fn create_test_bear(pool: &sqlx::PgPool) -> (uuid::Uuid, String) {
    let suffix = Uuid::new_v4().simple().to_string();
    let slug = format!("bearwire-test-{}", &suffix[..12]);
    let bear_id = bears_db::create_bear(
        pool,
        BearParams {
            slug: &slug,
            name: "BearWire Test Bear",
            description: "BearWire integration test bear",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    bears_db::ensure_bear_profile_binding_rows(pool, bear_id)
        .await
        .expect("ensure Bear profile bindings");
    (bear_id, slug)
}

async fn seed_test_bifrost_virtual_key(
    pool: &sqlx::PgPool,
    bear_id: uuid::Uuid,
    config: &den_core::config::Config,
) {
    bears_db::set_bear_bifrost_virtual_key(
        pool,
        bear_id,
        Some("vk-test"),
        Some("BearWire test virtual key"),
        Some("sk-bf-bearwire-test"),
        &config.den_secret_encryption_key,
    )
    .await
    .expect("seed test Bifrost virtual key");
}

async fn create_token_for_bear(pool: &sqlx::PgPool, user_id: i32, bear_id: uuid::Uuid) -> String {
    bears_db::grant_membership(pool, user_id, bear_id, Some(bears_db::BEAR_ROLE_ADMIN))
        .await
        .expect("grant membership");
    armature_tokens::create_for_bear(pool, user_id, bear_id, "BearWire test token")
        .await
        .expect("create token")
        .raw_token
}

async fn create_member_token(pool: &sqlx::PgPool, user_id: i32, bear_id: Uuid) -> String {
    bears_db::grant_membership(pool, user_id, bear_id, Some(bears_db::BEAR_ROLE_MEMBER))
        .await
        .expect("grant member role");
    armature_tokens::create_for_bear(pool, user_id, bear_id, "BearWire member token")
        .await
        .expect("create member token")
        .raw_token
}

fn bearer_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("header value"),
    );
    headers
}

async fn upsert_test_session(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_id: uuid::Uuid,
    bear_slug: &str,
    session_id: &str,
) {
    client_sessions::upsert_session(
        pool,
        client_sessions::UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: bear_slug.to_string(),
            client_session_id: session_id.to_string(),
            runtime_session_id: format!("bearwire-test:{bear_id}:{session_id}"),
            conversation_id: format!("den-conv-{}", Uuid::new_v4().simple()),
            resolved_conversation_id: None,
            client: "bearwire-test".to_string(),
            cwd: Some("/workspace".to_string()),
            current_mode: Some(client_sessions::ClientSessionMode::Write),
        },
    )
    .await
    .expect("upsert BearWire test session");
}

async fn wait_for_resolved_conversation_id(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_slug: &str,
    session_id: &str,
) -> String {
    for _ in 0..50 {
        let session =
            client_sessions::find_for_user_bear_session(pool, user_id, bear_slug, session_id)
                .await
                .expect("load session")
                .expect("session exists");
        if let Some(resolved) = session.resolved_conversation_id {
            return resolved;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run.start did not resolve conversation within one second");
}

async fn wait_for_user_message(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    prompt: &str,
) {
    for _ in 0..50 {
        let exists: bool = sqlx::query_scalar(
            r"
            SELECT EXISTS(
                SELECT 1
                FROM conversation_messages
                WHERE conversation_id = (
                    SELECT id FROM conversations
                    WHERE bear_id = $1 AND external_conversation_id = $2
                    LIMIT 1
                )
                AND message_type = 'user'
                AND role = 'user'
                AND content_text LIKE $3
            )
            ",
        )
        .bind(bear_id)
        .bind(conversation_id)
        .bind(format!("{prompt}%"))
        .fetch_one(pool)
        .await
        .expect("check persisted user message");
        if exists {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run.start did not persist the user message within one second");
}

async fn create_session_task(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_id: uuid::Uuid,
    client_session_id: &str,
    title: &str,
) -> uuid::Uuid {
    let (session_anchor_id,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(client_session_id)
    .fetch_one(pool)
    .await
    .expect("load test session anchor");
    PgDocketService::from_pool(pool)
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: None,
            session_anchor_id: Some(session_anchor_id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: title.to_string(),
            body: "BearWire current-task test".to_string(),
            completion_criteria: vec!["Selection is persisted".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .expect("create session task")
        .id
}

async fn create_checkoutable_work_run(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_id: uuid::Uuid,
) -> uuid::Uuid {
    create_checkoutable_work_run_for_target(pool, user_id, bear_id, WorkExecutionTarget::Sandbox)
        .await
}

async fn create_checkoutable_work_run_for_target(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_id: uuid::Uuid,
    execution_target: WorkExecutionTarget,
) -> uuid::Uuid {
    let surface_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now())",
    )
    .bind(surface_id)
    .bind(format!("bearwire-work-{}", Uuid::new_v4().simple()))
    .bind(user_id)
    .execute(pool)
    .await
    .expect("create work surface");
    sqlx::query("INSERT INTO git_work_surface_details (id, upstream_url) VALUES ($1, $2)")
        .bind(surface_id)
        .bind("https://example.test/bearwire-work.git")
        .execute(pool)
        .await
        .expect("create git surface details");
    sqlx::query("INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)")
        .bind(surface_id)
        .bind(bear_id)
        .execute(pool)
        .await
        .expect("assign bear to work surface");

    let job = PgDocketService::from_pool(pool)
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "pair".to_string(),
            goal: "Checkout must not replace the Pair task".to_string(),
            work_surface_id: Some(surface_id),
            work_surface_assignments: vec![],
            commit_policy: Some(DocketCommitPolicy::PerTask),
            work_branch: None,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![DocketJobCriterionInput {
                kind: DocketCriterionKind::Narrative,
                description: "Work checkout succeeds".to_string(),
                spec: None,
                sibling_order: 0,
            }],
            tasks: vec![DocketTaskInput {
                client_key: None,
                parent_client_key: None,
                parent_task_id: None,
                sibling_order: Some(0),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "Work task".to_string(),
                body: "Work task body".to_string(),
                completion_criteria: vec!["Work completes".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            }],
        })
        .await
        .expect("create work job");
    let attached_target = matches!(
        execution_target,
        WorkExecutionTarget::AttachedArmature { .. }
    );
    let runs = enqueue_work_job(
        pool,
        WorkJobEnqueue {
            bear_id,
            job_id: job.job.id,
            durable_result: den_docket::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user_id),
            execution_target,
            attachment_warning: None,
        },
    )
    .await
    .expect("enqueue work job");
    assert_eq!(runs.len(), 1, "one work run for the test job");
    let run = runs.into_iter().next().expect("work run exists");
    if attached_target {
        return run.id;
    }
    let claimed = claim_next_work_run(
        pool,
        "bearwire-test-runner",
        std::time::Duration::from_mins(1),
    )
    .await
    .expect("claim work run")
    .expect("queued work run claimed");
    assert_eq!(claimed.id, run.id);
    record_work_run_provisioned(
        pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox.test".to_string(),
            sandbox_id: "sandbox-test".to_string(),
            sandbox_type: "container".to_string(),
            sandbox_strength: "container: test".to_string(),
            work_surface: json!({ "is_git": true }),
            rust_dependency_preparation: None,
        },
    )
    .await
    .expect("provision work run");
    run.id
}

async fn rpc_value(state: DenState, token: &str, method: &str, params: Value) -> Value {
    let response = rpc(
        State(state),
        bearer_headers(token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!(format!("req-{}", Uuid::new_v4().simple()))),
            method: method.to_string(),
            params,
        }),
    )
    .await
    .expect("rpc response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[cfg(feature = "test-fixtures")]
#[sqlx::test(migrations = "../../migrations")]
async fn focused_session_loop_continues_across_two_bounded_slices(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-encryption-key".to_string();
    // Docket validates the configured model before it asks native runtime for
    // its stream. The scripted stream replaces provider I/O after this local
    // preflight, but the mock keeps the validation path realistic.
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("bounded-slice-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;

    let surface_id = Uuid::new_v4();
    sqlx::query("INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at) VALUES ($1, $2, 'git_workspace', $3, now(), now())")
        .bind(surface_id).bind(format!("bounded-slice-{}", Uuid::new_v4().simple())).bind(user_id)
        .execute(&pool).await.expect("create work surface");
    sqlx::query("INSERT INTO git_work_surface_details (id, upstream_url) VALUES ($1, $2)")
        .bind(surface_id)
        .bind("https://example.test/bounded-slice.git")
        .execute(&pool)
        .await
        .expect("create git surface details");
    sqlx::query("INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)")
        .bind(surface_id)
        .bind(bear_id)
        .execute(&pool)
        .await
        .expect("assign bear to surface");
    let job = PgDocketService::from_pool(&pool)
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "pair".to_string(),
            goal: "Prove bounded Pair continuation".to_string(),
            work_surface_id: Some(surface_id),
            work_surface_assignments: vec![],
            commit_policy: None,
            work_branch: None,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![],
            tasks: vec![DocketTaskInput {
                client_key: None,
                parent_client_key: None,
                parent_task_id: None,
                sibling_order: Some(0),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "Continue across slices".to_string(),
                body: "Stay focused.".to_string(),
                completion_criteria: vec!["Task is settled".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            }],
        })
        .await
        .expect("create job");
    let task_id: Uuid = sqlx::query_scalar("SELECT id FROM bear_tasks WHERE job_id = $1")
        .bind(job.job.id)
        .fetch_one(&pool)
        .await
        .expect("load task");
    let session_anchor: Uuid = sqlx::query_scalar("SELECT id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3")
        .bind(user_id).bind(bear_id).bind(&session_id).fetch_one(&pool).await.expect("load session");
    sqlx::query("INSERT INTO bear_session_task_attachments (task_id, session_id) VALUES ($1, $2)")
        .bind(task_id)
        .bind(session_anchor)
        .execute(&pool)
        .await
        .expect("attach task");
    let params = json!({"bear_slug": bear_slug, "session_id": session_id, "task_id": task_id});
    let selected = rpc_value(state.clone(), &token, "session.current_task.select", params).await;
    assert!(selected.get("error").is_none(), "{selected}");

    set_next_scripted_runtime_streams(
        &session_id,
        vec![
            ScriptedRuntimeStream::Events(vec![RuntimeStreamEvent::Semantic(
                RuntimeSemanticEvent::BoundedSlice {
                    reason: "first technical budget boundary".to_string(),
                },
            )]),
            ScriptedRuntimeStream::Events(vec![RuntimeStreamEvent::Semantic(
                RuntimeSemanticEvent::BoundedSlice {
                    reason: "second technical budget boundary".to_string(),
                },
            )]),
            ScriptedRuntimeStream::Pending,
        ],
    );
    let focused = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.execute",
        json!({"bear_slug": bear_slug, "job_id": job.job.id, "session_id": session_id}),
    )
    .await;
    assert!(focused.get("error").is_none(), "{focused}");
    let run_id = focused["result"]["pair_binding"]["run"]["id"]
        .as_str()
        .expect("focused run id");

    // Each BoundedSlice schedules a new native continuation. The successor
    // immediately claims the same turn run, so `continuing` is transient.
    // The feature-gated counter records native stream construction, proving
    // each boundary re-entered the real runtime without timing live telemetry.
    for expected_slices in 1..=2_usize {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let invocations = scripted_runtime_invocation_count(run_id);
            if invocations >= expected_slices + 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "bounded slice {expected_slices} did not start its continuation; observed {invocations} runtime invocations"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let attempt = sqlx::query!(
            "SELECT task_id, host_run_id, fence_epoch FROM docket_execution_attempts WHERE host_run_id = $1 AND state = 'running'",
            run_id,
        )
        .fetch_one(&pool)
        .await
        .expect("load canonical live attempt");
        assert_eq!(
            attempt.task_id, task_id,
            "slice {expected_slices} changed task"
        );
        assert_eq!(attempt.host_run_id, run_id);
        assert_eq!(
            attempt.fence_epoch, 1,
            "slice {expected_slices} changed fence"
        );
    }
    let terminal: i64 = sqlx::query_scalar("SELECT count(*) FROM bearwire_events WHERE session_id = $1 AND event_json->>'run_id' = $2 AND event_type IN ('run.completed', 'run.failed', 'run.cancelled')")
        .bind(&session_id).bind(run_id).fetch_one(&pool).await.expect("load terminal events");
    assert_eq!(
        terminal, 0,
        "bounded continuation must not terminalize before settlement"
    );

    let settled = rpc_value(
        state,
        &token,
        "docket.jobs.settle_task",
        json!({
            "bear_slug": bear_slug,
            "job_id": job.job.id,
            "task_id": task_id,
            "status": "done",
            "outcome_disposition": "completed",
            "result_summary": "two bounded slices verified",
            "session_id": session_id,
        }),
    )
    .await;
    assert!(settled.get("error").is_none(), "{settled}");
    assert_eq!(
        settled["result"]["outcome"]["control"]["next_action"].as_str(),
        Some("job_completed"),
        "settling the only focused task must return control to ordinary chat: {settled}"
    );
    let running_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE host_run_id = $1 AND state = 'running'",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("load live attempts after settlement");
    assert_eq!(
        running_attempts, 0,
        "settlement must release Docket authority"
    );
    assert_eq!(
        scripted_runtime_invocation_count(run_id),
        3,
        "settlement must not schedule another continuation"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn docket_execute_starts_focused_session_loop_for_selected_task(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-encryption-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    set_next_scripted_runtime_streams(
        &session_id,
        vec![
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
        ],
    );
    let surface_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)\n         VALUES ($1, $2, 'git_workspace', $3, now(), now())",
    )
    .bind(surface_id)
    .bind(format!("bearwire-binding-{}", Uuid::new_v4().simple()))
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("create work surface");
    sqlx::query("INSERT INTO git_work_surface_details (id, upstream_url) VALUES ($1, $2)")
        .bind(surface_id)
        .bind("https://example.test/bearwire-binding.git")
        .execute(&pool)
        .await
        .expect("create git surface details");
    sqlx::query("INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)")
        .bind(surface_id)
        .bind(bear_id)
        .execute(&pool)
        .await
        .expect("assign bear to work surface");

    let job = PgDocketService::from_pool(&pool)
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "pair".to_string(),
            goal: "Pair binding diagnostics regression".to_string(),
            work_surface_id: Some(surface_id),
            work_surface_assignments: vec![],
            commit_policy: None,
            work_branch: None,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![],
            tasks: vec![
                DocketTaskInput {
                    client_key: None,
                    parent_client_key: None,
                    parent_task_id: None,
                    sibling_order: Some(0),
                    kind: DocketTaskKind::Execution,
                    scope: DocketTaskScope::Template,
                    title: "Verify binding".to_string(),
                    body: "Verify Pair binding response".to_string(),
                    completion_criteria: vec!["Binding is reported".to_string()],
                    difficulty: Some(DocketTaskDifficulty::Trivial),
                    effort_hint: Some(DocketEffortHint::Low),
                    routing_strategy: RoutingStrategy::Auto,
                    expected_context_size: None,
                    result_rollup_policy: None,
                },
                DocketTaskInput {
                    client_key: None,
                    parent_client_key: None,
                    parent_task_id: None,
                    sibling_order: Some(1),
                    kind: DocketTaskKind::Execution,
                    scope: DocketTaskScope::Template,
                    title: "Continue after settlement".to_string(),
                    body: "Verify successor task control".to_string(),
                    completion_criteria: vec!["Successor is selected".to_string()],
                    difficulty: Some(DocketTaskDifficulty::Trivial),
                    effort_hint: Some(DocketEffortHint::Low),
                    routing_strategy: RoutingStrategy::Auto,
                    expected_context_size: None,
                    result_rollup_policy: None,
                },
            ],
        })
        .await
        .expect("create Docket job");

    let assigned_task_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM bear_tasks WHERE job_id = $1 ORDER BY sibling_order, id LIMIT 1",
    )
    .bind(job.job.id)
    .fetch_one(&pool)
    .await
    .expect("load task to assign before focus");
    let session_anchor_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load Pair session anchor");
    sqlx::query("INSERT INTO bear_session_task_attachments (task_id, session_id) VALUES ($1, $2)")
        .bind(assigned_task_id)
        .bind(session_anchor_id)
        .execute(&pool)
        .await
        .expect("attach job task to Pair session");
    let selection_params = json!({
        "bear_slug": bear_slug,
        "session_id": session_id,
        "task_id": assigned_task_id,
    });
    let preview = rpc_value(
        state.clone(),
        &token,
        "session.current_task.selection_request",
        selection_params.clone(),
    )
    .await;
    assert_eq!(
        preview["result"]["confirmation_required"], true,
        "{preview}"
    );
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        selection_params,
    )
    .await;
    assert!(selected.get("error").is_none(), "{selected}");

    // Assignment must not take control of the Pair session. Focus is the
    // explicit transition from chat to Docket control.
    let before_focus =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .expect("load client session before focus")
            .expect("client session exists");
    assert_eq!(before_focus.current_task_id, Some(assigned_task_id));
    let attempts_before_focus: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE binding_kind = 'client_session' AND binding_id = $1 AND state = 'running'",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load pre-focus execution authority");
    assert_eq!(attempts_before_focus, 0);

    let attached = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.execute",
        json!({ "bear_slug": bear_slug, "job_id": job.job.id, "session_id": session_id }),
    )
    .await;
    let task_id = attached["result"]["pair_binding"]["task"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("focus execution failed: {attached}"));
    assert_eq!(task_id, assigned_task_id.to_string());
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["kind"],
        "docket"
    );
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["state"],
        "accepted"
    );
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["launch_state"],
        "claimed"
    );
    assert_eq!(
        attached["result"]["session_execution"], attached["result"]["pair_binding"],
        "canonical session execution and legacy compatibility projection must agree"
    );
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["attempt_state"],
        "authorized"
    );
    assert_eq!(attached["result"]["pair_binding"]["task"]["selected"], true);
    let loop_run_id = attached["result"]["pair_binding"]["run"]["id"]
        .as_str()
        .expect("focused run id");
    wait_for_focused_run_started(state.clone(), &token, &bear_slug, &session_id, loop_run_id).await;
    let transition_events =
        bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 50)
            .await
            .expect("list focused execution transitions")
            .into_iter()
            .filter(|event| {
                event.event_type
                    == bearwire_protocol::lifecycle::FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE
            })
            .collect::<Vec<_>>();
    let transitions = transition_events
        .iter()
        .map(|event| {
            serde_json::from_value::<bearwire_protocol::lifecycle::FocusedExecutionTransition>(
                event.event.data.clone(),
            )
            .expect("decode focused execution transition")
        })
        .collect::<Vec<_>>();
    assert_eq!(transitions.len(), 2);
    assert_eq!(transitions[0].state_version, 1);
    assert_eq!(transitions[0].from, None);
    assert_eq!(
        transitions[0].reason,
        bearwire_protocol::lifecycle::FocusedExecutionTransitionReason::AuthorityClaimed
    );
    assert_eq!(
        transitions[0].to,
        bearwire_protocol::lifecycle::FocusedExecutionState::Starting
    );
    assert_eq!(transitions[1].state_version, 2);
    assert_eq!(
        transitions[1].from,
        Some(bearwire_protocol::lifecycle::FocusedExecutionState::Starting)
    );
    assert_eq!(
        transitions[1].reason,
        bearwire_protocol::lifecycle::FocusedExecutionTransitionReason::AuthorityStarted
    );
    assert_eq!(
        transitions[1].to,
        bearwire_protocol::lifecycle::FocusedExecutionState::Running
    );
    assert!(transition_events.iter().all(|event| {
        event.event.scope == bearwire_protocol::wire::BearWireEventScope::Persistent
    }));
    let replay = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.execute",
        json!({ "bear_slug": bear_slug, "job_id": job.job.id, "session_id": session_id }),
    )
    .await;
    assert_eq!(
        replay["result"]["pair_binding"]["task"]["id"],
        attached["result"]["pair_binding"]["task"]["id"],
        "repeating /focus must retain the selected task: {replay}"
    );
    assert_eq!(
        replay["result"]["pair_binding"]["run"]["id"],
        attached["result"]["pair_binding"]["run"]["id"],
        "repeating /focus must reconcile the existing run: {replay}"
    );
    assert_eq!(
        replay["result"]["pair_binding"]["control"]["launch_state"], "already_running",
        "repeating /focus must return its reconciled state: {replay}"
    );

    // The task is deliberately not settled yet. Focus must leave the exact
    // Pair host run and its canonical Docket attempt live; this catches the
    // historical failure where focus returned successfully but its loop ended
    // before the caller could make a task decision.
    let focused_run_state: String =
        sqlx::query_scalar("SELECT state FROM turn_runs WHERE run_id = $1 LIMIT 1")
            .bind(loop_run_id)
            .fetch_one(&pool)
            .await
            .expect("load focused Pair run state");
    assert!(
        matches!(focused_run_state.as_str(), "running" | "continuing"),
        "focused Pair loop ended before explicit settlement; state={focused_run_state}"
    );
    let terminal_events_before_settlement: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bearwire_events \
         WHERE session_id = $1 \
           AND event_json->>'run_id' = $2 \
           AND event_type IN ('run.completed', 'run.failed', 'run.cancelled')",
    )
    .bind(&session_id)
    .bind(loop_run_id)
    .fetch_one(&pool)
    .await
    .expect("load focused Pair terminal events");
    assert_eq!(
        terminal_events_before_settlement, 0,
        "focused Pair loop emitted a terminal event before explicit settlement"
    );
    let live_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE host_run_id = $1 AND state = 'running'",
    )
    .bind(loop_run_id)
    .fetch_one(&pool)
    .await
    .expect("load live Pair execution authority");
    assert_eq!(live_attempts, 1);
    let attached_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bear_session_task_attachments WHERE task_id = $1 AND released_at IS NULL",
    )
    .bind(Uuid::parse_str(task_id).expect("parse attached task id"))
    .fetch_one(&pool)
    .await
    .expect("load Pair attachment");
    assert_eq!(attached_count, 1);

    let docket_run_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM bear_job_runs WHERE job_id = $1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(job.job.id)
    .fetch_one(&pool)
    .await
    .expect("load focused Docket run");

    // Docket-controlled work may grow its own task tree.
    let spawned = PgDocketService::from_pool(&pool)
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: Some(job.job.id),
            session_anchor_id: None,
            parent_task_id: Some(Uuid::parse_str(task_id).expect("parse root task id")),
            sibling_order: 0,
            placement: Some(DocketTaskPlacement::Last),
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: "Follow up during focused control".to_string(),
            body: "Created while Docket owns the Pair loop".to_string(),
            completion_criteria: vec!["Follow-up is settled".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: Some(docket_run_id),
        })
        .await
        .expect("add subtask while focused");
    assert_eq!(
        spawned.parent_task_id,
        Some(Uuid::parse_str(task_id).unwrap())
    );

    // Children settle before their parent. This also proves a task added to
    // the live Docket run can be settled with only the required parameters.
    let settled_child = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.settle_task",
        json!({
            "bear_slug": bear_slug,
            "job_id": job.job.id,
            "task_id": spawned.id,
            "status": "done",
            "session_id": session_id,
        }),
    )
    .await;
    assert!(settled_child.get("error").is_none(), "{settled_child}");

    let settled = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.settle_task",
        json!({
            "bear_slug": bear_slug,
            "job_id": job.job.id,
            "task_id": task_id,
            "status": "done",
            "outcome_disposition": "completed",
            "result_summary": "First task completed",
            "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(
        settled["result"]["outcome"]["control"]["next_action"], "work_current_task",
        "{settled}"
    );
    let successor_id = settled["result"]["outcome"]["control"]["task"]["current_task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("settlement did not select a successor: {settled}"));
    assert_ne!(successor_id, task_id);
    let session =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .expect("load client session")
            .expect("client session exists");
    assert_eq!(
        session.current_task_id,
        Some(Uuid::parse_str(successor_id).expect("parse successor task id")),
        "settlement must advance session focus before final-answer gating"
    );
    assert_eq!(
        settled["result"]["pair_binding"]["control"]["state"], "accepted",
        "settlement must claim focused control before successor startup: {settled}"
    );
    assert_eq!(
        settled["result"]["pair_binding"]["control"]["launch_state"],
        "claimed"
    );
    assert_eq!(
        settled["result"]["pair_binding"]["task"]["id"], successor_id,
        "successor execution must be bound to the task selected by settlement"
    );
    let settled_run_id = settled["result"]["pair_binding"]["run"]["id"]
        .as_str()
        .expect("settlement successor run id");
    wait_for_focused_run_started(
        state.clone(),
        &token,
        &bear_slug,
        &session_id,
        settled_run_id,
    )
    .await;

    // A task-compatible user turn must remain in the focused Docket loop even
    // though it starts a successor Pair host run. The run-start path used to
    // drop the live execution attempt because only the explicit focus path
    // supplied an explicit focused task id.
    let continued = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bearwire-test",
            "prompt": "Also check the related handoff path."
        }),
    )
    .await;
    let successor_run_id = continued["result"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("run start did not return a run id: {continued}"));
    let continued_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts \
         WHERE host_run_id = $1 AND task_id = $2 AND state = 'running'",
    )
    .bind(successor_run_id)
    .bind(Uuid::parse_str(successor_id).expect("parse successor task id"))
    .fetch_one(&pool)
    .await
    .expect("load successor focused execution authority");
    assert_eq!(
        continued_attempts, 1,
        "a successor Pair run for a focused session must inherit Docket authority; {continued}"
    );

    // Optional settlement fields deliberately stay absent: the public default
    // must be sufficient to finish ordinary Docket work.
    let settled_successor = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.settle_task",
        json!({
            "bear_slug": bear_slug,
            "job_id": job.job.id,
            "task_id": successor_id,
            "status": "done",
            "session_id": session_id,
        }),
    )
    .await;
    assert!(
        settled_successor.get("error").is_none(),
        "{settled_successor}"
    );
    let terminal_run_state: String =
        sqlx::query_scalar("SELECT state FROM bear_job_runs WHERE id = $1")
            .bind(docket_run_id)
            .fetch_one(&pool)
            .await
            .expect("load terminal Docket run state");
    assert_eq!(
        terminal_run_state, "completed",
        "settling every task must complete the Docket run: {settled_successor}"
    );
    let terminal_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE binding_kind = 'client_session' AND binding_id = $1 AND state = 'running'",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load terminal Pair execution authority");
    assert_eq!(
        terminal_attempts, 0,
        "completion must release Docket control"
    );
    let terminal_transitions =
        bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 100)
            .await
            .expect("list terminal focused execution transitions")
            .into_iter()
            .filter(|event| {
                event.event_type
                    == bearwire_protocol::lifecycle::FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE
            })
            .map(|event| {
                serde_json::from_value::<bearwire_protocol::lifecycle::FocusedExecutionTransition>(
                    event.event.data,
                )
                .expect("decode terminal focused execution transition")
            })
            .collect::<Vec<_>>();
    assert!(
        terminal_transitions
            .iter()
            .enumerate()
            .all(|(index, transition)| transition.state_version == (index + 1) as u64),
        "focused execution transition versions must be contiguous"
    );
    assert_eq!(
        terminal_transitions.last().map(|transition| transition.to),
        Some(bearwire_protocol::lifecycle::FocusedExecutionState::Terminal),
        "settlement must leave a durable terminal diagnostic transition"
    );
    let settled_diagnostics = rpc_value(
        state.clone(),
        &token,
        "session.execution.diagnostics",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(
        settled_diagnostics["result"]["diagnostics"]["snapshot"]["state"]["phase"], "unfocused",
        "cleared task selection must not join historical execution authority"
    );

    let chat_run = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bearwire-test",
            "prompt": "Back to ordinary chat."
        }),
    )
    .await;
    assert!(
        chat_run.get("error").is_none(),
        "chat must resume: {chat_run}"
    );
    let chat_run_id = chat_run["result"]["run_id"].as_str().expect("chat run id");
    let chat_attempts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM docket_execution_attempts WHERE host_run_id = $1")
            .bind(chat_run_id)
            .fetch_one(&pool)
            .await
            .expect("load chat attempts");
    assert_eq!(
        chat_attempts, 0,
        "ordinary chat must not inherit Docket control"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn blocked_focused_task_ends_docket_control_and_returns_to_chat(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-encryption-key".to_string();
    // Focus performs an internal preparation request before the task-oriented
    // runtime turn. Keep the mock available for both requests so this test
    // verifies the latter rather than mistaking preparation EOF for a started loop.
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    set_next_scripted_runtime_streams(
        &session_id,
        vec![
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
            ScriptedRuntimeStream::Pending,
        ],
    );
    let surface_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)\n         VALUES ($1, $2, 'git_workspace', $3, now(), now())",
    )
    .bind(surface_id)
    .bind(format!("bearwire-binding-{}", Uuid::new_v4().simple()))
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("create work surface");
    sqlx::query("INSERT INTO git_work_surface_details (id, upstream_url) VALUES ($1, $2)")
        .bind(surface_id)
        .bind("https://example.test/bearwire-binding.git")
        .execute(&pool)
        .await
        .expect("create git surface details");
    sqlx::query("INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)")
        .bind(surface_id)
        .bind(bear_id)
        .execute(&pool)
        .await
        .expect("assign bear to work surface");

    let job = PgDocketService::from_pool(&pool)
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "pair".to_string(),
            goal: "Pair binding diagnostics regression".to_string(),
            work_surface_id: Some(surface_id),
            work_surface_assignments: vec![],
            commit_policy: None,
            work_branch: None,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![],
            tasks: vec![
                DocketTaskInput {
                    client_key: None,
                    parent_client_key: None,
                    parent_task_id: None,
                    sibling_order: Some(0),
                    kind: DocketTaskKind::Execution,
                    scope: DocketTaskScope::Template,
                    title: "Verify binding".to_string(),
                    body: "Verify Pair binding response".to_string(),
                    completion_criteria: vec!["Binding is reported".to_string()],
                    difficulty: Some(DocketTaskDifficulty::Trivial),
                    effort_hint: Some(DocketEffortHint::Low),
                    routing_strategy: RoutingStrategy::Auto,
                    expected_context_size: None,
                    result_rollup_policy: None,
                },
                DocketTaskInput {
                    client_key: None,
                    parent_client_key: None,
                    parent_task_id: None,
                    sibling_order: Some(1),
                    kind: DocketTaskKind::Execution,
                    scope: DocketTaskScope::Template,
                    title: "Continue after settlement".to_string(),
                    body: "Verify successor task control".to_string(),
                    completion_criteria: vec!["Successor is selected".to_string()],
                    difficulty: Some(DocketTaskDifficulty::Trivial),
                    effort_hint: Some(DocketEffortHint::Low),
                    routing_strategy: RoutingStrategy::Auto,
                    expected_context_size: None,
                    result_rollup_policy: None,
                },
            ],
        })
        .await
        .expect("create Docket job");

    let assigned_task_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM bear_tasks WHERE job_id = $1 ORDER BY sibling_order, id LIMIT 1",
    )
    .bind(job.job.id)
    .fetch_one(&pool)
    .await
    .expect("load task to assign before focus");
    let session_anchor_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load Pair session anchor");
    sqlx::query("INSERT INTO bear_session_task_attachments (task_id, session_id) VALUES ($1, $2)")
        .bind(assigned_task_id)
        .bind(session_anchor_id)
        .execute(&pool)
        .await
        .expect("attach job task to Pair session");
    let selection_params = json!({
        "bear_slug": bear_slug,
        "session_id": session_id,
        "task_id": assigned_task_id,
    });
    let preview = rpc_value(
        state.clone(),
        &token,
        "session.current_task.selection_request",
        selection_params.clone(),
    )
    .await;
    assert_eq!(
        preview["result"]["confirmation_required"], true,
        "{preview}"
    );
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        selection_params,
    )
    .await;
    assert!(selected.get("error").is_none(), "{selected}");

    // Assignment must not take control of the Pair session. Focus is the
    // explicit transition from chat to Docket control.
    let before_focus =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .expect("load client session before focus")
            .expect("client session exists");
    assert_eq!(before_focus.current_task_id, Some(assigned_task_id));
    let attempts_before_focus: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE binding_kind = 'client_session' AND binding_id = $1 AND state = 'running'",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load pre-focus execution authority");
    assert_eq!(attempts_before_focus, 0);

    let attached = rpc_value(
        state.clone(),
        &token,
        "docket.jobs.execute",
        json!({ "bear_slug": bear_slug, "job_id": job.job.id, "session_id": session_id }),
    )
    .await;
    let task_id = attached["result"]["pair_binding"]["task"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("focus execution failed: {attached}"));
    assert_eq!(task_id, assigned_task_id.to_string());
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["kind"],
        "docket"
    );
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["state"],
        "accepted"
    );
    assert_eq!(
        attached["result"]["pair_binding"]["control"]["launch_state"],
        "claimed"
    );
    assert_eq!(attached["result"]["pair_binding"]["task"]["selected"], true);
    assert!(attached["result"]["pair_binding"]["run"]["id"]
        .as_str()
        .is_some_and(|run_id| !run_id.is_empty()));
    let loop_run_id = attached["result"]["pair_binding"]["run"]["id"]
        .as_str()
        .expect("Pair loop run id");
    wait_for_focused_run_started(state.clone(), &token, &bear_slug, &session_id, loop_run_id).await;
    let live_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE host_run_id = $1 AND state = 'running'",
    )
    .bind(loop_run_id)
    .fetch_one(&pool)
    .await
    .expect("load live Pair execution authority");
    assert_eq!(live_attempts, 1);
    let attached_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bear_session_task_attachments WHERE task_id = $1 AND released_at IS NULL",
    )
    .bind(Uuid::parse_str(task_id).expect("parse attached task id"))
    .fetch_one(&pool)
    .await
    .expect("load Pair attachment");
    assert_eq!(attached_count, 1);

    // A blocked active task ends Docket control without pretending that the
    // job completed. Optional settlement fields remain absent here too.
    let blocked = rpc_value(
        test_state(pool.clone()),
        &token,
        "docket.jobs.settle_task",
        json!({
            "bear_slug": bear_slug,
            "job_id": job.job.id,
            "task_id": task_id,
            "status": "blocked",
            "session_id": session_id,
        }),
    )
    .await;
    assert!(blocked.get("error").is_none(), "{blocked}");

    let terminal_run_state: String = sqlx::query_scalar(
        "SELECT state FROM bear_job_runs WHERE job_id = $1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(job.job.id)
    .fetch_one(&pool)
    .await
    .expect("load blocked Docket run state");
    assert_eq!(terminal_run_state, "blocked", "{blocked}");
    let live_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_execution_attempts WHERE binding_kind = 'client_session' AND binding_id = $1 AND state = 'running'",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load terminal Pair execution authority");
    assert_eq!(live_attempts, 0, "blocking must release Docket control");

    // The blocked job remains selected/recoverable, but a normal chat turn is
    // no longer Docket-controlled.
    let session =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .expect("load client session")
            .expect("client session exists");
    assert_eq!(session.current_task_id, Some(assigned_task_id));
    let chat_run = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bearwire-test",
            "prompt": "Report the block to the user."
        }),
    )
    .await;
    assert!(
        chat_run.get("error").is_none(),
        "chat must resume: {chat_run}"
    );
    let chat_run_id = chat_run["result"]["run_id"].as_str().expect("chat run id");
    let chat_attempts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM docket_execution_attempts WHERE host_run_id = $1")
            .bind(chat_run_id)
            .fetch_one(&pool)
            .await
            .expect("load chat execution attempts");
    assert_eq!(
        chat_attempts, 0,
        "ordinary chat must not inherit Docket control"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_open_persists_event_and_events_replay(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let state = test_state(pool.clone());
    let session_id = format!("session-{}", Uuid::new_v4().simple());

    let response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-open")),
            method: "session.open".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "conversation_id": "conv-bearwire-test",
                "client": "bearwire-test"
            }),
        }),
    )
    .await
    .expect("session.open response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["ok"], true);
    let sequence = value["result"]["event_sequence"].as_i64().unwrap();

    let replay = events_page(
        State(state),
        bearer_headers(&token),
        Path(session_id.clone()),
        Query(EventPageQuery {
            bear_slug: value["result"]["session"]["bear_slug"]
                .as_str()
                .unwrap()
                .to_string(),
            after: None,
            limit: None,
        }),
    )
    .await
    .expect("events page response")
    .0;
    assert_eq!(replay["events"][0]["sequence"], sequence);
    assert_eq!(replay["events"][0]["event"]["type"], "session.opened");

    let replay_after = events_page(
        State(test_state(pool)),
        bearer_headers(&token),
        Path(session_id),
        Query(EventPageQuery {
            bear_slug: value["result"]["session"]["bear_slug"]
                .as_str()
                .unwrap()
                .to_string(),
            after: Some(sequence),
            limit: None,
        }),
    )
    .await
    .expect("events page response after cursor")
    .0;
    assert!(replay_after["events"].as_array().unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_open_preserves_sandbox_work_session_binding(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    let session_id = format!("work-{}", Uuid::new_v4().simple());
    let state = test_state(pool.clone());

    let checkout = rpc_value(
        state.clone(),
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert_eq!(checkout["result"]["ok"], true, "{checkout}");

    let opened = rpc_value(
        state,
        &token,
        "session.open",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bear-armature",
        }),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");

    let live = den_docket::work_runs::get_live_work_run_by_session(&pool, &session_id)
        .await
        .expect("look up live Work run")
        .expect("session remains bound to live Work run after session.open");
    assert_eq!(live.id, work_run_id);
}

fn start_mock_openai_sse_server() -> String {
    start_mock_openai_sse_server_asserting_body(Vec::new())
}

#[derive(Debug, Clone)]
struct MockLlmRequestAssertion {
    required_body_substrings: Vec<String>,
    exact_body_counts: Vec<(String, usize)>,
}

impl MockLlmRequestAssertion {
    fn requiring(required_body_substrings: Vec<String>) -> Self {
        Self {
            required_body_substrings,
            exact_body_counts: Vec::new(),
        }
    }
}

fn start_mock_openai_sse_server_asserting_body(required_body_substrings: Vec<String>) -> String {
    start_mock_openai_sse_server_asserting_requests(vec![MockLlmRequestAssertion::requiring(
        required_body_substrings,
    )])
}

fn start_mock_openai_sse_server_asserting_requests(
    request_assertions: Vec<MockLlmRequestAssertion>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock LLM server");
    let addr = listener.local_addr().expect("mock LLM local addr");
    thread::spawn(move || {
        for assertion in request_assertions {
            let (request, mut stream) = loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_http_request(&mut stream);
                if request.starts_with("GET /models") {
                    let body = r#"{"data":[{"id":"gpt-4.1","name":"GPT-4.1","owned_by":"openai","context_length":1047576,"max_output_tokens":32768,"supported_parameters":["tools"],"supported_methods":["chat_completion"]},{"id":"openai/bearwire-test-model","name":"BearWire test model","owned_by":"openai","context_length":128000,"max_output_tokens":4096,"supported_parameters":["tools"],"supported_methods":["chat_completion"]}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream
                        .write_all(response.as_bytes())
                        .expect("write mock models response");
                    continue;
                }
                break (request, stream);
            };
            assert!(
                request.starts_with("POST /chat/completions "),
                "unexpected LLM request: {request}"
            );
            for needle in &assertion.required_body_substrings {
                assert!(
                    request.contains(needle),
                    "LLM request body missing expected substring {needle:?}: {request}"
                );
            }
            for (needle, expected_count) in &assertion.exact_body_counts {
                let actual_count = request.matches(needle).count();
                assert_eq!(
                    actual_count, *expected_count,
                    "LLM request body had unexpected count for {needle:?}: {request}"
                );
            }
            let body = concat!(
                "data: {\"id\":\"chatcmpl-bearwire-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello from bearwire\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"chatcmpl-bearwire-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write mock LLM response");
        }
    });
    format!("http://{addr}")
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 1024];
    let mut header_end = None;
    while header_end.is_none() {
        let read = stream.read(&mut temp).expect("read mock LLM request");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        header_end = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    }

    let Some(header_end) = header_end else {
        return String::from_utf8_lossy(&buffer).into_owned();
    };
    let header_text = String::from_utf8_lossy(&buffer[..header_end + 4]);
    let content_length = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let already_read_body = buffer.len().saturating_sub(header_end + 4);
    let remaining = content_length.saturating_sub(already_read_body);
    if remaining > 0 {
        let mut body = vec![0_u8; remaining];
        stream
            .read_exact(&mut body)
            .expect("read mock LLM request body");
        buffer.extend_from_slice(&body);
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

async fn replay_events_text(
    state: DenState,
    token: &str,
    bear_slug: &str,
    session_id: &str,
) -> String {
    let replay = events_page(
        State(state),
        bearer_headers(token),
        Path(session_id.to_string()),
        Query(EventPageQuery {
            bear_slug: bear_slug.to_string(),
            after: None,
            limit: None,
        }),
    )
    .await
    .expect("events page response")
    .0;
    replay.to_string()
}

/// Wait for the client-visible start of a focused Pair run. A persisted
/// execution attempt alone is insufficient: focus used to leave attempts
/// running when setup ended before the Pair loop had actually started.
async fn wait_for_focused_run_started(
    state: DenState,
    _token: &str,
    _bear_slug: &str,
    session_id: &str,
    run_id: &str,
) {
    let mut last_state = None;
    for _ in 0..50 {
        let (started, terminal): (bool, bool) = sqlx::query_as(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM bearwire_events
                WHERE session_id = $1 AND event_type = 'run.started'
                  AND event_json->>'run_id' = $2
            ), EXISTS (
                SELECT 1 FROM bearwire_events
                WHERE session_id = $1
                  AND event_type IN ('run.completed', 'run.failed', 'run.cancelled')
                  AND event_json->>'run_id' = $2
            )
            "#,
        )
        .bind(session_id)
        .bind(run_id)
        .fetch_one(&state.sqlx_pool)
        .await
        .expect("inspect focused run lifecycle");
        last_state = turn_runs::get_run(&state.sqlx_pool, run_id)
            .await
            .expect("load focused run")
            .map(|run| run.state);
        assert!(
            !terminal || started,
            "focused run {run_id} reached terminal state before run.started; state={last_state:?}"
        );
        if started {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("focused run {run_id} did not emit run.started within one second; state={last_state:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_persists_message_delta_and_completed_events_for_mock_llm(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation_id = format!("conv-{}", Uuid::new_v4().simple());

    let response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-run-start")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "conversation_id": conversation_id,
                "client": "bearwire-test",
                "prompt": "Say hello."
            }),
        }),
    )
    .await
    .expect("run.start response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");
    assert_eq!(value["result"]["accepted"], true, "{value}");

    let mut last_replay = String::new();
    for _ in 0..40 {
        last_replay = replay_events_text(state.clone(), &token, &bear_slug, &session_id).await;
        if last_replay.contains("\"type\":\"message.delta\"")
            && last_replay.contains("\"type\":\"run.completed\"")
        {
            assert!(last_replay.contains("hello from bearwire"), "{last_replay}");
            assert!(
                last_replay.contains("\"type\":\"run.accepted\""),
                "{last_replay}"
            );
            assert!(
                last_replay.contains("\"type\":\"run.started\""),
                "{last_replay}"
            );
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "BearWire run.start did not persist message.delta and run.completed events: {last_replay}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_persists_user_prompt_for_future_history(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let prompt = "Remember this first prompt for future turns";

    let response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-persist-user-prompt")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "conversation_id": format!("new-acp-zed-{}", Uuid::new_v4().simple()),
                "client": "zed",
                "prompt": prompt
            }),
        }),
    )
    .await
    .expect("run.start response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");
    let resolved = wait_for_resolved_conversation_id(&pool, user_id, &bear_slug, &session_id).await;
    wait_for_user_message(&pool, bear_id, &resolved, prompt).await;
    let (count,): (i64,) = sqlx::query_as(
        r"
        SELECT COUNT(*)
        FROM conversation_messages
        WHERE conversation_id = (
            SELECT id FROM conversations
            WHERE bear_id = $1 AND external_conversation_id = $2
            LIMIT 1
        )
        AND message_type = 'user'
        AND role = 'user'
        AND content_text = $3
        ",
    )
    .bind(bear_id)
    .bind(resolved)
    .bind(prompt)
    .fetch_one(&pool)
    .await
    .expect("count persisted user prompt");
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_persists_wrapped_host_context_as_structured_metadata(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation_id = format!("new-acp-zed-{}", Uuid::new_v4().simple());
    let prompt = "Please inspect the library entrypoint.";
    let prompt_context = json!({
        "format": "acp_prompt_context.v1",
        "host_context": {
            "kind": "referenced_resources",
            "delivery": "reference_only",
            "persistence": "not_human_message",
            "resources": [
                {
                    "label": "src/lib.rs",
                    "uri": "file:///workspace/src/lib.rs",
                    "name": "src/lib.rs",
                    "mime_type": "text/rust",
                    "embedded_text_bytes": 128
                }
            ]
        }
    });

    let response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-persist-host-context")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "conversation_id": conversation_id,
                "client": "zed",
                "prompt": prompt,
                "prompt_context": prompt_context
            }),
        }),
    )
    .await
    .expect("run.start response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");

    let resolved = wait_for_resolved_conversation_id(&pool, user_id, &bear_slug, &session_id).await;
    wait_for_user_message(&pool, bear_id, &resolved, prompt).await;

    let row = sqlx::query(
        r"
        SELECT content_text, content_json
        FROM conversation_messages
        WHERE conversation_id = (
            SELECT id FROM conversations
            WHERE bear_id = $1 AND external_conversation_id = $2
            LIMIT 1
        )
        AND message_type = 'user'
        AND role = 'user'
        ORDER BY sequence_no DESC
        LIMIT 1
        ",
    )
    .bind(bear_id)
    .bind(&resolved)
    .fetch_one(&pool)
    .await
    .expect("load persisted user prompt row");

    let persisted_text: String = row.try_get("content_text").expect("decode content_text");
    let persisted_json: Value = row.try_get("content_json").expect("decode content_json");
    assert!(
        persisted_text.starts_with("Please inspect the library entrypoint."),
        "persisted text should retain the human prompt: {persisted_text}"
    );
    assert!(
        persisted_text.contains("[Referenced resource: src/lib.rs]"),
        "persisted text should retain the resource marker: {persisted_text}"
    );
    assert_eq!(
        persisted_json["prompt_context"]["format"],
        "acp_prompt_context.v1"
    );
    assert_eq!(
        persisted_json["host_context"]["kind"],
        "referenced_resources"
    );
    assert_eq!(persisted_json["host_context"]["delivery"], "reference_only");
    assert_eq!(
        persisted_json["host_context"]["persistence"],
        "not_human_message"
    );
    assert_eq!(
        persisted_json["host_context"]["resources"][0]["uri"],
        "file:///workspace/src/lib.rs"
    );

    let surface_response = rpc_value(
        state,
        &token,
        "conversation.surface_history",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": resolved,
            "limit": 20
        }),
    )
    .await;
    let surface_events = surface_response["result"]["surface_events"]
        .as_array()
        .expect("surface_events array");
    let user_event = surface_events
        .iter()
        .find(|event| {
            event.get("kind").and_then(Value::as_str) == Some("message")
                && event.get("role").and_then(Value::as_str) == Some("user")
        })
        .unwrap_or_else(|| {
            panic!("surface history should include user message: {surface_response}")
        });
    let surface_text = user_event
        .get("text")
        .and_then(Value::as_str)
        .expect("surface text");
    assert!(
        surface_text.contains("Please inspect the library entrypoint."),
        "surface text should keep the human prompt: {surface_text}"
    );
    assert!(
        surface_text.contains("Referenced resources:"),
        "surface text should render referenced resource heading: {surface_text}"
    );
    assert!(
        surface_text.contains("src/lib.rs"),
        "surface text should render referenced resource label: {surface_text}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_second_turn_replays_first_user_and_assistant_once(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let first_prompt = "First prompt: remember the blue teapot";
    let second_prompt = "What was my first prompt?";
    let mut second = MockLlmRequestAssertion::requiring(vec![
        first_prompt.to_string(),
        "hello from bearwire".to_string(),
        second_prompt.to_string(),
    ]);
    second
        .exact_body_counts
        .push((second_prompt.to_string(), 1));

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion {
            required_body_substrings: vec![first_prompt.to_string()],
            exact_body_counts: vec![(first_prompt.to_string(), 1)],
        },
        second,
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);

    let first_response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-first-history-turn")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "conversation_id": format!("new-acp-zed-{}", Uuid::new_v4().simple()),
                "client": "zed",
                "prompt": first_prompt
            }),
        }),
    )
    .await
    .expect("first run.start response")
    .into_response();
    let body = axum::body::to_bytes(first_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(first_value["result"]["ok"], true, "{first_value}");
    let first_run_id = first_value["result"]["run_id"]
        .as_str()
        .expect("first run_id")
        .to_string();

    let resolved = wait_for_resolved_conversation_id(&pool, user_id, &bear_slug, &session_id).await;

    let first_turn_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM turn_runs WHERE run_id = $1 LIMIT 1")
                .bind(&first_run_id)
                .fetch_optional(&pool)
                .await
                .expect("load first run state");
        let assistant_count: i64 = sqlx::query_scalar(
            r"
            SELECT COUNT(*)::bigint
            FROM conversation_messages
            WHERE conversation_id = (
                SELECT id FROM conversations
                WHERE bear_id = $1 AND external_conversation_id = $2
                LIMIT 1
            )
            AND message_type = 'assistant'
            AND content_text = 'hello from bearwire'
            ",
        )
        .bind(bear_id)
        .bind(&resolved)
        .fetch_one(&pool)
        .await
        .expect("count first assistant message");
        if state.as_deref() == Some("completed") && assistant_count == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < first_turn_deadline,
            "first turn did not complete and persist assistant output before second turn"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let second_response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-second-history-turn")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "client": "zed",
                "prompt": second_prompt
            }),
        }),
    )
    .await
    .expect("second run.start response")
    .into_response();
    let body = axum::body::to_bytes(second_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(second_value["result"]["ok"], true, "{second_value}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn surface_history_projects_persisted_assistant_message(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure conversation");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::Assistant,
            role: Some(ConversationMessageRole::Assistant),
            visibility: ConversationMessageVisibility::Default,
            content_text: "persisted agent message".to_string(),
            content_json: json!({}),
            provider_message_id: Some("persisted-agent-message".to_string()),
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("persist assistant message");

    let response = rpc_value(
        test_state(pool),
        &token,
        "conversation.surface_history",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": conversation_id,
            "limit": 20
        }),
    )
    .await;

    assert!(
        response["result"]["surface_events"]
            .as_array()
            .expect("surface_events array")
            .iter()
            .any(
                |event| event.get("kind").and_then(Value::as_str) == Some("message")
                    && event.get("role").and_then(Value::as_str) == Some("assistant")
                    && event.get("text").and_then(Value::as_str) == Some("persisted agent message"),
            ),
        "BearWire surface history must project the persisted assistant row: {response}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn native_history_loader_replays_canonical_user_and_assistant_rows(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let canonical = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure canonical conversation");
    append_message(
        &pool,
        canonical.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::User,
            role: Some(ConversationMessageRole::User),
            visibility: ConversationMessageVisibility::Default,
            content_text: "first user prompt".to_string(),
            content_json: json!({}),
            provider_message_id: Some("prior-user".to_string()),
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append user message");
    append_message(
        &pool,
        canonical.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::Assistant,
            role: Some(ConversationMessageRole::Assistant),
            visibility: ConversationMessageVisibility::Default,
            content_text: "first assistant reply".to_string(),
            content_json: json!({}),
            provider_message_id: Some("prior-assistant".to_string()),
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append assistant message");
    for index in 0..100 {
        append_message(
            &pool,
            canonical.id,
            &ConversationMessageWrite {
                message_type: ConversationMessageType::WorkflowEvent,
                role: Some(ConversationMessageRole::System),
                visibility: ConversationMessageVisibility::DiagnosticOnly,
                content_text: format!("diagnostic-{index}"),
                content_json: json!({"index": index}),
                provider_message_id: None,
                source_event_id: None,
                created_at: None,
            },
        )
        .await
        .expect("append diagnostic message");
    }

    let user_history = list_projected_messages_page(
        &pool,
        canonical.id,
        None,
        2,
        ConversationHistoryProjection::UserHistory,
    )
    .await
    .expect("load projected user history");
    assert_eq!(user_history.len(), 2);
    assert_eq!(
        user_history
            .iter()
            .filter_map(|message| message.to_user_history_record().map(|record| record.role))
            .collect::<Vec<_>>(),
        vec!["assistant", "user"]
    );
    assert!(user_history
        .iter()
        .all(|message| message.visibility == "default"));

    let surface_response = rpc_value(
        test_state(pool.clone()),
        &token,
        "conversation.surface_history",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": conversation_id,
            "limit": 2
        }),
    )
    .await;
    let surface_messages = surface_response["result"]["surface_events"]
        .as_array()
        .expect("surface_events array")
        .iter()
        .filter(|event| event.get("kind").and_then(Value::as_str) == Some("message"))
        .map(|event| {
            (
                event.get("role").and_then(Value::as_str),
                event.get("text").and_then(Value::as_str),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        surface_messages,
        vec![
            (Some("user"), Some("first user prompt")),
            (Some("assistant"), Some("first assistant reply")),
        ],
        "surface replay must include canonical assistant messages: {surface_response}"
    );

    let backend = NativeRuntimeConversationBackend::with_pool(pool.clone());
    let binding = RoleRuntimeBinding {
        binding_id: format!("den-native:{bear_id}:pair"),
        compatibility_backend: Some("native".to_string()),
    };
    let history = backend
        .load_history(
            &binding,
            &RuntimeConversationRef {
                id: conversation_id,
            },
        )
        .await
        .expect("load native history");

    assert_eq!(history.records.len(), 2);
    assert!(matches!(
        &history.records[0],
        RuntimeHistoryRecord::Message { role, content, .. }
        if role == "user" && content == "first user prompt"
    ));
    assert!(matches!(
        &history.records[1],
        RuntimeHistoryRecord::Message { role, content, .. }
        if role == "assistant" && content == "first assistant reply"
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_uses_resolved_conversation_history_for_existing_session(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let pending_conversation_id = format!("new-acp-zed-{}", Uuid::new_v4().simple());
    let resolved_conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: bear_slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{session_id}"),
            conversation_id: pending_conversation_id.clone(),
            resolved_conversation_id: Some(resolved_conversation_id.clone()),
            client: "zed".to_string(),
            cwd: Some("/workspace".to_string()),
            current_mode: Some(client_sessions::ClientSessionMode::Write),
        },
    )
    .await
    .expect("upsert resolved BearWire session");
    let canonical = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &resolved_conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure resolved conversation");
    append_message(
        &pool,
        canonical.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::User,
            role: Some(ConversationMessageRole::User),
            visibility: ConversationMessageVisibility::Default,
            content_text: "Earlier user asked about cached history".to_string(),
            content_json: json!({}),
            provider_message_id: Some("prior-user".to_string()),
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append prior user message");
    append_message(
        &pool,
        canonical.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::Assistant,
            role: Some(ConversationMessageRole::Assistant),
            visibility: ConversationMessageVisibility::Default,
            content_text: "Earlier assistant reply from persisted history".to_string(),
            content_json: json!({}),
            provider_message_id: Some("prior-assistant".to_string()),
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append prior assistant message");

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_body(vec![
        "Earlier user asked about cached history".to_string(),
        "Earlier assistant reply from persisted history".to_string(),
        "Current turn should see history".to_string(),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let response = rpc(
        State(state.clone()),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-history-run-start")),
            method: "run.start".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
                "client": "zed",
                "prompt": "Current turn should see history"
            }),
        }),
    )
    .await
    .expect("run.start response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_state_auth_error_reports_specific_token_bear_diagnostics(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let (other_bear_id, other_bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    bears_db::grant_membership(
        &pool,
        user_id,
        other_bear_id,
        Some(bears_db::BEAR_ROLE_ADMIN),
    )
    .await
    .expect("grant membership to other Bear");

    let response = rpc(
        State(test_state(pool)),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-state-diagnostics")),
            method: "session.state".to_string(),
            params: json!({
                "bear_slug": other_bear_slug,
                "limit": 1,
            }),
        }),
    )
    .await
    .expect("session.state response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    let error = value["error"]["data"]["error"].as_str().unwrap();
    assert!(error.contains("token_found=true"), "{error}");
    assert!(error.contains("bear_found=true"), "{error}");
    assert!(error.contains("token_bound_to_bear=false"), "{error}");
    assert!(error.contains("token_owner_is_bear_member=true"), "{error}");
    assert!(error.contains("required_scope_present=true"), "{error}");
    assert!(
        error.contains("token is not granted to this Bear"),
        "{error}"
    );
    assert!(
        error.contains(&format!("bear_slug=\"{}\"", other_bear_slug)),
        "{error}"
    );
    assert!(
        !error.contains(&token),
        "diagnostics must not echo raw token"
    );
    assert!(
        !error.contains(&bear_slug),
        "diagnostics should only report requested Bear slug"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_state_includes_trusted_workspace_diagnostics(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    client_sessions::update_adapter_environment(
        &pool,
        user_id,
        bear_id,
        &session_id,
        &json!({
            "cwd": "/workspace/project",
            "workspace_roots": ["/workspace/project", "/workspace/shared"]
        }),
    )
    .await
    .expect("update adapter environment");

    let response = rpc_value(
        test_state(pool),
        &token,
        "session.state",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id
        }),
    )
    .await;
    let diagnostics = &response["result"]["session"]["diagnostics"];
    assert_eq!(
        diagnostics["trusted_workspace"]["cwd"],
        "/workspace/project"
    );
    assert_eq!(
        diagnostics["trusted_workspace"]["roots"][0],
        "/workspace/project"
    );
    assert_eq!(
        diagnostics["trusted_workspace"]["source"],
        "trusted_session"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_state_auth_error_reports_missing_bear_slug(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, _bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let missing_slug = format!("missing-bear-{}", Uuid::new_v4().simple());

    let response = rpc(
        State(test_state(pool)),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-missing-bear")),
            method: "session.state".to_string(),
            params: json!({
                "bear_slug": missing_slug,
                "limit": 1,
            }),
        }),
    )
    .await
    .expect("session.state response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    let error = value["error"]["data"]["error"].as_str().unwrap();
    assert!(error.contains("token_found=true"), "{error}");
    assert!(error.contains("bear_found=false"), "{error}");
    assert!(
        error.contains("bear slug does not exist in this Den database"),
        "{error}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_state_includes_latest_context_budget_for_resolved_conversation(
    pool: sqlx::PgPool,
) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let pending_conversation_id = format!("pending-{}", Uuid::new_v4().simple());
    let resolved_conversation_id = format!("resolved-{}", Uuid::new_v4().simple());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: bear_slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{session_id}"),
            conversation_id: pending_conversation_id,
            resolved_conversation_id: Some(resolved_conversation_id.clone()),
            client: "zed".to_string(),
            cwd: Some("/workspace".to_string()),
            current_mode: Some(client_sessions::ClientSessionMode::Write),
        },
    )
    .await
    .expect("upsert session");

    let report = ContextBudgetReport {
        model: "openai/test-model".to_string(),
        context_window: Some(128_000),
        max_output_tokens: Some(4_096),
        reserved_output_tokens: 4_096,
        estimated_input_tokens: 12_345,
        estimated_total_tokens: 16_441,
        estimate_precision: ContextBudgetEstimatePrecision::Approximate,
        near_budget: false,
        over_budget: false,
        calibration: None,
        components: vec![ContextBudgetComponentReport {
            key: "history".to_string(),
            label: "Conversation history".to_string(),
            estimated_tokens: 12_000,
            estimated_characters: 48_000,
        }],
    };
    update_latest_context_budget(
        &pool,
        bear_id,
        &resolved_conversation_id,
        Some(&session_id),
        &report,
    )
    .await
    .expect("persist latest context budget");

    let response = rpc(
        State(test_state(pool)),
        bearer_headers(&token),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-context-budget")),
            method: "session.state".to_string(),
            params: json!({
                "bear_slug": bear_slug,
                "session_id": session_id,
            }),
        }),
    )
    .await
    .expect("session.state response")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        value.pointer("/result/session/history_conversation_id").and_then(Value::as_str),
        Some(resolved_conversation_id.as_str()),
        "session.state must expose the canonical history conversation ID, not the pending client ID"
    );
    assert_eq!(
        value.pointer("/result/session/context_budget").cloned(),
        Some(serde_json::to_value(report).unwrap())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn client_result_recording_is_idempotent_and_detects_conflicts(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, _bear_slug) = create_test_bear(&pool).await;
    let run = turn_runs::create_run(
        &pool,
        "run-idempotency-test",
        "session-idempotency-test",
        bear_id,
        user_id,
    )
    .await
    .expect("create run");
    assert_eq!(run.state, "accepted");

    let first = turn_runs::record_client_result(
        &pool,
        "run-idempotency-test",
        "tool",
        "call-1",
        json!({ "status": "ok", "content": "same" }),
    )
    .await
    .expect("record first result");
    assert!(matches!(
        first,
        turn_runs::TurnObligationResultRecord::Inserted { .. }
    ));

    let duplicate = turn_runs::record_client_result(
        &pool,
        "run-idempotency-test",
        "tool",
        "call-1",
        json!({ "status": "ok", "content": "same" }),
    )
    .await
    .expect("record duplicate result");
    assert!(matches!(
        duplicate,
        turn_runs::TurnObligationResultRecord::DuplicateIdentical { .. }
    ));

    let conflict = turn_runs::record_client_result(
        &pool,
        "run-idempotency-test",
        "tool",
        "call-1",
        json!({ "status": "ok", "content": "different" }),
    )
    .await
    .expect("record conflicting result");
    assert!(matches!(
        conflict,
        turn_runs::TurnObligationResultRecord::DuplicateConflict { .. }
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn client_result_methods_reject_wrong_tool_kind_and_ignore_stale_permission_result(
    pool: sqlx::PgPool,
) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    turn_obligations::upsert_permission_decision_obligation(
        &pool,
        &run_id,
        &session_id,
        "perm-wrong-tool-route",
        Some("call-wrong-tool-route"),
        json!({ "test": "permission obligation" }),
    )
    .await
    .expect("insert permission obligation");
    let tool_response = rpc_value(
        test_state(pool.clone()),
        &token,
        "client.tool.result",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "run_id": run_id,
            "tool_call_id": "call-wrong-tool-route",
            "status": "ok",
            "content": "not accepted by permission obligation"
        }),
    )
    .await;
    let tool_error = tool_response["error"]["data"]["error"]
        .as_str()
        .expect("JSON-RPC validation error detail");
    assert!(
        tool_error.contains("does not accept client.tool.result"),
        "{tool_response}"
    );

    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-wrong-permission-route",
        Some("perm-wrong-permission-route"),
        json!({ "test": "tool obligation" }),
    )
    .await
    .expect("insert tool obligation");
    let permission_response = rpc_value(
        test_state(pool.clone()),
        &token,
        "client.permission.result",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "run_id": run_id,
            "permission_id": "perm-wrong-permission-route",
            "decision": "approved"
        }),
    )
    .await;
    assert_eq!(
        permission_response["result"]["ok"], true,
        "{permission_response}"
    );
    assert_eq!(
        permission_response["result"]["status"], "late_result_ignored",
        "{permission_response}"
    );
    assert_eq!(
        permission_response["result"]["duplicate"], true,
        "{permission_response}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn tool_result_without_live_native_session_is_not_accepted_for_continuation(
    pool: sqlx::PgPool,
) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let tool_call_id = format!("call_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("transition run to running");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        &tool_call_id,
        None,
        json!({ "test": "fresh-state persisted obligation" }),
    )
    .await
    .expect("insert tool obligation");

    let params = json!({
        "bear_slug": bear_slug,
        "session_id": session_id,
        "run_id": run_id,
        "tool_call_id": tool_call_id,
        "status": "ok",
        "content": "persisted tool result"
    });
    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "client.tool.result",
        params.clone(),
    )
    .await;
    assert_eq!(response["result"]["ok"], false, "{response}");
    assert_eq!(
        response["result"]["status"], "continuation_unavailable",
        "{response}"
    );
    assert_eq!(
        response["result"]["reason"], "native_agent_loop_session_not_found",
        "{response}"
    );
    assert_eq!(
        response["result"]["diagnostic"]["run_id"], run_id,
        "{response}"
    );
    let obligation = turn_obligations::get_tool_call_obligation(&pool, &run_id, &tool_call_id)
        .await
        .expect("load obligation")
        .expect("obligation exists");
    assert_eq!(obligation.state, "waiting_for_client");
    let recorded = turn_runs::existing_client_result_for_payload(
        &pool,
        &run_id,
        "tool",
        &tool_call_id,
        &json!({
            "tool_call_id": tool_call_id,
            "status": "ok",
            "content": "persisted tool result",
            "structured_content": Value::Null,
            "error": Value::Null,
        }),
    )
    .await
    .expect("query existing result");
    assert!(
        recorded.is_none(),
        "result must not be recorded when continuation is unavailable"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_state_reports_run_obligations_results_and_events(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let obligation = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-state",
        None,
        json!({ "tool_name": "fs_list_directory" }),
    )
    .await
    .expect("create obligation");
    turn_runs::record_client_result(
        &pool,
        &run_id,
        "tool",
        "call-state",
        json!({ "status": "ok", "content": "listed" }),
    )
    .await
    .expect("record result");
    let mut event = BearWireEvent::ephemeral(
        "tool_call.completed",
        json!({ "tool_call": { "id": "call-state", "name": "fs_list_directory" } }),
    );
    event.run_id = Some(run_id.clone());
    bearwire_events::append_bearwire_event(&pool, &session_id, Some(bear_id), Some(user_id), event)
        .await
        .expect("append event");

    let response = rpc_value(
        test_state(pool),
        &token,
        "run.state",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "run_id": run_id,
            "limit": 10,
        }),
    )
    .await;

    let result = &response["result"];
    assert_eq!(result["kind"], "run_state", "{response}");
    assert_eq!(result["run"]["run_id"], run_id);
    assert_eq!(result["blocking_reason"], "tool_result");
    assert_eq!(result["obligations"][0]["id"], obligation.id.to_string());
    assert_eq!(result["results"][0]["obligation_id"], "call-state");
    assert_eq!(
        result["recent_events"][0]["event_type"],
        "tool_call.completed"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn client_tool_result_persists_output_summary_and_preview(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let tool_call_id = format!("call_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        &tool_call_id,
        None,
        json!({ "tool_name": "fs_read_text_file" }),
    )
    .await
    .expect("insert tool obligation");
    let compacted = den_core::tools::result_compaction::compact_client_tool_result(
        &den_core::tools::result_compaction::ClientToolResultInput::new(
            tool_call_id.clone(),
            Some("fs_read_text_file".to_string()),
            den_core::tools::result_compaction::ToolResultStatus::Ok,
            Some("file contents".to_string()),
            Value::Null,
            Value::Null,
        ),
    );
    assert_eq!(
        compacted.payload["output_summary"],
        json!("Used fs_read_text_file (ok): file contents")
    );
    assert_eq!(compacted.payload["output_preview"], json!("file contents"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn persist_run_failed_writes_hidden_model_visible_operational_outcome(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let session =
        client_sessions::find_for_user_bear_session(&pool, user_id, &bear_slug, &session_id)
            .await
            .expect("load session")
            .expect("session exists");

    persist_run_failed(
        &pool,
        &session_id,
        &run_id,
        bear_id,
        user_id,
        RunFailureReason::RuntimeInternal,
        "I stopped because this turn exhausted its wall-clock budget (elapsed=252985ms/limit=240000ms).".to_string(),
        None,
    )
    .await;

    let rows = sqlx::query(
        r"
        SELECT message_type, role, visibility, content_text, content_json
        FROM conversation_messages
        WHERE conversation_id = (
            SELECT id FROM conversations
            WHERE bear_id = $1 AND external_conversation_id = $2
            LIMIT 1
        )
        ORDER BY sequence_no ASC
        ",
    )
    .bind(bear_id)
    .bind(&session.conversation_id)
    .fetch_all(&pool)
    .await
    .expect("query operational outcome rows");
    assert_eq!(rows.len(), 2, "hidden model note plus visible marker");
    let hidden = rows
        .iter()
        .find(|row| {
            row.try_get::<String, _>("visibility")
                .is_ok_and(|visibility| visibility == "hidden_from_user")
        })
        .expect("hidden operational outcome row");
    let message_type: String = hidden.try_get("message_type").expect("decode message_type");
    let role: Option<String> = hidden.try_get("role").expect("decode role");
    let visibility: String = hidden.try_get("visibility").expect("decode visibility");
    let content_text: String = hidden.try_get("content_text").expect("decode content_text");
    let content_json: Value = hidden.try_get("content_json").expect("decode content_json");

    assert_eq!(message_type, "assistant");
    assert_eq!(role.as_deref(), Some("assistant"));
    assert_eq!(visibility, "hidden_from_user");
    assert!(!content_text.starts_with("Operational note from Den:"));
    assert!(content_text.contains("Previous turn stopped"));
    assert_eq!(content_json["event"], "operational_outcome");
    assert_eq!(content_json["reason"], "runtime_internal");
    assert_eq!(content_json["run_id"], run_id);

    let visible = rows
        .iter()
        .find(|row| {
            row.try_get::<String, _>("visibility")
                .is_ok_and(|visibility| visibility == "default")
        })
        .expect("visible runtime marker row");
    let marker_text: String = visible.try_get("content_text").expect("decode marker text");
    let marker_json: Value = visible.try_get("content_json").expect("decode marker json");
    assert!(
        marker_text.contains("**Den**: BearWire Test Bear stopped this turn after it ran too long")
    );
    assert_eq!(marker_json["event"], "runtime_marker");
    assert_eq!(marker_json["marker_kind"], "operational_outcome");
    assert_eq!(marker_json["run_id"], run_id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_history_returns_tool_result_summary_from_persisted_record(
    pool: sqlx::PgPool,
) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    client_sessions::mark_resolved(&pool, user_id, bear_id, &session_id, &conversation_id)
        .await
        .expect("mark test session conversation resolved");
    client_sessions::set_title_for_bear_conversation(
        &pool,
        bear_id,
        &conversation_id,
        "History replay title",
    )
    .await
    .expect("set test conversation title");
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure conversation");
    let context = canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(user_id),
        conversation_id.clone(),
        Some(session_id.clone()),
        Some("req-history".to_string()),
        session_id.clone(),
        false,
    );
    persist_canonical_conversation_record(
        &context,
        &CanonicalConversationRecord::visible_user_message(
            "Read that file",
            json!({ "event": "user_message" }),
            None,
        ),
    )
    .await
    .expect("persist user message");
    persist_canonical_conversation_record(
        &context,
        &CanonicalConversationRecord::visible_assistant_message(
            "I found the requested file.",
            json!({ "event": "assistant_message" }),
            None,
        ),
    )
    .await
    .expect("persist assistant message");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite::structured(
            ConversationMessageType::ToolCall,
            Some(ConversationMessageRole::Assistant),
            ConversationMessageVisibility::Default,
            "",
            json!({
                "event": "tool_request",
                "tool_call_id": "call-history",
                "tool_name": "fs_read_text_file",
                "args": { "path": "README.md" },
                "approval_required": false
            }),
        ),
    )
    .await
    .expect("persist tool call");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite::structured(
            ConversationMessageType::ToolResult,
            Some(ConversationMessageRole::System),
            ConversationMessageVisibility::Default,
            "Tool result: fs_read_text_file",
            json!({
                "event": "tool_result",
                "tool_call_id": "call-history",
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "",
                "structured_content": { "content": "hello from file" },
                "output_summary": "Used fs_read_text_file (ok)"
            }),
        ),
    )
    .await
    .expect("persist visible tool result");
    let diagnostic_sentinel = "diagnostic-only tool result sentinel";
    persist_canonical_conversation_record(
        &context,
        &CanonicalConversationRecord::tool_result(
            CanonicalToolResultRecord::new(
                Some("fs_read_text_file".to_string()),
                "call-history",
                None,
                den_core::tools::result_compaction::ToolResultStatus::Ok,
                Some(diagnostic_sentinel.to_string()),
                json!({ "content": diagnostic_sentinel }),
                Value::Null,
                Some("req-history".to_string()),
            ),
            &ConversationEventProvenance::client_session(session_id.clone()),
        ),
    )
    .await
    .expect("persist diagnostic-only tool result");
    let stored = den_service::conversation::persistence::list_messages_page(
        &pool,
        conversation.id,
        None,
        20,
    )
    .await
    .expect("load raw conversation records");
    assert!(stored.iter().any(|row| {
        row.storage_visibility().ok() == Some(ConversationMessageVisibility::DiagnosticOnly)
            && row
                .content_json
                .pointer("/structured_content/content")
                .and_then(Value::as_str)
                == Some(diagnostic_sentinel)
    }));
    let model_replay = NativeRuntimeConversationBackend::with_pool(pool.clone())
        .load_history(
            &RoleRuntimeBinding {
                binding_id: format!("den-native:{bear_id}:pair"),
                compatibility_backend: Some("native".to_string()),
            },
            &RuntimeConversationRef {
                id: conversation_id.clone(),
            },
        )
        .await
        .expect("load model transcript");
    assert_eq!(model_replay.records.len(), 4, "{model_replay:?}");
    assert!(matches!(
        &model_replay.records[2],
        RuntimeHistoryRecord::ToolCall { tool_call_id, .. } if tool_call_id == "call-history"
    ));
    assert!(matches!(
        &model_replay.records[3],
        RuntimeHistoryRecord::ToolResult { tool_call_id, structured_content, .. }
            if tool_call_id.as_deref() == Some("call-history")
                && structured_content["content"] == "hello from file"
    ));
    assert!(
        !format!("{model_replay:?}").contains(diagnostic_sentinel),
        "diagnostic-only result must not enter the model transcript: {model_replay:?}"
    );

    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "conversation.history",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": conversation_id,
            "limit": 20
        }),
    )
    .await;
    let messages = response["result"]["messages"]
        .as_array()
        .expect("messages array");
    assert!(
        messages.iter().any(|message| {
            message.get("kind").and_then(Value::as_str) == Some("message")
                && message.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("text").and_then(Value::as_str)
                    == Some("I found the requested file.")
        }),
        "conversation history must replay persisted assistant output: {response}"
    );
    let tool_call = messages
        .iter()
        .find(|message| message.get("kind").and_then(Value::as_str) == Some("tool_call"))
        .unwrap_or_else(|| panic!("missing structured tool_call in {response}"));
    assert_eq!(tool_call["tool_call_id"], "call-history");
    assert_eq!(tool_call["tool_name"], "fs_read_text_file");
    assert_eq!(tool_call["status"], "pending");
    assert_eq!(tool_call["arguments"]["path"], "README.md");

    let tool_result = messages
        .iter()
        .find(|message| message.get("kind").and_then(Value::as_str) == Some("tool_result"))
        .unwrap_or_else(|| panic!("missing structured tool_result in {response}"));
    assert_eq!(tool_result["tool_call_id"], "call-history");
    assert_eq!(tool_result["tool_name"], "fs_read_text_file");
    assert_eq!(tool_result["status"], "ok");
    assert_eq!(tool_result["raw_output"]["content"], "hello from file");
    assert!(
        !response.to_string().contains(diagnostic_sentinel),
        "diagnostic-only result must not appear in conversation history: {response}"
    );
    assert_ne!(
        tool_result.get("text").and_then(Value::as_str),
        Some("Used fs_read_text_file (incomplete)")
    );
    bearwire_events::append_bearwire_event(
        &pool,
        &session_id,
        Some(bear_id),
        Some(user_id),
        bearwire_protocol::wire::BearWireEvent::ephemeral(
            "session_info_update",
            json!({
                "title": "Persisted replay title",
                "updated_at": "2026-07-07T00:00:00Z"
            }),
        ),
    )
    .await
    .expect("persist session info surface event");
    bearwire_events::append_bearwire_event(
        &pool,
        &session_id,
        Some(bear_id),
        Some(user_id),
        bearwire_protocol::wire::BearWireEvent::ephemeral(
            "message.reasoning.delta",
            json!({
                "delta": "thinking privately",
                "source": "provider_reasoning",
                "replay_policy": "none"
            }),
        ),
    )
    .await
    .expect("persist omitted reasoning surface event");
    bearwire_events::append_bearwire_event(
        &pool,
        &session_id,
        Some(bear_id),
        Some(user_id),
        bearwire_protocol::wire::BearWireEvent::ephemeral(
            "message.reasoning.delta",
            json!({
                "delta": "replayable thought",
                "source": "provider_reasoning",
                "replay_policy": "thought"
            }),
        ),
    )
    .await
    .expect("persist replayable reasoning surface event");
    bearwire_events::append_bearwire_event(
        &pool,
        &session_id,
        Some(bear_id),
        Some(user_id),
        bearwire_protocol::wire::BearWireEvent::ephemeral(
            "message.reasoning.delta",
            json!({
                "delta": "unsupported replay policy thought",
                "source": "provider_reasoning",
                "replay_policy": "summary_once"
            }),
        ),
    )
    .await
    .expect("persist unsupported reasoning replay policy event");

    let docket_job_id: Uuid = sqlx::query_scalar(
        r"
        INSERT INTO bear_jobs (
            bear_id, created_by_user_id, created_by_role, goal, source_conversation_id
        )
        VALUES ($1, $2, 'pair', 'Surface diagnostics job', $3)
        RETURNING id
        ",
    )
    .bind(bear_id)
    .bind(user_id)
    .bind(&conversation_id)
    .fetch_one(&pool)
    .await
    .expect("insert docket job");
    let docket_run_id: Uuid = sqlx::query_scalar(
        r"
        INSERT INTO bear_job_runs (job_id, state, started_at)
        VALUES ($1, 'running', NOW())
        RETURNING id
        ",
    )
    .bind(docket_job_id)
    .fetch_one(&pool)
    .await
    .expect("insert docket run");
    sqlx::query("UPDATE bear_jobs SET current_run_id = $2 WHERE id = $1")
        .bind(docket_job_id)
        .bind(docket_run_id)
        .execute(&pool)
        .await
        .expect("attach docket run");
    let docket_task_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO bear_tasks (
            bear_id, job_id, kind, scope, title, body, completion_criteria, created_by_role, created_by_user_id
        )
        VALUES ($1, $2, 'execution', 'template', 'Diagnostic task', 'Check projection', '["projection includes task"]'::jsonb, 'pair', $3)
        RETURNING id
        "#,
    )
    .bind(bear_id)
    .bind(docket_job_id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("insert docket task");
    sqlx::query(
        r"
        INSERT INTO docket_execution_attempts (
            bear_id, task_id, binding_kind, binding_id, host_kind, host_run_id,
            fence_epoch, authorization_key, state, started_at
        )
        VALUES ($1, $2, 'client_session', $3, 'pair', $4::text,
                1, $5, 'running', NOW())
        ",
    )
    .bind(bear_id)
    .bind(docket_task_id)
    .bind(&session_id)
    .bind(docket_run_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("insert canonical docket execution attempt");
    sqlx::query(
        r"
        INSERT INTO bear_task_events (task_id, run_id, event_type, by_role, by_user_id, payload)
        VALUES ($1, $2, 'created', 'pair', $3, $4::jsonb)
        ",
    )
    .bind(docket_task_id)
    .bind(docket_run_id)
    .bind(user_id)
    .bind(json!({
        "definition": {
            "title": "Diagnostic task"
        }
    }))
    .execute(&pool)
    .await
    .expect("insert task definition event");
    bearwire_events::append_bearwire_event(
        &pool,
        &session_id,
        Some(bear_id),
        Some(user_id),
        bearwire_protocol::wire::BearWireEvent::ephemeral(
            "runtime.objective_orientation",
            json!({
                "source": "turn_assembly",
                "profile": "pair",
                "conversation_id": conversation_id,
                "kind": "focused",
                "orientation": {
                    "kind": "focused",
                    "job": {
                        "job_id": docket_job_id.to_string(),
                        "active_task_ref": {
                            "kind": "docket_task",
                            "job_id": docket_job_id.to_string(),
                            "task_id": docket_task_id.to_string(),
                            "title": "Diagnostic task"
                        },
                        "mutable": true
                    }
                }
            }),
        ),
    )
    .await
    .expect("persist orientation diagnostic event");

    let surface_response = rpc_value(
        test_state(pool),
        &token,
        "conversation.surface_history",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": conversation_id,
            "limit": 20
        }),
    )
    .await;
    assert_eq!(
        surface_response["result"]["kind"],
        "conversation_surface_history"
    );
    let surface_events = surface_response["result"]["surface_events"]
        .as_array()
        .expect("surface_events array");
    assert!(
        !surface_response.to_string().contains(diagnostic_sentinel),
        "diagnostic-only tool result must not appear in user-visible surface history: {surface_response}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("message")
                && event.get("role").and_then(Value::as_str) == Some("assistant")
                && event.get("text").and_then(Value::as_str) == Some("I found the requested file.")
        }),
        "surface history must replay persisted assistant output: {surface_response}"
    );
    let message_event = surface_events
        .iter()
        .find(|event| {
            event.get("kind").and_then(Value::as_str) == Some("message")
                && event.get("role").and_then(Value::as_str) == Some("user")
                && event.get("text").and_then(Value::as_str) == Some("Read that file")
        })
        .unwrap_or_else(|| panic!("missing typed message surface event in {surface_response}"));
    assert!(
        message_event
            .get("created_at")
            .and_then(Value::as_str)
            .is_some(),
        "message surface event created_at must be a string: {message_event}"
    );
    assert!(
        matches!(
            serde_json::from_value::<SurfaceHistoryEvent>(message_event.clone()),
            Ok(SurfaceHistoryEvent::Message { .. })
        ),
        "message surface event should decode as shared SurfaceHistoryEvent::Message: {message_event}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("session_info_update")
                && event.get("title").and_then(Value::as_str) == Some("History replay title")
                && event.get("current_mode").and_then(Value::as_str) == Some("write")
        }),
        "surface history should expose typed session metadata update from latest session state: {surface_response}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("session_info_update")
                && event.get("title").and_then(Value::as_str) == Some("Persisted replay title")
                && event.get("title_updated_at").and_then(Value::as_str)
                    == Some("2026-07-07T00:00:00Z")
        }),
        "surface history should expose persisted typed session metadata update: {surface_response}"
    );
    assert!(
        !surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("reasoning_delta")
                && event.get("text").and_then(Value::as_str) == Some("thinking privately")
        }),
        "surface history should omit reasoning with replay_policy=none: {surface_response}"
    );
    assert!(
        !surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("reasoning_delta")
                && event.get("text").and_then(Value::as_str) == Some("replayable thought")
        }),
        "conversation surface history should omit transient reasoning events: {surface_response}"
    );
    assert!(
        !surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("reasoning_delta")
                && event.get("text").and_then(Value::as_str)
                    == Some("unsupported replay policy thought")
        }),
        "surface history should omit unsupported reasoning replay policies: {surface_response}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("tool_call")
                && event.get("tool_call_id").and_then(Value::as_str) == Some("call-history")
                && event.get("tool_name").and_then(Value::as_str) == Some("fs_read_text_file")
                && event.get("status").and_then(Value::as_str) == Some("pending")
                && event.pointer("/arguments/path").and_then(Value::as_str) == Some("README.md")
        }),
        "surface history should expose full structured tool-call start: {surface_response}"
    );
    assert!(
        surface_events
            .iter()
            .any(
                |event| event.get("kind").and_then(Value::as_str) == Some("tool_result")
                    && event.get("status").and_then(Value::as_str) == Some("ok")
            ),
        "surface history should expose structured ok tool result: {surface_response}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("message")
                && event.get("role").and_then(Value::as_str) == Some("system")
                && event
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("Docket task created: Diagnostic task"))
        }),
        "surface history should expose Docket task definition diagnostics: {surface_response}"
    );
    assert!(
        surface_events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("message")
                && event.get("role").and_then(Value::as_str) == Some("system")
                && event
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        text.contains("Runtime orientation: kind=focused")
                            && text.contains(&format!("job={docket_job_id}"))
                            && text.contains(&format!("task={docket_task_id}"))
                    })
        }),
        "surface history should expose persisted orientation diagnostics: {surface_response}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_reuses_active_run_unless_explicitly_superseded(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-encryption-key".to_string();
    config.llm_api_url =
        start_mock_openai_sse_server_asserting_requests(vec![MockLlmRequestAssertion::requiring(
            Vec::new(),
        )]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let active_run_id = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(&pool, &active_run_id, &session_id, bear_id, user_id)
        .await
        .expect("create active run");

    let retry = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bearwire-test",
            "prompt": "Retry the same request."
        }),
    )
    .await;
    assert!(retry.get("error").is_none(), "{retry}");
    assert_eq!(retry["result"]["reused"], true, "{retry}");
    assert_eq!(retry["result"]["run_id"], active_run_id, "{retry}");
    assert_eq!(
        turn_runs::active_run_for_session(&pool, &session_id)
            .await
            .expect("load active run")
            .expect("active run remains")
            .run_id,
        active_run_id
    );

    let replacement = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "client": "bearwire-test",
            "prompt": "A distinct user message.",
            "supersede_active_run": true
        }),
    )
    .await;
    assert!(replacement.get("error").is_none(), "{replacement}");
    assert_ne!(
        replacement["result"]["run_id"], active_run_id,
        "{replacement}"
    );
    let previous = turn_runs::get_run(&pool, &active_run_id)
        .await
        .expect("load prior run")
        .expect("prior run exists");
    assert_eq!(previous.state, "cancelled");
    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list lifecycle events");
    let accepted = events
        .iter()
        .find(|event| {
            event.event_type == "run.accepted"
                && event.event.run_id.as_deref() == replacement["result"]["run_id"].as_str()
        })
        .expect("replacement run.accepted event");
    assert_eq!(
        accepted.event.data["creation_cause"],
        "explicit_supersession"
    );
    let cancelled = events
        .iter()
        .find(|event| {
            event.event_type == "run.cancelled"
                && event.event.run_id.as_deref() == Some(&active_run_id)
        })
        .expect("superseded run.cancelled event");
    assert_eq!(
        cancelled.event.data["superseded_by_run_id"],
        replacement["result"]["run_id"]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_session_rejects_second_active_run(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_a = format!("run_{}", Uuid::new_v4().simple());
    let run_b = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_a, &session_id, bear_id, user_id)
        .await
        .expect("create first active run");

    let err = turn_runs::create_run(&pool, &run_b, &session_id, bear_id, user_id)
        .await
        .expect_err("second active run in one ACP session should be rejected");
    assert!(
        err.to_string()
            .contains("idx_turn_runs_one_active_per_session"),
        "unexpected error: {err}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_session_non_superseding_start_attaches_active_run(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_a = format!("run_{}", Uuid::new_v4().simple());
    let run_b = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_a, &session_id, bear_id, user_id)
        .await
        .expect("create first active run");

    let session_id = ClientSessionId::new(session_id).expect("valid session id");
    let run_b = TurnRunId::new(run_b).expect("valid run id");
    let attached = turn_runs::create_or_attach_active_run_with_ids(
        &pool,
        &run_b,
        &session_id,
        bear_id,
        user_id,
    )
    .await
    .expect("attach to active run");
    let turn_runs::CreateOrAttachRun::Attached(attached) = attached else {
        panic!("non-superseding start should attach rather than create a second run");
    };
    assert_eq!(attached.run_id, run_a);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_non_superseding_starts_create_one_run(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = ClientSessionId::new(format!("session-{}", Uuid::new_v4().simple()))
        .expect("valid session id");
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, session_id.as_str()).await;
    let first_run = TurnRunId::new(format!("run_{}", Uuid::new_v4().simple())).expect("run id");
    let second_run = TurnRunId::new(format!("run_{}", Uuid::new_v4().simple())).expect("run id");

    let first = turn_runs::create_or_attach_active_run_with_ids(
        &pool,
        &first_run,
        &session_id,
        bear_id,
        user_id,
    );
    let second = turn_runs::create_or_attach_active_run_with_ids(
        &pool,
        &second_run,
        &session_id,
        bear_id,
        user_id,
    );
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first create-or-attach succeeds");
    let second = second.expect("second create-or-attach succeeds");

    let created_run_id = match (&first, &second) {
        (
            turn_runs::CreateOrAttachRun::Created(created),
            turn_runs::CreateOrAttachRun::Attached(attached),
        )
        | (
            turn_runs::CreateOrAttachRun::Attached(attached),
            turn_runs::CreateOrAttachRun::Created(created),
        ) => {
            assert_eq!(attached.run_id, created.run_id);
            created.run_id.as_str()
        }
        _ => panic!("concurrent starts must create one run and attach the other"),
    };
    assert!(created_run_id == first_run.as_str() || created_run_id == second_run.as_str());
}

#[sqlx::test(migrations = "../../migrations")]
async fn superseding_active_run_allows_new_run_for_session(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_a = format!("run_{}", Uuid::new_v4().simple());
    let run_b = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_a, &session_id, bear_id, user_id)
        .await
        .expect("create first active run");

    let superseded = turn_runs::supersede_active_run_for_session(
        &pool,
        &session_id,
        bear_id,
        user_id,
        "superseded_by_new_run",
    )
    .await
    .expect("supersede active run")
    .expect("active run should be superseded");
    assert_eq!(superseded.run_id, run_a);
    assert_eq!(superseded.state, "failed");

    let created = turn_runs::create_run(&pool, &run_b, &session_id, bear_id, user_id)
        .await
        .expect("create replacement active run");
    assert_eq!(created.run_id, run_b);
}

#[sqlx::test(migrations = "../../migrations")]
async fn approval_required_tool_request_creates_permission_obligation(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let tool_call_id = "call-needs-permission";
    let permission_id = "perm-needs-permission";
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create active run");

    let state = test_state(pool.clone());
    crate::methods::run::persist_runtime_event_as_bearwire(
        &state,
        &pool,
        &session_id,
        &run_id,
        bear_id,
        user_id,
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id: tool_call_id.to_string(),
            tool_name: "fs_list_directory".to_string(),
            title: None,
            kind: Some("read".to_string()),
            arguments: json!({ "path": "/workspace" }),
            approval_request_id: Some(permission_id.to_string()),
            approval_required: true,
            approval_reason: Some("needs approval".to_string()),
            run_id: Some(run_id.clone()),
        }),
        Uuid::new_v4(),
        None,
    )
    .await;

    let obligation = turn_obligations::get_permission_obligation(&pool, &run_id, permission_id)
        .await
        .expect("load permission obligation")
        .expect("permission obligation exists");
    assert_eq!(obligation.expected_responder_action, "permission_decision");
    assert_eq!(obligation.tool_call_id.as_deref(), Some(tool_call_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_session_tool_call_id_collision_is_isolated_by_run_and_session(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_a = format!("session-a-{}", Uuid::new_v4().simple());
    let session_b = format!("session-b-{}", Uuid::new_v4().simple());
    let run_a = format!("run_{}", Uuid::new_v4().simple());
    let run_b = format!("run_{}", Uuid::new_v4().simple());
    let tool_call_id = "call-collision";
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_a).await;
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_b).await;
    turn_runs::create_run(&pool, &run_a, &session_a, bear_id, user_id)
        .await
        .expect("create run a");
    turn_runs::create_run(&pool, &run_b, &session_b, bear_id, user_id)
        .await
        .expect("create run b");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_a,
        &session_a,
        tool_call_id,
        None,
        json!({ "session": "a" }),
    )
    .await
    .expect("insert session a obligation");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_b,
        &session_b,
        tool_call_id,
        None,
        json!({ "session": "b" }),
    )
    .await
    .expect("insert session b obligation");

    let wrong_session = rpc_value(
        test_state(pool.clone()),
        &token,
        "client.tool.result",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_b,
            "run_id": run_a,
            "tool_call_id": tool_call_id,
            "status": "ok",
            "content": "wrong session"
        }),
    )
    .await;
    let error = wrong_session["error"]["data"]["error"].as_str().unwrap();
    assert!(
        error.contains("run does not belong to authenticated Bear/session"),
        "{wrong_session}"
    );

    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "client.tool.result",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_a,
            "run_id": run_a,
            "tool_call_id": tool_call_id,
            "status": "ok",
            "content": "correct session"
        }),
    )
    .await;
    assert_eq!(response["result"]["ok"], false, "{response}");
    assert_eq!(
        response["result"]["status"], "continuation_unavailable",
        "{response}"
    );

    let obligation_a = turn_obligations::get_tool_call_obligation(&pool, &run_a, tool_call_id)
        .await
        .expect("load session a obligation")
        .expect("session a obligation exists");
    let obligation_b = turn_obligations::get_tool_call_obligation(&pool, &run_b, tool_call_id)
        .await
        .expect("load session b obligation")
        .expect("session b obligation exists");
    assert_eq!(obligation_a.state, "waiting_for_client");
    assert_eq!(obligation_b.state, "waiting_for_client");
}

#[sqlx::test(migrations = "../../migrations")]
async fn permission_decision_expiry_fails_run_with_retry_metadata(pool: sqlx::PgPool) {
    let state = test_state(pool.clone());
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let obligation = turn_obligations::upsert_permission_decision_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-timeout",
        Some("permission-timeout"),
        json!({ "tool_name": "fs_edit_file" }),
    )
    .await
    .expect("insert tool obligation");
    let request_id = Uuid::new_v4();
    let active_turn = state
        .tool_turns
        .acquire_active_turn(&session_id, request_id, None)
        .expect("register active session turn");
    sqlx::query(
        "UPDATE turn_obligations SET created_at = NOW() - INTERVAL '10 minutes' WHERE id = $1",
    )
    .bind(obligation.id)
    .execute(&pool)
    .await
    .expect("age obligation");

    let expired_runs = crate::expire_client_obligations_once(&state, 100)
        .await
        .expect("expire obligations");
    assert_eq!(expired_runs, 1);
    assert!(state
        .tool_turns
        .active_turn_for_session(&session_id)
        .is_none());
    // The active-turn guard may outlive cancellation, but must not restore it.
    drop(active_turn);
    let recovered_turn = state
        .tool_turns
        .acquire_active_turn(&session_id, Uuid::new_v4(), None)
        .expect("accept a new turn after expiry");
    drop(recovered_turn);

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "failed");
    assert_eq!(
        run.terminal_reason.as_deref(),
        Some("permission_decision_expired")
    );
    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    assert!(events.iter().any(|row| {
        row.event_type == "run.failed"
            && row.event.data["reason"] == "permission_decision_expired"
            && row.event.data["context"]["source"] == "bearwire_client_obligation_expiry_loop"
    }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn prior_process_obligation_is_reported_as_den_restart(pool: sqlx::PgPool) {
    let state = test_state(pool.clone());
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let prior_process_epoch_id = Uuid::new_v4();
    assert_ne!(prior_process_epoch_id, state.process_epoch_id);
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-restart",
        None,
        json!({
            "tool_name": "fs_list_directory",
            "den_process_epoch_id": prior_process_epoch_id,
        }),
    )
    .await
    .expect("insert prior-process obligation");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-restart-second",
        None,
        json!({
            "tool_name": "fs_search_files",
            "den_process_epoch_id": prior_process_epoch_id,
        }),
    )
    .await
    .expect("insert second prior-process obligation");

    assert_eq!(
        crate::expire_client_obligations_once(&state, 100)
            .await
            .expect("reconcile obligations"),
        1
    );

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "failed");
    assert_eq!(
        run.terminal_reason.as_deref(),
        Some("server_restart_interrupted")
    );
    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    let failed = events
        .iter()
        .find(|row| row.event_type == "run.failed")
        .expect("run.failed event");
    assert_eq!(failed.event.data["reason"], "server_restart_interrupted");
    assert_eq!(
        failed.event.data["context"]["source"],
        "bearwire_client_obligation_restart_reconciliation"
    );
    assert_eq!(
        failed.event.data["context"]["recovery"]["status"],
        "interrupted"
    );
    assert_eq!(failed.event.data["context"]["recovery"]["retryable"], true);
    assert_eq!(
        failed.event.data["context"]["recovery"]["next_action"],
        "send_message"
    );
    assert_eq!(failed.event.data["settled_obligations"], 2);
    assert!(
        turn_obligations::open_client_obligations_for_run(&pool, &run_id)
            .await
            .expect("list open obligations")
            .is_empty()
    );
    assert!(failed.event.data["user_message"]
        .as_str()
        .is_some_and(|message| message.contains("Den restarted")));
}

#[sqlx::test(migrations = "../../migrations")]
async fn command_obligation_expiry_blocks_automatic_retry_as_outcome_unknown(pool: sqlx::PgPool) {
    let state = test_state(pool.clone());
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let obligation = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-command-timeout",
        None,
        json!({
            "tool_name": "run_command",
            "den_process_epoch_id": state.process_epoch_id,
        }),
    )
    .await
    .expect("insert command obligation");
    turn_obligations::claim_tool_execution(
        &pool,
        obligation.id,
        &run_id,
        &session_id,
        "call-command-timeout",
        &turn_obligations::lease_attempt_token_hash("command-attempt"),
    )
    .await
    .expect("claim command obligation")
    .expect("command obligation was claimable");
    sqlx::query(
        "UPDATE turn_obligations SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(obligation.id)
    .execute(&pool)
    .await
    .expect("expire command lease");

    assert_eq!(
        crate::expire_client_obligations_once(&state, 100)
            .await
            .expect("expire obligations"),
        1
    );

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "failed");
    assert_eq!(
        run.terminal_reason.as_deref(),
        Some("command_outcome_unknown")
    );
    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    let failed = events
        .iter()
        .find(|row| row.event_type == "run.failed")
        .expect("run.failed event");
    assert_eq!(failed.event.data["reason"], "command_outcome_unknown");
    assert_eq!(
        failed.event.data["message"],
        "Connection failure: Builder Bear lost contact with the BearWire service or connected work surface before it could confirm whether the command completed. To avoid duplicate changes, the command was not retried automatically."
    );
    assert_eq!(
        failed.event.data["context"]["recovery"]["automatic_retry_allowed"],
        false
    );
    assert_eq!(
        failed.event.data["context"]["recovery"]["next_action"],
        "run_state"
    );
    assert_eq!(
        failed.event.data["context"]["recovery"]["next_action_params"]["run_id"],
        run_id
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn events_poll_does_not_expire_client_obligations(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let obligation = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-timeout",
        None,
        json!({ "tool_name": "fs_list_directory" }),
    )
    .await
    .expect("insert tool obligation");
    sqlx::query(
        "UPDATE turn_obligations SET created_at = NOW() - INTERVAL '10 minutes' WHERE id = $1",
    )
    .bind(obligation.id)
    .execute(&pool)
    .await
    .expect("age obligation");

    let replay = events_page(
        State(test_state(pool.clone())),
        bearer_headers(&token),
        Path(session_id.clone()),
        Query(EventPageQuery {
            bear_slug: bear_slug.clone(),
            after: None,
            limit: None,
        }),
    )
    .await
    .expect("events page response")
    .0;
    assert!(!replay.to_string().contains("client_obligation_timeout"));

    let obligation = turn_obligations::get_tool_call_obligation(&pool, &run_id, "call-timeout")
        .await
        .expect("load obligation")
        .expect("obligation exists");
    assert_eq!(obligation.state, "waiting_for_client");
    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "accepted");
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_cancel_settles_outstanding_obligations(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-cancelled",
        Some("perm-cancelled"),
        json!({ "test": "tool obligation" }),
    )
    .await
    .expect("insert tool obligation");
    turn_obligations::upsert_permission_decision_obligation(
        &pool,
        &run_id,
        &session_id,
        "perm-cancelled",
        Some("call-cancelled"),
        json!({ "test": "permission obligation" }),
    )
    .await
    .expect("insert permission obligation");

    let task_id = create_session_task(&pool, user_id, bear_id, &session_id, "Cancel task").await;
    let attempt = PgDocketService::from_pool(&pool)
        .acquire_focused_execution(DocketFocusedExecutionAcquire {
            bear_id,
            task_id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: session_id.clone(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id: run_id.clone(),
            },
            acquisition_key: Uuid::new_v4(),
        })
        .await
        .expect("acquire focused execution");

    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.cancel",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "run_id": run_id,
        }),
    )
    .await;
    assert_eq!(response["result"]["ok"], true, "{response}");
    assert_eq!(response["result"]["cancelled"], true, "{response}");
    assert_eq!(response["result"]["run_id"], run_id, "{response}");
    assert_eq!(response["result"]["settled_obligations"], 1, "{response}");

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list BearWire events");
    let cancelled = events
        .iter()
        .find(|row| row.event_type == "run.cancelled")
        .expect("run.cancelled event persisted");
    assert_eq!(cancelled.event.run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(cancelled.event.data["run_id"], run_id);
    assert_eq!(cancelled.event.data["reason"], "client_requested");

    let tool = turn_obligations::get_tool_call_obligation(&pool, &run_id, "call-cancelled")
        .await
        .expect("load tool obligation")
        .expect("tool obligation exists");
    let permission = turn_obligations::get_permission_obligation(&pool, &run_id, "perm-cancelled")
        .await
        .expect("load permission obligation")
        .expect("permission obligation exists");
    assert_eq!(tool.state, "cancelled");
    assert_eq!(permission.state, "cancelled");
    let attempt_state: String =
        sqlx::query_scalar("SELECT state FROM docket_execution_attempts WHERE id = $1")
            .bind(attempt.id)
            .fetch_one(&pool)
            .await
            .expect("load focused execution attempt");
    assert_eq!(attempt_state, "released");

    let successor_run_id = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(&pool, &successor_run_id, &session_id, bear_id, user_id)
        .await
        .expect("create successor run");
    let stale_cancel = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.cancel",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "run_id": run_id,
        }),
    )
    .await;
    assert_eq!(stale_cancel["result"]["cancelled"], false, "{stale_cancel}");
    assert_eq!(
        turn_runs::get_run(&pool, &successor_run_id)
            .await
            .expect("load successor")
            .expect("successor exists")
            .state,
        "accepted",
        "stale cancellation must not terminalize the successor"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn focused_pair_git_commit_creates_candidate_task_artifact(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id = create_session_task(&pool, user_id, bear_id, &session_id, "Commit task").await;
    // No work run is created here: persistence must use this focused Pair attempt.
    let attempt = PgDocketService::from_pool(&pool)
        .acquire_focused_execution(DocketFocusedExecutionAcquire {
            bear_id,
            task_id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: session_id.clone(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id,
            },
            acquisition_key: Uuid::new_v4(),
        })
        .await
        .expect("acquire focused execution");

    let sha = "0123456789abcdef0123456789abcdef01234567";
    crate::methods::client::persist_work_git_commit_artifact(
        &test_state(pool.clone()),
        bear_id,
        user_id,
        &session_id,
        Some("git_commit"),
        den_core::tools::result_compaction::ToolResultStatus::Ok,
        &json!({
            "ok": true,
            "repo_path": "/workspace/project",
            "sha": sha,
            "subject": "Persist commit evidence",
        }),
    )
    .await;

    let citations = artifacts::list_docket_artifact_citations(
        &pool,
        bear_id,
        DocketArtifactTargetKind::Task,
        task_id,
        ArtifactAccessContext {
            bear_id,
            user_id: Some(user_id),
            profile: BearProfile::Pair,
        },
    )
    .await
    .expect("list task artifacts");
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0].kind, "git_commit");
    assert_eq!(
        citations[0].summary.as_deref(),
        Some("Git commit 0123456789abcdef0123456789abcdef01234567")
    );

    let links = artifacts::list_artifact_links(&pool, bear_id, "docket_task", &task_id.to_string())
        .await
        .expect("list task artifact links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].metadata["candidate"], true);
    assert_eq!(links[0].metadata["work_run_id"], Value::Null);
    assert_eq!(
        links[0].metadata["execution_attempt_id"],
        attempt.id.to_string()
    );

    let refs = crate::methods::docket::resolve_candidate_git_commit_output(
        &test_state(pool.clone()),
        bear_id,
        task_id,
        Some(&session_id),
        Some(json!({
            "validation": {
                "command": "cargo test",
                "result": "passed",
                "execution_provenance": "bearwire client tool result",
            }
        })),
    )
    .await
    .expect("resolve linked commit output")
    .expect("linked commit output");
    assert_eq!(refs["primary_output"]["kind"], "git_commit");
    assert_eq!(
        refs["primary_output"]["artifact_ref"],
        citations[0].artifact_ref
    );
    assert_eq!(refs["primary_output"]["immutable_identity"], sha);
    assert_eq!(
        refs["validation"]["primary_output_ref"],
        citations[0].artifact_ref
    );
    assert_eq!(refs["validation"]["immutable_identity"], sha);
    assert_eq!(refs["validation"]["command"], "cargo test");
    assert_eq!(refs["validation"]["result"], "passed");

    let links = artifacts::list_artifact_links(&pool, bear_id, "docket_task", &task_id.to_string())
        .await
        .expect("list promoted task artifact links");
    assert!(links.iter().any(|link| link.role == "primary_output"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_focus_promotes_the_origin_run_idempotently(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_member_token(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id = create_session_task(
        &pool,
        user_id,
        bear_id,
        &session_id,
        "Promote the current turn run",
    )
    .await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    set_next_scripted_runtime_streams(&session_id, vec![ScriptedRuntimeStream::Pending]);
    let policy = den_core::EffectivePolicy::compile(
        den_core::TrustProfile::Pair,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Connected,
    );
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({ "bear_slug": bear_slug, "session_id": session_id, "task_id": task_id }),
    )
    .await;
    assert_eq!(selected["result"]["current_task_id"], task_id.to_string());

    let started = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "prompt": "Begin interactive work",
            "client": "bearwire-test",
            "supersede_active_run": true,
        }),
    )
    .await;
    let run_id = TurnRunId::new(
        started["result"]["run_id"]
            .as_str()
            .unwrap_or_else(|| panic!("run.start failed: {started}"))
            .to_string(),
    )
    .unwrap();
    wait_for_focused_run_started(
        state.clone(),
        &token,
        &bear_slug,
        &session_id,
        run_id.as_str(),
    )
    .await;
    let bear = bears_db::get_bear(&pool, bear_id)
        .await
        .expect("load Bear")
        .expect("Bear exists");
    let tool_call_id = ToolCallId::new("call-model-focus").unwrap();
    let chat_policy = den_core::EffectivePolicy::compile(
        den_core::TrustProfile::Chat,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Connected,
    );
    let denied = crate::methods::focused_execution::acquire_selected_task_for_run(
        &state,
        user_id,
        bear.clone(),
        &session_id,
        &run_id,
        &tool_call_id,
        &chat_policy.capabilities,
    )
    .await
    .expect_err("trust profile without focused-execution capability must be rejected");
    assert!(denied.to_string().contains("ExecuteFocusedTask"));

    let first = crate::methods::focused_execution::acquire_selected_task_for_run(
        &state,
        user_id,
        bear.clone(),
        &session_id,
        &run_id,
        &tool_call_id,
        &policy.capabilities,
    )
    .await
    .expect("promote origin run");
    let replay = crate::methods::focused_execution::acquire_selected_task_for_run(
        &state,
        user_id,
        bear.clone(),
        &session_id,
        &run_id,
        &tool_call_id,
        &policy.capabilities,
    )
    .await
    .expect("replay focus");
    assert_eq!(first.run_id(), Some(&run_id));
    assert_eq!(replay.run_id(), first.run_id());
    assert_eq!(replay.attempt_id(), first.attempt_id());
    assert_eq!(replay.fence_epoch(), first.fence_epoch());
    assert_eq!(
        replay.launch_state,
        crate::methods::focused_execution::FocusedExecutionLaunchState::AlreadyRunning
    );

    let foreign_owner = create_test_user(&pool).await;
    create_member_token(&pool, foreign_owner, bear_id).await;
    seed_docket_visibility_surface(&pool, bear_id, foreign_owner).await;
    let private = visibility_job(
        &pool,
        bear_id,
        foreign_owner,
        TaskListVisibility::SameUser,
        None,
    )
    .await;
    sqlx::query("UPDATE client_sessions SET current_task_id = $2 WHERE client_session_id = $1")
        .bind(&session_id)
        .bind(private.tasks[0].id)
        .execute(&pool)
        .await
        .expect("simulate forged selected task");
    let denied_foreign = crate::methods::focused_execution::acquire_selected_task_for_run(
        &state,
        user_id,
        bear.clone(),
        &session_id,
        &run_id,
        &ToolCallId::new("call-foreign-model-focus").unwrap(),
        &policy.capabilities,
    )
    .await
    .expect_err("model focus must reject a foreign selected task before acquisition");
    assert!(matches!(
        denied_foreign,
        den_http::errors::CustomError::NotFound(_)
    ));
    assert!(!denied_foreign.to_string().contains("Private task"));
    let foreign_attempts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM docket_execution_attempts WHERE task_id = $1")
            .bind(private.tasks[0].id)
            .fetch_one(&pool)
            .await
            .expect("count foreign model-focus attempts");
    assert_eq!(foreign_attempts, 0);
    sqlx::query("UPDATE client_sessions SET current_task_id = $2 WHERE client_session_id = $1")
        .bind(&session_id)
        .bind(task_id)
        .execute(&pool)
        .await
        .expect("restore owned selection");

    let run_count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM turn_runs WHERE session_id = $1"#,
        session_id,
    )
    .fetch_one(&pool)
    .await
    .expect("count Pair runs");
    assert_eq!(run_count, 1, "model focus must not create a successor run");
    let attempt_count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM docket_execution_attempts WHERE binding_kind = 'client_session' AND binding_id = $1"#,
        session_id,
    )
    .fetch_one(&pool)
    .await
    .expect("count Pair attempts");
    assert_eq!(attempt_count, 1, "focus replay must reuse one attempt");
    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 50)
        .await
        .expect("list focus events");
    let transitions = events
        .iter()
        .filter(|event| {
            event.event_type
                == bearwire_protocol::lifecycle::FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transitions.len(),
        1,
        "focus replay must not duplicate focused-execution transitions"
    );
    let transition: bearwire_protocol::lifecycle::FocusedExecutionTransition =
        serde_json::from_value(transitions[0].event.data.clone())
            .expect("decode focused-execution transition");
    assert_eq!(transition.state_version, 1);
    assert_eq!(transition.from, None);
    assert_eq!(
        transition.reason,
        bearwire_protocol::lifecycle::FocusedExecutionTransitionReason::FocusAcquired
    );
    assert_eq!(
        transition.to,
        bearwire_protocol::lifecycle::FocusedExecutionState::Running
    );
    assert_eq!(
        transitions[0].event.scope,
        bearwire_protocol::wire::BearWireEventScope::Persistent
    );
    assert!(events.iter().all(|event| {
        !matches!(
            event.event_type.as_str(),
            "run.recovering" | "run.recovered" | "run.completed" | "run.failed" | "run.cancelled"
        )
    }));

    state
        .turn_cancellations
        .cancel_run(&session_id, run_id.as_str())
        .expect("cancel live origin controller");
    for _ in 0..50 {
        if state
            .turn_cancellations
            .active_for_run(&session_id, run_id.as_str())
            .is_none()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let error = crate::methods::focused_execution::acquire_selected_task_for_run(
        &state,
        user_id,
        bear,
        &session_id,
        &run_id,
        &ToolCallId::new("call-after-controller-loss").unwrap(),
        &policy.capabilities,
    )
    .await
    .expect_err("model focus must require the canonical origin controller");
    assert!(error.to_string().contains("no live controller"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_start_recovers_orphaned_controller_without_execution_authority(
    pool: sqlx::PgPool,
) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id = create_session_task(&pool, user_id, bear_id, &session_id, "Recover task").await;

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config.clone());

    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({ "bear_slug": bear_slug, "session_id": session_id, "task_id": task_id }),
    )
    .await;
    assert_eq!(selected["result"]["current_task_id"], task_id.to_string());
    let first = rpc_value(
        state,
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(first["result"]["claimed"], true, "{first}");
    let first_run_id = first["result"]["run_id"].as_str().unwrap().to_string();
    wait_for_focused_run_started(
        test_state_with_config(pool.clone(), config.clone()),
        &token,
        &bear_slug,
        &session_id,
        &first_run_id,
    )
    .await;
    let first_attempt_id = first["result"]["execution_attempt_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Simulate a lost execution lease: the host run remains active, but its
    // canonical Docket authority has already been released.
    PgDocketService::from_pool(&pool)
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: Uuid::parse_str(&first_attempt_id).expect("attempt UUID"),
            fence_epoch: first["result"]["fence_epoch"]
                .as_i64()
                .expect("fence epoch"),
            recovery_key: Uuid::new_v4(),
            recovery_reason: "test_lost_execution_authority".to_string(),
        })
        .await
        .expect("release execution authority");

    // A rebuilt service has durable run state but no in-memory controller registry.
    let recovered = rpc_value(
        test_state_with_config(pool.clone(), config),
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(recovered["result"]["claimed"], true, "{recovered}");
    assert_ne!(recovered["result"]["run_id"], first_run_id);
    assert_ne!(
        recovered["result"]["execution_attempt_id"],
        first_attempt_id
    );

    let old_run: (String, Option<String>) =
        sqlx::query_as("SELECT state, terminal_reason FROM turn_runs WHERE run_id = $1")
            .bind(&first_run_id)
            .fetch_one(&pool)
            .await
            .expect("load recovered run");
    assert_eq!(old_run.0, "failed");
    assert_eq!(old_run.1.as_deref(), Some("orphaned_execution_controller"));
    let old_attempt_state: String =
        sqlx::query_scalar("SELECT state FROM docket_execution_attempts WHERE id = $1::uuid")
            .bind(&first_attempt_id)
            .fetch_one(&pool)
            .await
            .expect("load released attempt");
    assert_eq!(old_attempt_state, "released");
    let selected_task: Option<Uuid> = sqlx::query_scalar(
        "SELECT current_task_id FROM client_sessions WHERE client_session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load preserved task selection");
    assert_eq!(selected_task, Some(task_id));

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 50)
        .await
        .expect("list recovery events");
    let failed = events
        .iter()
        .find(|row| {
            row.event_type == "run.failed"
                && row.event.run_id.as_deref() == Some(first_run_id.as_str())
        })
        .expect("old host run reaches a typed failed terminal boundary");
    assert_eq!(failed.event.data["recovery"], "replacement_pending");
    assert_eq!(failed.event.data["task_id"], task_id.to_string());
    assert_eq!(failed.event.data["task_selection_preserved"], true);
    let failed_transition = events
        .iter()
        .filter(|row| {
            row.event_type == bearwire_protocol::lifecycle::FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE
        })
        .filter_map(|row| {
            serde_json::from_value::<bearwire_protocol::lifecycle::FocusedExecutionTransition>(
                row.event.data.clone(),
            )
            .ok()
        })
        .find(|transition| {
            transition.run_id.as_deref() == Some(first_run_id.as_str())
                && transition.reason
                    == bearwire_protocol::lifecycle::FocusedExecutionTransitionReason::OrphanedControllerReconciled
        })
        .expect("orphan recovery records the failed focused-execution transition");
    assert_eq!(
        failed_transition.to,
        bearwire_protocol::lifecycle::FocusedExecutionState::Terminal
    );
    let recovered_event = events
        .iter()
        .find(|row| {
            row.event_type == "run.recovered"
                && row.event.run_id.as_deref() == Some(first_run_id.as_str())
        })
        .expect("recovery projects the replacement host run");
    assert_eq!(recovered_event.event.data["run_id"], first_run_id);
    assert_eq!(
        recovered_event.event.data["replacement_run_id"],
        recovered["result"]["run_id"]
    );
    assert_eq!(recovered_event.event.data["task_selection_preserved"], true);
    assert!(events.iter().all(|row| {
        row.event_type != "run.recovering"
            || row.event.run_id.as_deref() != Some(first_run_id.as_str())
    }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_start_releases_orphaned_foreign_task_authority(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let foreign_session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &foreign_session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Recover foreign task").await;
    let foreign_attempt = PgDocketService::from_pool(&pool)
        .authorize_execution_attempt(DocketExecutionAttemptAuthorize {
            bear_id,
            task_id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: foreign_session_id,
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id: format!("run_{}", Uuid::new_v4().simple()),
            },
            authorization_key: Uuid::new_v4(),
        })
        .await
        .expect("authorize orphaned foreign authority");

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url =
        start_mock_openai_sse_server_asserting_requests(vec![MockLlmRequestAssertion::requiring(
            Vec::new(),
        )]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);

    rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({ "bear_slug": bear_slug, "session_id": session_id, "task_id": task_id }),
    )
    .await;
    let started = rpc_value(
        state,
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(started["result"]["claimed"], true, "{started}");
    assert_ne!(
        started["result"]["execution_attempt_id"],
        foreign_attempt.id.to_string()
    );
    let foreign_state: String =
        sqlx::query_scalar("SELECT state FROM docket_execution_attempts WHERE id = $1")
            .bind(foreign_attempt.id)
            .fetch_one(&pool)
            .await
            .expect("load orphaned foreign attempt");
    assert_eq!(foreign_state, "released");
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_start_releases_stale_session_authority_for_previous_task(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let stale_task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Stale task").await;
    let selected_task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Selected task").await;
    let stale_run_id = format!("run_{}", Uuid::new_v4().simple());
    let stale_attempt = PgDocketService::from_pool(&pool)
        .authorize_execution_attempt(DocketExecutionAttemptAuthorize {
            bear_id,
            task_id: stale_task_id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: session_id.clone(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id: stale_run_id,
            },
            authorization_key: Uuid::new_v4(),
        })
        .await
        .expect("authorize stale execution authority");

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url =
        start_mock_openai_sse_server_asserting_requests(vec![MockLlmRequestAssertion::requiring(
            Vec::new(),
        )]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);

    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "task_id": selected_task_id,
        }),
    )
    .await;
    assert_eq!(
        selected["result"]["current_task_id"],
        selected_task_id.to_string()
    );

    let started = rpc_value(
        state,
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(started["result"]["claimed"], true, "{started}");
    assert_eq!(started["result"]["task_id"], selected_task_id.to_string());
    assert_ne!(
        started["result"]["execution_attempt_id"],
        stale_attempt.id.to_string()
    );
    let stale_state: String =
        sqlx::query_scalar("SELECT state FROM docket_execution_attempts WHERE id = $1")
            .bind(stale_attempt.id)
            .fetch_one(&pool)
            .await
            .expect("load stale attempt");
    assert_eq!(stale_state, "released");
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_start_requires_selection_and_reuses_active_run(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_body(Vec::new());
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);

    let missing_selection = rpc_value(
        state.clone(),
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert!(
        missing_selection.get("error").is_some(),
        "{missing_selection}"
    );

    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Start Pair task").await;
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "task_id": task_id,
        }),
    )
    .await;
    assert_eq!(selected["result"]["current_task_id"], task_id.to_string());

    let first = rpc_value(
        state.clone(),
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(first["result"]["started"], false, "{first}");
    assert_eq!(first["result"]["claimed"], true, "{first}");
    assert_eq!(first["result"]["reused"], false, "{first}");
    assert_eq!(
        first["result"]["execution_attempt_state"], "authorized",
        "claimed starts must not advertise running authority: {first}"
    );
    assert_eq!(
        first["result"]["launch_state"], "claimed",
        "new starts report controller claim before native startup: {first}"
    );
    let execution_attempt_id = first["result"]["execution_attempt_id"]
        .as_str()
        .expect("Pair start returns canonical execution attempt id");
    assert!(
        first["result"]["fence_epoch"].as_i64().is_some(),
        "focused start returns canonical attempt fence: {first}"
    );
    let first_snapshot = &first["result"]["focused_execution"];
    assert_eq!(first_snapshot["state"]["phase"], "starting", "{first}");
    assert_eq!(first_snapshot["controller"], "claimed", "{first}");
    assert_eq!(first_snapshot["task"]["id"], task_id.to_string());
    assert_eq!(first_snapshot["run"]["id"], first["result"]["run_id"]);
    assert_eq!(
        first_snapshot["attempt"]["id"],
        first["result"]["execution_attempt_id"]
    );
    assert_eq!(first_snapshot["obligations"]["open"], 0);
    let first_run_id = first["result"]["run_id"].as_str().expect("claimed run id");
    wait_for_focused_run_started(state.clone(), &token, &bear_slug, &session_id, first_run_id)
        .await;

    let attempt: (String, String, String, String) = sqlx::query_as(
        "SELECT id::TEXT, binding_kind, binding_id, host_run_id
         FROM docket_execution_attempts WHERE id = $1::uuid",
    )
    .bind(execution_attempt_id)
    .fetch_one(&pool)
    .await
    .expect("Pair start persists canonical execution attempt");
    assert_eq!(attempt.1, "client_session");
    assert_eq!(attempt.2, session_id);
    assert_eq!(attempt.3, first["result"]["run_id"].as_str().unwrap());

    let second = rpc_value(
        state.clone(),
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(second["result"]["started"], false, "{second}");
    assert_eq!(second["result"]["reused"], true, "{second}");
    assert_eq!(
        second["result"]["execution_attempt_state"], "running",
        "reused starts must return canonical attempt state: {second}"
    );
    assert_eq!(
        second["result"]["launch_state"], "already_running",
        "reused starts must report a live native run: {second}"
    );
    assert_eq!(second["result"]["run_id"], first["result"]["run_id"]);
    assert_eq!(
        second["result"]["execution_attempt_id"], first["result"]["execution_attempt_id"],
        "active focused run must retain its attempt capability"
    );
    let session_state = rpc_value(
        state.clone(),
        &token,
        "session.state",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(
        session_state["result"]["session"]["diagnostics"]["focused_execution"],
        second["result"]["focused_execution"],
        "focus/start and session.state must share one canonical projection"
    );
    assert!(
        session_state["result"]["session"]["diagnostics"]
            .get("active_docket_execution")
            .is_none(),
        "session.state must not retain an independently writable execution projection"
    );
    let execution_diagnostics = rpc_value(
        state,
        &token,
        "session.execution.diagnostics",
        json!({ "bear_slug": bear_slug, "session_id": session_id, "limit": 16 }),
    )
    .await;
    let diagnostics = &execution_diagnostics["result"]["diagnostics"];
    assert_eq!(
        diagnostics["snapshot"], second["result"]["focused_execution"],
        "operator diagnostics must use the canonical focused snapshot"
    );
    assert_eq!(diagnostics["version_gap"], false);
    assert_eq!(diagnostics["snapshot_matches_latest_transition"], true);
    assert_eq!(diagnostics["reason_counts"]["authority_claimed"], 1);
    assert_eq!(diagnostics["reason_counts"]["authority_started"], 1);
    assert_eq!(
        diagnostics["transitions"].as_array().map(Vec::len),
        Some(2),
        "one claimed and one started transition should explain the active focus"
    );

    let docket_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bear_jobs WHERE bear_id = $1")
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("count docket jobs");
    let work_runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM bear_work_runs WHERE bear_id = $1")
            .bind(bear_id)
            .fetch_one(&pool)
            .await
            .expect("count work runs");
    assert_eq!(docket_jobs, 0, "Pair selection/start must not create a Job");
    assert_eq!(
        work_runs, 0,
        "Pair selection/start must not create a Work run"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkout_rejection_projects_non_dispatchable_gate(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    let (job_id, job_run_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT job_id, job_run_id FROM bear_work_runs WHERE id = $1")
            .bind(work_run_id)
            .fetch_one(&pool)
            .await
            .expect("load work run job");
    sqlx::query(
        "INSERT INTO bear_task_run_state (run_id, task_id, status)
         SELECT $1, id, 'done' FROM bear_tasks WHERE job_id = $2
         ON CONFLICT (run_id, task_id) DO UPDATE SET status = 'done'",
    )
    .bind(job_run_id)
    .bind(job_id)
    .execute(&pool)
    .await
    .expect("settle work tasks");

    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": format!("work-{}", Uuid::new_v4().simple()),
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    let result = &response["result"];
    assert_eq!(result["ok"], false, "{response}");
    assert_eq!(result["permission_mode"], "none", "{response}");
    assert_eq!(result["gate"]["status"], "rejected", "{response}");
    assert_eq!(result["gate"]["disposition"], "stop", "{response}");
    assert_eq!(result["prompt"], "", "{response}");
    assert!(result["task_title"].is_null(), "{response}");

    let repeated = rpc_value(
        test_state(pool.clone()),
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": format!("work-{}", Uuid::new_v4().simple()),
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    let repeated_result = &repeated["result"];
    assert_eq!(repeated_result["ok"], false, "{repeated}");
    assert_eq!(
        repeated_result["gate"]["disposition"], "require_intervention",
        "{repeated}"
    );
    assert_eq!(repeated_result["permission_mode"], "none", "{repeated}");
    assert_eq!(repeated_result["prompt"], "", "{repeated}");
    assert!(repeated_result["task_title"].is_null(), "{repeated}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn configured_hats_reject_legacy_work_checkout_before_session_or_attempt_binding(
    pool: sqlx::PgPool,
) {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats;
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user_id),
        "Work review",
        "New runs need an eligible hat",
    )
    .await
    .unwrap();
    let denied = rpc_value(
        test_state(pool.clone()),
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": format!("work-{}", Uuid::new_v4().simple()),
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
    let bound_session = sqlx::query_scalar!(
        "SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1",
        work_run_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(bound_session.is_none());
    let attempts = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM docket_execution_attempts
         WHERE binding_kind = 'work_assignment' AND binding_id = $1",
        work_run_id.to_string(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkout_returns_a_stable_canonical_attempt(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    let state = test_state(pool.clone());
    let session_id = format!("work-{}", Uuid::new_v4().simple());

    let checkout = rpc_value(
        state.clone(),
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert_eq!(checkout["result"]["ok"], true, "{checkout}");
    assert!(
        checkout["result"]["execution_attempt_id"]
            .as_str()
            .is_some(),
        "work checkout returns an attempt identity: {checkout}"
    );
    assert!(
        checkout["result"]["execution_attempt_fence_epoch"]
            .as_i64()
            .is_some(),
        "work checkout returns an attempt fence: {checkout}"
    );

    let replay = rpc_value(
        state,
        &token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "work_order_id": work_run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert_eq!(replay["result"]["ok"], true, "{replay}");
    assert_eq!(
        replay["result"]["execution_attempt_id"], checkout["result"]["execution_attempt_id"],
        "re-checkout must replay the same Work attempt"
    );
    assert_eq!(
        replay["result"]["execution_attempt_fence_epoch"],
        checkout["result"]["execution_attempt_fence_epoch"],
        "re-checkout must retain the Work attempt fence"
    );

    let attempt_id: Uuid = checkout["result"]["execution_attempt_id"]
        .as_str()
        .expect("checkout returns attempt id")
        .parse()
        .expect("attempt id is UUID");
    let fence_epoch = checkout["result"]["execution_attempt_fence_epoch"]
        .as_i64()
        .expect("checkout returns fence epoch");
    let boundary = rpc_value(
        test_state(pool.clone()),
        &token,
        "work.boundary",
        json!({ "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch, "boundary_key": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(boundary["result"]["ok"], true, "{boundary}");

    let stale = rpc_value(
        test_state(pool),
        &token,
        "work.boundary",
        json!({ "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch + 1, "boundary_key": Uuid::new_v4() }),
    )
    .await;
    assert!(stale.get("error").is_some(), "{stale}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_methods_deny_other_members_same_user_job_without_binding_or_mutation(
    pool: sqlx::PgPool,
) {
    let owner_id = create_test_user(&pool).await;
    let other_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner_id, bear_id).await;
    let other_token = create_member_token(&pool, other_id, bear_id).await;
    let admin_id = create_test_user(&pool).await;
    let admin_token = create_token_for_bear(&pool, admin_id, bear_id).await;
    let run_id = create_checkoutable_work_run(&pool, owner_id, bear_id).await;
    let state = test_state(pool.clone());
    let session_id = format!("work-{}", Uuid::new_v4().simple());

    let denied = rpc_value(
        state.clone(),
        &other_token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "work_order_id": run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert!(denied.get("error").is_some(), "{denied}");
    let bound: Option<String> =
        sqlx::query_scalar("SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("load run binding");
    assert_eq!(bound, None, "failed checkout must not bind a session");
    let attempts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM docket_execution_attempts WHERE binding_id = $1")
            .bind(run_id.to_string())
            .fetch_one(&pool)
            .await
            .expect("count attempts");
    assert_eq!(attempts, 0, "failed checkout must not create an attempt");

    let owner = rpc_value(
        state.clone(),
        &owner_token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "work_order_id": run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert_eq!(owner["result"]["ok"], true, "{owner}");
    let attempt_id: Uuid = owner["result"]["execution_attempt_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let fence = owner["result"]["execution_attempt_fence_epoch"]
        .as_i64()
        .unwrap();
    let guessed_checkout = rpc_value(
        state.clone(),
        &other_token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug, "session_id": format!("guess-{}", Uuid::new_v4().simple()),
            "work_order_id": run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert!(
        guessed_checkout.get("error").is_some(),
        "{guessed_checkout}"
    );
    let denied_boundary = rpc_value(
        state.clone(),
        &other_token,
        "work.boundary",
        json!({
            "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence,
            "boundary_key": Uuid::new_v4(), "signal": "excessive_exploration",
        }),
    )
    .await;
    assert!(denied_boundary.get("error").is_some(), "{denied_boundary}");
    let directives: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docket_checkpoint_directives WHERE execution_attempt_id = $1",
    )
    .bind(attempt_id)
    .fetch_one(&pool)
    .await
    .expect("count directives");
    assert_eq!(
        directives, 0,
        "unauthorized boundary must not create a directive"
    );

    let boundary = rpc_value(
        state.clone(),
        &owner_token,
        "work.boundary",
        json!({
            "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence,
            "boundary_key": Uuid::new_v4(), "signal": "excessive_exploration",
        }),
    )
    .await;
    assert_eq!(
        boundary["result"]["gate"]["disposition"], "require_checkpoint",
        "{boundary}"
    );
    let directive_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM docket_checkpoint_directives WHERE execution_attempt_id = $1",
    )
    .bind(attempt_id)
    .fetch_one(&pool)
    .await
    .expect("checkpoint directive");
    for method in ["work.checkpoint_evidence", "work.acknowledge_checkpoint"] {
        let denied = rpc_value(
            state.clone(),
            &other_token,
            method,
            json!({
                "bear_slug": bear_slug, "directive_id": directive_id,
                "execution_attempt_id": attempt_id, "fence_epoch": fence,
                "summary": "unauthorized evidence", "checkpoint_artifact_ref": "untrusted",
            }),
        )
        .await;
        assert!(denied.get("error").is_some(), "{method}: {denied}");
    }
    let directive_state: String =
        sqlx::query_scalar("SELECT state FROM docket_checkpoint_directives WHERE id = $1")
            .bind(directive_id)
            .fetch_one(&pool)
            .await
            .expect("directive state");
    assert_eq!(directive_state, "pending");
    let artifacts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artifacts WHERE bear_id = $1 AND kind = 'runtime_checkpoint'",
    )
    .bind(bear_id)
    .fetch_one(&pool)
    .await
    .expect("count evidence artifacts");
    assert_eq!(artifacts, 0);

    for (token, reported_session) in [
        (&other_token, session_id.as_str()),
        (&owner_token, "unbound-session"),
    ] {
        let denied = rpc_value(
            state.clone(),
            token,
            "work.report",
            json!({
                "bear_slug": bear_slug, "work_order_id": run_id, "session_id": reported_session,
                "summary": "forged report",
            }),
        )
        .await;
        assert!(denied.get("error").is_some(), "{denied}");
    }
    let report: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT result_refs -> 'armature_report' FROM bear_work_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("report unchanged");
    assert_eq!(report, None);
    let legitimate = rpc_value(
        state.clone(),
        &owner_token,
        "work.report",
        json!({
            "bear_slug": bear_slug, "work_order_id": run_id, "session_id": session_id,
            "summary": "owner report",
        }),
    )
    .await;
    assert_eq!(legitimate["result"]["ok"], true, "{legitimate}");

    let admin_run = create_checkoutable_work_run(&pool, owner_id, bear_id).await;
    upsert_test_session(&pool, owner_id, bear_id, &bear_slug, &session_id).await;
    let denied_admin_session = rpc_value(
        state.clone(),
        &admin_token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
            "work_order_id": admin_run,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert!(
        denied_admin_session.get("error").is_some(),
        "{denied_admin_session}"
    );
    let still_unbound: Option<String> =
        sqlx::query_scalar("SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1")
            .bind(admin_run)
            .fetch_one(&pool)
            .await
            .expect("admin run binding");
    assert_eq!(still_unbound, None);
    let admin_checkout = rpc_value(
        state,
        &admin_token,
        "work.checkout",
        json!({
            "bear_slug": bear_slug, "session_id": format!("admin-{}", Uuid::new_v4().simple()),
            "work_order_id": admin_run,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        }),
    )
    .await;
    assert_eq!(admin_checkout["result"]["ok"], true, "{admin_checkout}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkout_requires_dispatch_authority_and_session_provenance(pool: sqlx::PgPool) {
    let owner_id = create_test_user(&pool).await;
    let member_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let owner_token = create_token_for_bear(&pool, owner_id, bear_id).await;
    let member_token = create_member_token(&pool, member_id, bear_id).await;
    let run_id = create_checkoutable_work_run(&pool, owner_id, bear_id).await;
    let job_id: Uuid = sqlx::query_scalar("SELECT job_id FROM bear_work_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("job id");
    sqlx::query("UPDATE bear_jobs SET visibility = $2 WHERE id = $1")
        .bind(job_id)
        .bind(TaskListVisibility::BearVisible.as_str())
        .execute(&pool)
        .await
        .expect("share job");
    let state = test_state(pool.clone());
    let session_id = format!("work-{}", Uuid::new_v4().simple());
    let checkout = |session_id: &str| {
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "work_order_id": run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        })
    };
    let denied = rpc_value(
        state.clone(),
        &member_token,
        "work.checkout",
        checkout(&session_id),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "visible job ID alone is not dispatch authority: {denied}"
    );
    let bound: Option<String> =
        sqlx::query_scalar("SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("binding");
    assert_eq!(bound, None);

    // A run-specific token must not override another human's persisted session.
    let dispatched = armature_tokens::create_for_bear(&pool, member_id, bear_id, "test dispatch")
        .await
        .expect("mint run token");
    den_docket::work_runs::merge_work_run_result_refs(
        &pool,
        run_id,
        &json!({
            "armature_token_id": dispatched.id,
        }),
    )
    .await
    .expect("record dispatch token");
    let member_session = format!("work-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, member_id, bear_id, &bear_slug, &member_session).await;
    let denied_owner_session = rpc_value(
        state.clone(),
        &owner_token,
        "work.checkout",
        checkout(&member_session),
    )
    .await;
    assert!(
        denied_owner_session.get("error").is_some(),
        "{denied_owner_session}"
    );
    upsert_test_session(&pool, owner_id, bear_id, &bear_slug, &session_id).await;
    let denied = rpc_value(
        state.clone(),
        &dispatched.raw_token,
        "work.checkout",
        checkout(&session_id),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "cannot take another human's session: {denied}"
    );
    let bound: Option<String> =
        sqlx::query_scalar("SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("binding");
    assert_eq!(bound, None);
    let denied_owner = rpc_value(
        state.clone(),
        &owner_token,
        "work.checkout",
        checkout(&session_id),
    )
    .await;
    assert_eq!(
        denied_owner["result"]["ok"], true,
        "owner may use their session: {denied_owner}"
    );

    let denied = rpc_value(
        state.clone(),
        &dispatched.raw_token,
        "work.checkout",
        checkout(&session_id),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "dispatcher cannot replay another human's session: {denied}"
    );
    let owner_dispatch =
        armature_tokens::create_for_bear(&pool, owner_id, bear_id, "owner dispatch")
            .await
            .expect("mint owner run token");
    den_docket::work_runs::merge_work_run_result_refs(
        &pool,
        run_id,
        &json!({ "armature_token_id": owner_dispatch.id }),
    )
    .await
    .expect("record owner dispatch token");
    let owner_replay = rpc_value(
        state.clone(),
        &owner_dispatch.raw_token,
        "work.checkout",
        checkout(&session_id),
    )
    .await;
    assert_eq!(owner_replay["result"]["ok"], true, "{owner_replay}");
    assert_eq!(
        owner_replay["result"]["execution_attempt_id"],
        denied_owner["result"]["execution_attempt_id"]
    );
    let attempt_id = denied_owner["result"]["execution_attempt_id"].clone();
    let fence = denied_owner["result"]["execution_attempt_fence_epoch"].clone();
    let denied = rpc_value(
        state.clone(),
        &member_token,
        "work.boundary",
        json!({
            "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence,
            "boundary_key": Uuid::new_v4(),
        }),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "generic member token cannot operate shared attempt: {denied}"
    );
    let allowed_boundary = rpc_value(
        state,
        &owner_token,
        "work.boundary",
        json!({
            "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence,
            "boundary_key": Uuid::new_v4(),
        }),
    )
    .await;
    assert_eq!(allowed_boundary["result"]["ok"], true, "{allowed_boundary}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_work_run_requires_its_exact_dispatch_token_for_noncreator(pool: sqlx::PgPool) {
    let owner_id = create_test_user(&pool).await;
    let dispatcher_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    create_member_token(&pool, owner_id, bear_id).await;
    let generic_token = create_member_token(&pool, dispatcher_id, bear_id).await;
    let shared_run = create_checkoutable_work_run(&pool, owner_id, bear_id).await;
    let private_run = create_checkoutable_work_run(&pool, owner_id, bear_id).await;
    let shared_job: Uuid = sqlx::query_scalar("SELECT job_id FROM bear_work_runs WHERE id = $1")
        .bind(shared_run)
        .fetch_one(&pool)
        .await
        .expect("shared job id");
    sqlx::query("UPDATE bear_jobs SET visibility = $2 WHERE id = $1")
        .bind(shared_job)
        .bind(TaskListVisibility::BearVisible.as_str())
        .execute(&pool)
        .await
        .expect("share job");
    let shared_token =
        armature_tokens::create_for_bear(&pool, dispatcher_id, bear_id, "shared dispatch")
            .await
            .expect("mint shared run token");
    let private_token =
        armature_tokens::create_for_bear(&pool, dispatcher_id, bear_id, "other dispatch")
            .await
            .expect("mint other run token");
    let session_id = format!("work-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, dispatcher_id, bear_id, &bear_slug, &session_id).await;
    let state = test_state(pool.clone());
    let checkout = |run_id| {
        json!({
            "bear_slug": bear_slug, "session_id": session_id, "work_order_id": run_id,
            "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] },
        })
    };
    let missing_ref = rpc_value(
        state.clone(),
        &shared_token.raw_token,
        "work.checkout",
        checkout(shared_run),
    )
    .await;
    assert!(missing_ref.get("error").is_some(), "{missing_ref}");
    for (run_id, token_id) in [
        (shared_run, shared_token.id),
        (private_run, private_token.id),
    ] {
        den_docket::work_runs::merge_work_run_result_refs(
            &pool,
            run_id,
            &json!({ "armature_token_id": token_id }),
        )
        .await
        .expect("store run-specific dispatch token");
    }
    for (token, run_id) in [
        (&generic_token, shared_run),
        (&private_token.raw_token, shared_run),
        (&private_token.raw_token, private_run),
        (&shared_token.raw_token, private_run),
    ] {
        let denied = rpc_value(state.clone(), token, "work.checkout", checkout(run_id)).await;
        assert!(denied.get("error").is_some(), "{denied}");
    }
    for run_id in [shared_run, private_run] {
        let bound: Option<String> =
            sqlx::query_scalar("SELECT bearwire_session_id FROM bear_work_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&pool)
                .await
                .expect("denied checkout leaves run unbound");
        let attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM docket_execution_attempts WHERE binding_id = $1",
        )
        .bind(run_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("denied checkout creates no attempt");
        assert_eq!(bound, None);
        assert_eq!(attempts, 0);
    }

    let allowed = rpc_value(
        state.clone(),
        &shared_token.raw_token,
        "work.checkout",
        checkout(shared_run),
    )
    .await;
    assert_eq!(allowed["result"]["ok"], true, "{allowed}");
    let attempt_id = allowed["result"]["execution_attempt_id"].clone();
    let fence = allowed["result"]["execution_attempt_fence_epoch"].clone();
    for token in [&generic_token, &private_token.raw_token] {
        let denied_boundary = rpc_value(
            state.clone(),
            token,
            "work.boundary",
            json!({
                "bear_slug": bear_slug, "execution_attempt_id": attempt_id,
                "fence_epoch": fence, "boundary_key": Uuid::new_v4(),
            }),
        )
        .await;
        assert!(denied_boundary.get("error").is_some(), "{denied_boundary}");
        let denied_report = rpc_value(
            state.clone(),
            token,
            "work.report",
            json!({
                "bear_slug": bear_slug, "work_order_id": shared_run,
                "session_id": session_id, "summary": "forged report",
            }),
        )
        .await;
        assert!(denied_report.get("error").is_some(), "{denied_report}");
    }
    let report: Option<Value> = sqlx::query_scalar(
        "SELECT result_refs -> 'armature_report' FROM bear_work_runs WHERE id = $1",
    )
    .bind(shared_run)
    .fetch_one(&pool)
    .await
    .expect("no forged report");
    assert_eq!(report, None);
    let boundary = rpc_value(
        state.clone(),
        &shared_token.raw_token,
        "work.boundary",
        json!({
            "bear_slug": bear_slug, "execution_attempt_id": attempt_id,
            "fence_epoch": fence, "boundary_key": Uuid::new_v4(),
        }),
    )
    .await;
    assert_eq!(boundary["result"]["ok"], true, "{boundary}");
    let report = rpc_value(
        state,
        &shared_token.raw_token,
        "work.report",
        json!({
            "bear_slug": bear_slug, "work_order_id": shared_run,
            "session_id": session_id, "summary": "dispatched report",
        }),
    )
    .await;
    assert_eq!(report["result"]["ok"], true, "{report}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkpoint_acknowledgement_unblocks_a_fresh_checkout(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    let state = test_state(pool.clone());
    let first = rpc_value(
        state.clone(),
        &token,
        "work.checkout",
        json!({ "bear_slug": bear_slug, "session_id": format!("work-{}", Uuid::new_v4().simple()), "work_order_id": work_run_id, "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] } }),
    ).await;
    let attempt_id: Uuid = first["result"]["execution_attempt_id"]
        .as_str()
        .expect("checkout returns attempt id")
        .parse()
        .expect("attempt id is UUID");
    let fence_epoch = first["result"]["execution_attempt_fence_epoch"]
        .as_i64()
        .expect("checkout returns fence epoch");
    let boundary = rpc_value(
        state.clone(), &token, "work.boundary",
        json!({ "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch, "boundary_key": Uuid::new_v4(), "signal": "excessive_exploration" }),
    ).await;
    let directive_id: Uuid = sqlx::query_scalar("SELECT id FROM docket_checkpoint_directives WHERE execution_attempt_id = $1 AND fence_epoch = $2")
        .bind(attempt_id).bind(fence_epoch).fetch_one(&pool).await.expect("boundary signal creates checkpoint directive");
    assert_eq!(boundary["result"]["ok"], false, "{boundary}");
    assert_eq!(
        boundary["result"]["gate"]["disposition"], "require_checkpoint",
        "{boundary}"
    );

    let denied = rpc_value(
        state.clone(), &token, "work.checkout",
        json!({ "bear_slug": bear_slug, "session_id": format!("work-{}", Uuid::new_v4().simple()), "work_order_id": work_run_id, "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] } }),
    ).await;
    assert_eq!(denied["result"]["ok"], false, "{denied}");
    assert_eq!(denied["result"]["permission_mode"], "none", "{denied}");
    assert_eq!(
        denied["result"]["gate"]["disposition"], "require_checkpoint",
        "{denied}"
    );

    let acknowledge = rpc_value(
        state.clone(), &token, "work.checkpoint_evidence",
        json!({ "bear_slug": bear_slug, "directive_id": directive_id, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch, "summary": "exploration limit reached; requesting a fresh fence" }),
    ).await;
    assert_eq!(
        acknowledge["result"]["directive_id"],
        directive_id.to_string(),
        "{acknowledge}"
    );
    let artifact_ref = acknowledge["result"]["checkpoint_artifact_ref"]
        .as_str()
        .expect("evidence endpoint returns artifact ref");
    let acknowledged: Option<(String, String)> = sqlx::query_as(
        "SELECT state, acknowledged_artifact_ref FROM docket_checkpoint_directives WHERE id = $1",
    )
    .bind(directive_id)
    .fetch_optional(&pool)
    .await
    .expect("directive query");
    assert_eq!(
        acknowledged,
        Some(("acknowledged".to_string(), artifact_ref.to_string()))
    );
    let stale_fence = rpc_value(
        state.clone(), &token, "work.checkpoint_evidence",
        json!({ "bear_slug": bear_slug, "directive_id": directive_id, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch + 1, "summary": "must reject stale fence" }),
    ).await;
    assert!(stale_fence.get("error").is_some(), "{stale_fence}");

    let resumed = rpc_value(
        state, &token, "work.checkout",
        json!({ "bear_slug": bear_slug, "session_id": format!("work-{}", Uuid::new_v4().simple()), "work_order_id": work_run_id, "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] } }),
    ).await;
    assert_eq!(resumed["result"]["ok"], true, "{resumed}");
    assert_eq!(
        resumed["result"]["execution_attempt_id"],
        attempt_id.to_string(),
        "{resumed}"
    );
    assert!(
        resumed["result"]["execution_attempt_fence_epoch"]
            .as_i64()
            .expect("resumed checkout returns fence epoch")
            > fence_epoch,
        "{resumed}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkpoint_signals_require_a_fresh_fence(pool: sqlx::PgPool) {
    for signal in ["repeated_failure", "near_ko"] {
        let user_id = create_test_user(&pool).await;
        let (bear_id, bear_slug) = create_test_bear(&pool).await;
        let token = create_token_for_bear(&pool, user_id, bear_id).await;
        let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
        let state = test_state(pool.clone());
        let checkout = rpc_value(
            state.clone(), &token, "work.checkout",
            json!({ "bear_slug": bear_slug, "session_id": format!("work-{}", Uuid::new_v4().simple()), "work_order_id": work_run_id, "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] } }),
        ).await;
        let attempt_id: Uuid = checkout["result"]["execution_attempt_id"]
            .as_str()
            .expect("attempt id")
            .parse()
            .expect("UUID");
        let fence_epoch = checkout["result"]["execution_attempt_fence_epoch"]
            .as_i64()
            .expect("fence epoch");
        let boundary = rpc_value(
            state.clone(), &token, "work.boundary",
            json!({ "bear_slug": bear_slug, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch, "boundary_key": Uuid::new_v4(), "signal": signal }),
        ).await;
        assert_eq!(
            boundary["result"]["gate"]["disposition"], "require_checkpoint",
            "{signal}: {boundary}"
        );
        let directive_id: Uuid = sqlx::query_scalar("SELECT id FROM docket_checkpoint_directives WHERE execution_attempt_id = $1 AND fence_epoch = $2")
            .bind(attempt_id).bind(fence_epoch).fetch_one(&pool).await.expect("checkpoint directive");
        let artifact_ref = format!("artifact_{}", Uuid::new_v4().simple());
        let artifact_id: Uuid = sqlx::query_scalar("INSERT INTO artifacts (artifact_ref, bear_id, owner_profile, kind, storage_kind) VALUES ($1, $2, 'work', 'runtime_checkpoint', 'db_text') RETURNING id")
            .bind(&artifact_ref).bind(bear_id).fetch_one(&pool).await.expect("checkpoint artifact");
        sqlx::query("INSERT INTO artifact_links (artifact_id, target_kind, target_id, role) VALUES ($1, 'work_run', $2, 'runtime_checkpoint')")
            .bind(artifact_id).bind(work_run_id.to_string()).execute(&pool).await.expect("link checkpoint artifact");
        let acknowledged = rpc_value(
            state.clone(), &token, "work.acknowledge_checkpoint",
            json!({ "bear_slug": bear_slug, "directive_id": directive_id, "execution_attempt_id": attempt_id, "fence_epoch": fence_epoch, "checkpoint_artifact_ref": artifact_ref }),
        ).await;
        assert_eq!(
            acknowledged["result"]["state"], "acknowledged",
            "{signal}: {acknowledged}"
        );
        let resumed = rpc_value(
            state, &token, "work.checkout",
            json!({ "bear_slug": bear_slug, "session_id": format!("work-{}", Uuid::new_v4().simple()), "work_order_id": work_run_id, "compatibility": { "protocol": 1, "capabilities": ["tool_attempt_token"] } }),
        ).await;
        assert!(
            resumed["result"]["execution_attempt_fence_epoch"]
                .as_i64()
                .expect("resumed fence")
                > fence_epoch,
            "{signal}: {resumed}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_checkout_preserves_selected_session_current_task(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let client_session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &client_session_id).await;
    let selected_session_task_id =
        create_session_task(&pool, user_id, bear_id, &client_session_id, "Session task").await;
    let state = test_state(pool.clone());
    let selected = rpc_value(
        state.clone(),
        &token,
        "session.current_task.select",
        json!({
            "bear_slug": bear_slug,
            "session_id": client_session_id,
            "task_id": selected_session_task_id,
        }),
    )
    .await;
    assert_eq!(
        selected["result"]["current_task_id"],
        selected_session_task_id.to_string()
    );

    let work_run_id = create_checkoutable_work_run(&pool, user_id, bear_id).await;
    let work_session_id = format!("work-{}", Uuid::new_v4().simple());
    let checkout = checkout_work_run_for_session(&pool, work_run_id, bear_id, &work_session_id)
        .await
        .expect("checkout work run");
    assert_eq!(checkout.run.id, work_run_id);

    let client_session =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &client_session_id)
            .await
            .expect("load client session")
            .expect("client session exists");
    assert_eq!(
        client_session.current_task_id,
        Some(selected_session_task_id),
        "Work checkout must not replace the selected session task"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_start_recovers_an_abandoned_continuation(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Recover Pair task").await;
    client_sessions::set_current_task(&pool, user_id, bear_id, &session_id, Some(task_id))
        .await
        .expect("select test task");
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create source run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("start source run");
    let snapshot = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        session_id.clone(),
        bear_id,
        user_id,
        Some(task_id),
        json!({
            "client": "test-client", "cwd": null, "conversation_id": "conversation-1",
            "prompt": "Continue.", "prompt_context": null, "client_context": null,
            "requested_mode": null,
        }),
    ))
    .expect("serialize recovery snapshot");
    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("claim continuation"),
        turn_runs::TechnicalBudgetContinuationClaim::Claimed(_)
    ));

    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let response = rpc_value(
        test_state_with_config(pool.clone(), config),
        &token,
        "session.current_task.start",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert_eq!(response["result"]["recovered"], true, "{response}");
    assert_eq!(response["result"]["recovered_run_id"], run_id, "{response}");
    assert_ne!(response["result"]["run_id"], run_id, "{response}");
    assert_eq!(response["result"]["state"], "accepted", "{response}");
    assert_eq!(response["result"]["launch_state"], "claimed", "{response}");
    assert_eq!(
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists")
            .current_task_id,
        Some(task_id)
    );
    let ledger = den_runtime::agent_loop::list_loop_control_decisions_for_run(&pool, &run_id)
        .await
        .expect("list recovery ledger");
    assert!(ledger.iter().any(|entry| {
        entry.decision_kind == "budget_slice_recovery"
            && entry.related_docket_task_id == Some(task_id)
            && entry.decision["same_run"] == false
            && entry.decision["replacement_run_id"] == response["result"]["run_id"]
            && entry.decision["launch_state"] == "claimed"
    }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_recover_refuses_when_selected_pair_task_changed(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Recover Pair task").await;
    client_sessions::set_current_task(&pool, user_id, bear_id, &session_id, Some(task_id))
        .await
        .expect("select test task");
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("transition run to running");
    let snapshot = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        session_id.clone(),
        bear_id,
        user_id,
        Some(task_id),
        json!({
            "client": "test-client",
            "cwd": null,
            "conversation_id": "conversation-1",
            "prompt": "Continue.",
            "prompt_context": null,
            "client_context": null,
            "requested_mode": null,
        }),
    ))
    .expect("serialize recovery snapshot");
    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("claim recovery continuation"),
        turn_runs::TechnicalBudgetContinuationClaim::Claimed(_)
    ));
    client_sessions::set_current_task(&pool, user_id, bear_id, &session_id, None)
        .await
        .expect("clear selected task");

    let response = rpc_value(
        test_state(pool.clone()),
        &token,
        "run.recover",
        json!({ "bear_slug": bear_slug, "run_id": run_id }),
    )
    .await;
    assert!(response.get("error").is_some(), "{response}");
    let stored = turn_runs::technical_budget_recovery_snapshot(&pool, &run_id)
        .await
        .expect("load recovery snapshot")
        .expect("snapshot remains available after rejected recovery");
    assert!(stored.recovery_lease_id.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_recovery_launches_claimed_successor_and_preserves_selected_task(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Recover Pair task").await;
    client_sessions::set_current_task(&pool, user_id, bear_id, &session_id, Some(task_id))
        .await
        .expect("select test task");
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create source run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("start source run");
    let snapshot = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        session_id.clone(),
        bear_id,
        user_id,
        Some(task_id),
        json!({
            "client": "test-client",
            "cwd": null,
            "conversation_id": "conversation-1",
            "prompt": "Continue.",
            "prompt_context": null,
            "client_context": null,
            "requested_mode": null,
        }),
    ))
    .expect("serialize recovery snapshot");
    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("claim continuation"),
        turn_runs::TechnicalBudgetContinuationClaim::Claimed(_)
    ));

    let response = rpc_value(
        state.clone(),
        &token,
        "run.recover",
        json!({ "bear_slug": bear_slug, "run_id": run_id }),
    )
    .await;
    assert_eq!(response["result"]["ok"], true, "{response}");
    assert_eq!(response["result"]["recovered_run_id"], run_id, "{response}");
    assert_ne!(response["result"]["run_id"], run_id, "{response}");
    assert_eq!(response["result"]["state"], "accepted", "{response}");
    assert_eq!(response["result"]["launch_state"], "claimed", "{response}");
    let replacement_run_id = response["result"]["run_id"]
        .as_str()
        .expect("replacement run id");
    wait_for_focused_run_started(state, &token, &bear_slug, &session_id, replacement_run_id).await;

    let source = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load source run")
        .expect("source run remains durable");
    assert_eq!(source.state, "cancelled");
    assert!(
        turn_runs::technical_budget_recovery_snapshot(&pool, &run_id)
            .await
            .expect("load consumed snapshot")
            .is_none()
    );
    let current_task: Option<Uuid> = sqlx::query_scalar(
        "SELECT current_task_id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load preserved selected task");
    assert_eq!(current_task, Some(task_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_concurrent_technical_budget_claim_leaves_run_continuing(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("transition run to running");
    let snapshot = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        session_id,
        bear_id,
        user_id,
        None,
        json!({"client": "test-client"}),
    ))
    .expect("serialize recovery snapshot");

    let (first, second) = tokio::join!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        ),
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        ),
    );
    assert!(matches!(
        (first, second),
        (
            Ok(turn_runs::TechnicalBudgetContinuationClaim::Claimed(_)),
            Ok(turn_runs::TechnicalBudgetContinuationClaim::AlreadyClaimed)
        ) | (
            Ok(turn_runs::TechnicalBudgetContinuationClaim::AlreadyClaimed),
            Ok(turn_runs::TechnicalBudgetContinuationClaim::Claimed(_))
        )
    ));

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run remains present");
    assert_eq!(run.state, "continuing");
    assert_ne!(run.state, "failed");

    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            "missing-run",
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("check missing run"),
        turn_runs::TechnicalBudgetContinuationClaim::RunStateConflict { actual_state: None }
    ));
    sqlx::query("UPDATE turn_runs SET state = 'failed' WHERE run_id = $1")
        .bind(&run_id)
        .execute(&pool)
        .await
        .expect("force terminal state for claim disposition test");
    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("check terminal run"),
        turn_runs::TechnicalBudgetContinuationClaim::RunStateConflict {
            actual_state: Some(state)
        } if state == "failed"
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn technical_budget_recovery_lease_is_exclusive_and_releasable(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("transition run to running");
    let snapshot = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        session_id,
        bear_id,
        user_id,
        None,
        json!({"client": "test-client"}),
    ))
    .expect("serialize recovery snapshot");
    assert!(matches!(
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &run_id,
            "emergency_hard_step_limit",
            &snapshot,
        )
        .await
        .expect("claim recovery continuation"),
        turn_runs::TechnicalBudgetContinuationClaim::Claimed(_)
    ));

    let first_lease = Uuid::new_v4();
    let leased = turn_runs::lease_technical_budget_recovery(&pool, &run_id, first_lease)
        .await
        .expect("lease recovery")
        .expect("first recovery worker owns the lease");
    assert_eq!(leased.recovery_lease_id, Some(first_lease));
    assert!(leased.recovery_lease_expires_at.is_some());
    assert!(
        turn_runs::lease_technical_budget_recovery(&pool, &run_id, Uuid::new_v4())
            .await
            .expect("check second lease")
            .is_none()
    );

    assert!(
        turn_runs::release_technical_budget_recovery(&pool, &run_id, first_lease)
            .await
            .expect("release failed replacement-start lease")
    );
    let second_lease = Uuid::new_v4();
    assert!(
        turn_runs::lease_technical_budget_recovery(&pool, &run_id, second_lease)
            .await
            .expect("lease after release")
            .is_some()
    );
    assert!(
        turn_runs::complete_technical_budget_recovery(&pool, &run_id, second_lease)
            .await
            .expect("consume recovery after replacement starts")
    );
    assert!(
        turn_runs::technical_budget_recovery_snapshot(&pool, &run_id)
            .await
            .expect("load consumed recovery")
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_task_rpc_requires_confirmation_and_preserves_clear_title(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Ship Pair controls").await;
    let params = json!({
        "bear_slug": bear_slug,
        "session_id": session_id,
        "task_id": task_id,
    });

    let preview = rpc_value(
        test_state(pool.clone()),
        &token,
        "session.current_task.selection_request",
        params.clone(),
    )
    .await;
    assert_eq!(
        preview["result"]["confirmation_required"], true,
        "{preview}"
    );
    let current: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT current_task_id FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load current task after preview");
    assert_eq!(current, None);

    let selected = rpc_value(
        test_state(pool.clone()),
        &token,
        "session.current_task.select",
        params,
    )
    .await;
    assert_eq!(
        selected["result"]["current_task_id"],
        task_id.to_string(),
        "{selected}"
    );
    assert_eq!(
        selected["result"]["title"], "Ship Pair controls",
        "{selected}"
    );

    let cleared = rpc_value(
        test_state(pool.clone()),
        &token,
        "session.current_task.clear",
        json!({ "bear_slug": bear_slug, "session_id": session_id }),
    )
    .await;
    assert!(cleared["result"]["current_task_id"].is_null(), "{cleared}");
    let (current, title): (Option<uuid::Uuid>, Option<String>) = sqlx::query_as(
        "SELECT current_task_id, conversation_title FROM client_sessions WHERE user_id = $1 AND bear_id = $2 AND client_session_id = $3",
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load cleared current task and title");
    assert_eq!(current, None);
    assert_eq!(title.as_deref(), Some("Ship Pair controls"));

    let other_session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &other_session_id).await;
    let other_task_id = create_session_task(
        &pool,
        user_id,
        bear_id,
        &other_session_id,
        "Other session task",
    )
    .await;
    let rejected = rpc_value(
        test_state(pool.clone()),
        &token,
        "session.current_task.selection_request",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "task_id": other_task_id,
        }),
    )
    .await;
    assert!(rejected.get("error").is_some(), "{rejected}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_task_settlement_rpc_settles_and_releases_attachment(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let task_id =
        create_session_task(&pool, user_id, bear_id, &session_id, "Settle through RPC").await;

    let settled = rpc_value(
        test_state(pool.clone()),
        &token,
        "docket.session_tasks.settle",
        json!({
            "bear_slug": bear_slug,
            "session_id": session_id,
            "task_id": task_id,
            "status": "done",
            "result_summary": "Verified Pair adapter settlement."
        }),
    )
    .await;

    assert!(settled.get("error").is_none(), "{settled}");
    assert_eq!(settled["result"]["task"]["task"]["id"], json!(task_id));
    assert!(settled["result"]["task"]["task"]["settled_by_entry_id"].is_string());
    let attachment_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bear_session_task_attachments WHERE task_id = $1 AND released_at IS NULL",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .expect("count active attachments");
    assert_eq!(attachment_count, 0);
}

#[tokio::test]
async fn initialize_returns_bearwire_capabilities() {
    let response = rpc(
        State(test_state(
            sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").unwrap(),
        )),
        HeaderMap::new(),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-1")),
            method: "initialize".to_string(),
            params: json!({}),
        }),
    )
    .await
    .expect("initialize ok")
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn planned_v1_methods_are_recognized() {
    let state = test_state(
        sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").unwrap(),
    );
    for method in [
        "session.open",
        "session.resume",
        "session.close",
        "session.state",
        "session.execution.diagnostics",
        "run.start",
        "run.state",
        "run.timeline",
        "run.cancel",
        "run.recover",
        "client.tool.result",
        "client.permission.result",
        "resource.update",
    ] {
        let response = rpc(
            State(state.clone()),
            HeaderMap::new(),
            Json(JsonRpcRequest {
                jsonrpc: Some("2.0".to_string()),
                id: Some(json!(method)),
                method: method.to_string(),
                params: json!({ "session_id": "session-test" }),
            }),
        )
        .await
        .expect("rpc ok")
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(
            value.pointer("/error/code"),
            Some(&json!(-32601)),
            "{method}"
        );
    }
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let response = rpc(
        State(test_state(
            sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").unwrap(),
        )),
        HeaderMap::new(),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!("req-unknown")),
            method: "not.real".to_string(),
            params: json!({}),
        }),
    )
    .await
    .expect("rpc ok")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], -32601);
}

async fn assert_method_requires_bearer_token(method: &str, params: Value) {
    let response = rpc(
        State(test_state(
            sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").unwrap(),
        )),
        HeaderMap::new(),
        Json(JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(json!(method)),
            method: method.to_string(),
            params,
        }),
    )
    .await
    .expect("rpc ok")
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], -32001);
    assert!(value["error"]["data"]["error"]
        .as_str()
        .unwrap()
        .contains("missing Authorization"));
}

#[tokio::test]
async fn bear_scoped_methods_require_bearer_token() {
    assert_method_requires_bearer_token(
        "session.open",
        json!({ "bear_slug": "meta", "session_id": "session-test" }),
    )
    .await;
    assert_method_requires_bearer_token("session.state", json!({ "bear_slug": "meta" })).await;
    assert_method_requires_bearer_token(
        "run.start",
        json!({ "bear_slug": "meta", "session_id": "session-test", "prompt": "hello" }),
    )
    .await;
    assert_method_requires_bearer_token(
        "run.cancel",
        json!({
            "bear_slug": "meta",
            "session_id": "session-test",
            "run_id": "run-test"
        }),
    )
    .await;
    assert_method_requires_bearer_token(
        "client.tool.result",
        json!({
            "bear_slug": "meta",
            "session_id": "session-test",
            "run_id": "run-test",
            "tool_call_id": "call-test",
            "status": "ok"
        }),
    )
    .await;
    assert_method_requires_bearer_token(
        "client.permission.result",
        json!({
            "bear_slug": "meta",
            "session_id": "session-test",
            "run_id": "run-test",
            "permission_id": "perm-test",
            "decision": "approved"
        }),
    )
    .await;
    assert_method_requires_bearer_token(
        "resource.update",
        json!({
            "bear_slug": "meta",
            "session_id": "session-test",
            "resource": { "kind": "acp_adapter", "id": "armature-test" }
        }),
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_diagnostics_includes_bounded_owned_checkpoint_artifacts(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    upsert_test_session(&pool, user_id, bear_id, &bear_slug, &session_id).await;
    let conversation_id: String = sqlx::query_scalar(
        "SELECT conversation_id FROM client_sessions WHERE client_session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .expect("load conversation id");
    ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure conversation");
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
        .await
        .expect("start run");
    den_runtime::agent_loop::record_checkpoint_request(
        &pool,
        den_runtime::agent_loop::CheckpointArtifactInput {
            bear_id,
            created_by_user_id: Some(user_id),
            owner_profile: BearProfile::Pair,
            run_id: run_id.clone(),
            turn_step_id: None,
            orientation_kind: None,
            audit_context: None,
            request: den_runtime::agent_loop::RuntimeCheckpointRequest {
                checkpoint_id: "ckpt-diagnostics".to_string(),
                run_id: run_id.clone(),
                reason: den_runtime::agent_loop::CheckpointReason::OverExploration,
                control_level: den_core::AgentLoopControlLevel::Standard,
                profile_fingerprint: None,
                active_objective: Some("test checkpoint audit".to_string()),
                task_context: None,
                evidence_refs: vec![],
                required_fields: vec![],
            },
            visibility: den_runtime::agent_loop::CheckpointVisibility::AuditOnly,
            replay_policy: den_runtime::agent_loop::CheckpointReplayPolicy::None,
        },
    )
    .await
    .expect("record checkpoint");

    let response = rpc_value(
        test_state(pool),
        &token,
        "conversation.diagnostics",
        json!({
            "bear_slug": bear_slug,
            "conversation_id": conversation_id,
            "run_id": run_id,
            "include_checkpoints": true,
            "limit": 1,
        }),
    )
    .await;
    assert_eq!(
        response["result"]["checkpoints"].as_array().map(Vec::len),
        Some(1),
        "{response}"
    );
    assert_eq!(
        response["result"]["checkpoints"][0]["checkpoint_id"],
        "ckpt-diagnostics"
    );
    assert!(response["result"]["records"].as_array().is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_conversation_reads_require_canonical_owner_or_bear_admin(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    let owned_id = format!("owned-{}", Uuid::new_v4());
    let null_id = format!("unowned-{}", Uuid::new_v4());
    let owned =
        ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &owned_id, None, None)
            .await
            .expect("create owned conversation");
    ensure_conversation_for_external_id(&pool, bear_id, None, &null_id, None, None)
        .await
        .expect("create NULL-owner conversation");
    append_message(
        &pool,
        owned.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::User,
            role: Some(ConversationMessageRole::User),
            visibility: ConversationMessageVisibility::Default,
            content_text: "private owner message".to_string(),
            content_json: json!({}),
            provider_message_id: None,
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append private message");
    let state = test_state(pool.clone());
    for method in [
        "conversation.history",
        "conversation.surface_history",
        "conversation.diagnostics",
    ] {
        let params = if method == "conversation.surface_history" {
            json!({"bear_slug": slug, "conversation_id": owned_id, "include_surface_enrichment": true})
        } else {
            json!({"bear_slug": slug, "conversation_id": owned_id})
        };
        let allowed = rpc_value(state.clone(), &owner_token, method, params.clone()).await;
        assert!(allowed.get("error").is_none(), "owner {method}: {allowed}");
        if method == "conversation.history" {
            assert!(
                allowed["result"]["messages"]
                    .to_string()
                    .contains("private owner message"),
                "{allowed}"
            );
        }
        let forbidden = rpc_value(state.clone(), &other_token, method, params.clone()).await;
        assert!(
            forbidden["error"]["data"]["error"]
                .as_str()
                .unwrap_or("")
                .contains("Not Found"),
            "other {method}: {forbidden}"
        );
        let admin_read = rpc_value(state.clone(), &admin_token, method, params).await;
        assert!(
            admin_read.get("error").is_none(),
            "admin {method}: {admin_read}"
        );
        let null_params = json!({"bear_slug": slug, "conversation_id": null_id});
        let null_denied = rpc_value(state.clone(), &owner_token, method, null_params.clone()).await;
        assert!(
            null_denied.get("error").is_some(),
            "NULL owner must fail closed: {null_denied}"
        );
        let null_admin = rpc_value(state.clone(), &admin_token, method, null_params).await;
        assert!(
            null_admin.get("error").is_none(),
            "admin may inspect NULL owner: {null_admin}"
        );
    }
    bears_db::grant_membership(&pool, admin, bear_id, Some(bears_db::BEAR_ROLE_MEMBER))
        .await
        .expect("demote Bear admin");
    let demoted = rpc_value(
        state.clone(),
        &admin_token,
        "conversation.history",
        json!({"bear_slug": slug, "conversation_id": owned_id}),
    )
    .await;
    assert!(
        demoted.get("error").is_some(),
        "demoted admin must not inspect: {demoted}"
    );
    bears_db::revoke_membership(&pool, owner, bear_id)
        .await
        .expect("remove membership");
    for method in ["conversation.history", "session.open", "run.start"] {
        let params = if method == "conversation.history" {
            json!({"bear_slug": slug, "conversation_id": owned_id})
        } else if method == "session.open" {
            json!({"bear_slug": slug, "conversation_id": owned_id, "session_id": format!("stale-{}", Uuid::new_v4())})
        } else {
            json!({"bear_slug": slug, "conversation_id": owned_id, "session_id": format!("stale-{}", Uuid::new_v4()), "prompt": "must not run"})
        };
        let stale = rpc_value(state.clone(), &owner_token, method, params).await;
        assert!(
            stale.get("error").is_some(),
            "stale membership {method}: {stale}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_open_model_and_start_reject_other_owners_before_side_effects(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    let owned_id = format!("owned-{}", Uuid::new_v4());
    let null_id = format!("unowned-{}", Uuid::new_v4());
    let owned =
        ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &owned_id, None, None)
            .await
            .expect("create owned conversation");
    ensure_conversation_for_external_id(&pool, bear_id, None, &null_id, None, None)
        .await
        .expect("create NULL-owner conversation");
    let state = test_state(pool.clone());
    let session_id = format!("session-{}", Uuid::new_v4());
    for method in ["session.open", "run.start"] {
        let params = json!({"bear_slug": slug, "session_id": session_id, "conversation_id": owned_id, "prompt": "must not run"});
        let denied = rpc_value(state.clone(), &other_token, method, params.clone()).await;
        assert!(
            denied["error"]["data"]["error"]
                .as_str()
                .unwrap_or("")
                .contains("Not Found"),
            "{method}: {denied}"
        );
        let null_denied = rpc_value(state.clone(), &other_token, method, json!({"bear_slug": slug, "session_id": session_id, "conversation_id": null_id, "prompt": "must not run"})).await;
        assert!(
            null_denied.get("error").is_some(),
            "NULL owner {method}: {null_denied}"
        );
    }
    assert!(
        client_sessions::find_for_user_bear_session(&pool, other, &slug, &session_id)
            .await
            .expect("find session")
            .is_none()
    );
    let source: Option<String> =
        sqlx::query_scalar("SELECT source_client_session_id FROM conversations WHERE id = $1")
            .bind(owned.id)
            .fetch_one(&pool)
            .await
            .expect("read canonical metadata");
    assert!(
        source.is_none(),
        "denied attach must not mutate canonical metadata"
    );
    let message_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversation_messages WHERE conversation_id = $1")
            .bind(owned.id)
            .fetch_one(&pool)
            .await
            .expect("count messages");
    assert_eq!(message_count, 0);
    let open = rpc_value(
        state.clone(),
        &owner_token,
        "session.open",
        json!({"bear_slug": slug, "session_id": session_id, "conversation_id": owned_id}),
    )
    .await;
    assert_eq!(open["result"]["ok"], true, "same-owner open: {open}");
    let admin_open = rpc_value(state.clone(), &admin_token, "session.open", json!({"bear_slug": slug, "session_id": format!("admin-{}", Uuid::new_v4()), "conversation_id": null_id})).await;
    assert_eq!(
        admin_open["result"]["ok"], true,
        "admin NULL-owner open: {admin_open}"
    );

    // A client session is not proof of canonical ownership, even when it is
    // associated with the authenticated human.
    let hijack_id = format!("model-{}", Uuid::new_v4());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: other,
            bear_id,
            bear_slug: slug.clone(),
            client_session_id: hijack_id.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{hijack_id}"),
            conversation_id: owned_id.clone(),
            resolved_conversation_id: None,
            client: "bearwire-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("seed session pointing at someone else's conversation");
    for method in [
        "session.model.get",
        "session.model.set",
        "session.state",
        "session.execution.diagnostics",
    ] {
        let denied = rpc_value(
            state.clone(),
            &other_token,
            method,
            json!({"bear_slug": slug, "session_id": hijack_id, "selection_mode": "auto"}),
        )
        .await;
        assert!(
            denied.get("error").is_some(),
            "{method} must deny hijacked canonical ID: {denied}"
        );
    }
    let denied_open = rpc_value(
        state.clone(),
        &other_token,
        "session.open",
        json!({"bear_slug": slug, "session_id": hijack_id}),
    )
    .await;
    assert!(
        denied_open.get("error").is_some(),
        "session.open must recheck existing session: {denied_open}"
    );
    let denied = rpc_value(
        state.clone(),
        &other_token,
        "run.start",
        json!({"bear_slug": slug, "session_id": hijack_id, "prompt": "must not run"}),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "run.start must deny hijacked session: {denied}"
    );
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(&pool, &run_id, &hijack_id, bear_id, other)
        .await
        .expect("seed hijacked session run");
    for method in ["run.state", "run.cancel"] {
        let denied = rpc_value(
            state.clone(),
            &other_token,
            method,
            json!({"bear_slug": slug, "session_id": hijack_id, "run_id": run_id}),
        )
        .await;
        assert!(
            denied.get("error").is_some(),
            "{method} must deny hijacked session: {denied}"
        );
    }
    let model_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_model_state WHERE conversation_id = $1",
    )
    .bind(owned.id)
    .fetch_one(&pool)
    .await
    .expect("count model state");
    assert_eq!(
        model_count, 0,
        "denied model set must not mutate model state"
    );
    let own_model = rpc_value(
        state,
        &owner_token,
        "session.model.set",
        json!({"bear_slug": slug, "session_id": session_id, "selection_mode": "auto"}),
    )
    .await;
    assert_eq!(
        own_model["result"]["ok"], true,
        "same-owner model set: {own_model}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_owner_and_admin_can_continue_canonical_conversations(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    let owned_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let null_id = format!("den-conv-{}", Uuid::new_v4().simple());
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &owned_id, None, None)
        .await
        .expect("create owned conversation");
    // NativeRuntimeConversationBackend::create_conversation can create NULL-owner
    // rows. BearWire must not claim them for a non-admin on reconnect.
    ensure_conversation_for_external_id(&pool, bear_id, None, &null_id, None, None)
        .await
        .expect("create NULL-owner runtime conversation");
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    for (token, conversation_id, prompt) in [
        (&owner_token, &owned_id, "owner continuation"),
        (&admin_token, &null_id, "admin NULL-owner continuation"),
    ] {
        let session_id = format!("session-{}", Uuid::new_v4().simple());
        let start = rpc_value(
            state.clone(),
            token,
            "run.start",
            json!({
                "bear_slug": slug,
                "session_id": session_id,
                "conversation_id": conversation_id,
                "prompt": prompt,
            }),
        )
        .await;
        assert_eq!(start["result"]["accepted"], true, "{start}");
        wait_for_user_message(&pool, bear_id, conversation_id, prompt).await;
    }
    let owner_after: Option<i32> = sqlx::query_scalar(
        "SELECT created_by_user_id FROM conversations WHERE bear_id = $1 AND external_conversation_id = $2",
    ).bind(bear_id).bind(&null_id).fetch_one(&pool).await.expect("read NULL owner");
    assert_eq!(
        owner_after, None,
        "admin continuation must not reassign NULL owner"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_resolved_target_is_checked_independently_of_session_selection(
    pool: sqlx::PgPool,
) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let _owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let stolen = format!("den-conv-{}", Uuid::new_v4().simple());
    let pending = format!("new-acp-{}", Uuid::new_v4().simple());
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &stolen, None, None)
        .await
        .expect("create other member's conversation");
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: other,
            bear_id,
            bear_slug: slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{session_id}"),
            conversation_id: pending.clone(),
            resolved_conversation_id: Some(stolen.clone()),
            client: "bearwire-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("seed resolved session");
    let state = test_state(pool.clone());
    for method in ["session.open", "run.start"] {
        let denied = rpc_value(
            state.clone(),
            &other_token,
            method,
            json!({
                "bear_slug": slug, "session_id": session_id,
                "conversation_id": pending, "prompt": "must not run",
            }),
        )
        .await;
        assert!(
            denied.get("error").is_some(),
            "resolved target {method}: {denied}"
        );
    }
    let pending_record = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, bear_id, &pending,
    )
    .await
    .expect("check pending ID");
    assert!(
        pending_record.is_none(),
        "denied reconnect must not allocate pending ID"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_new_external_id_is_owned_before_run_writes(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server();
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let external = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let started = rpc_value(
        state.clone(),
        &owner_token,
        "run.start",
        json!({
            "bear_slug": slug, "session_id": session_id,
            "conversation_id": external, "prompt": "owner's first turn",
        }),
    )
    .await;
    assert_eq!(started["result"]["accepted"], true, "{started}");
    wait_for_user_message(&pool, bear_id, &external, "owner's first turn").await;
    let created_by: Option<i32> = sqlx::query_scalar(
        "SELECT created_by_user_id FROM conversations WHERE bear_id = $1 AND external_conversation_id = $2",
    ).bind(bear_id).bind(&external).fetch_one(&pool).await.expect("read canonical owner");
    assert_eq!(created_by, Some(owner));
    let denied = rpc_value(
        state,
        &other_token,
        "session.open",
        json!({
            "bear_slug": slug, "session_id": format!("other-{}", Uuid::new_v4()),
            "conversation_id": external,
        }),
    )
    .await;
    assert!(
        denied.get("error").is_some(),
        "second member cannot attach: {denied}"
    );
}

async fn seed_docket_visibility_surface(pool: &sqlx::PgPool, bear_id: Uuid, owner: i32) {
    sqlx::query!(
        r"
        INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
        VALUES ($1, $2, 'git_workspace', $3, NOW(), NOW())
        ",
        bear_id,
        format!("surface-{}", &bear_id.simple().to_string()[..12]),
        owner,
    )
    .execute(pool)
    .await
    .expect("create test work surface");
    sqlx::query!(
        r"
        INSERT INTO git_work_surface_details (id, upstream_url)
        VALUES ($1, $2)
        ",
        bear_id,
        "https://example.test/docket.git",
    )
    .execute(pool)
    .await
    .expect("create git details");
    sqlx::query!(
        r"
        INSERT INTO work_surface_bears (surface_id, bear_id)
        VALUES ($1, $2)
        ",
        bear_id,
        bear_id,
    )
    .execute(pool)
    .await
    .expect("assign surface to bear");
}

async fn visibility_job(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    owner: i32,
    visibility: TaskListVisibility,
    source: Option<&str>,
) -> den_docket::DocketJobProjection {
    PgDocketService::from_pool(pool)
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: owner,
            created_by_role: "pair".to_string(),
            goal: format!("Visibility test {}", Uuid::new_v4()),
            work_surface_id: Some(bear_id),
            work_surface_assignments: vec![],
            commit_policy: None,
            work_branch: None,
            visibility,
            source_conversation_id: source.map(str::to_string),
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![],
            tasks: vec![DocketTaskInput {
                client_key: None,
                parent_client_key: None,
                parent_task_id: None,
                sibling_order: Some(0),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "Private task".to_string(),
                body: "Private task body".to_string(),
                completion_criteria: vec!["Done".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            }],
        })
        .await
        .expect("create visibility job")
}

fn assert_docket_not_found(value: &Value) {
    assert!(
        value["error"]["data"]["error"]
            .as_str()
            .unwrap_or("")
            .contains("Not Found"),
        "expected an indistinguishable not-found result: {value}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn focused_execution_authorizes_task_before_attachment_and_reuse(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    create_member_token(&pool, owner, bear_id).await;
    create_member_token(&pool, other, bear_id).await;
    create_token_for_bear(&pool, admin, bear_id).await;
    seed_docket_visibility_surface(&pool, bear_id, owner).await;
    let private = visibility_job(&pool, bear_id, owner, TaskListVisibility::SameUser, None).await;
    let admin_private =
        visibility_job(&pool, bear_id, owner, TaskListVisibility::SameUser, None).await;
    let public = visibility_job(&pool, bear_id, owner, TaskListVisibility::BearVisible, None).await;
    let owner_session = format!("owner-{}", Uuid::new_v4());
    let other_session = format!("other-{}", Uuid::new_v4());
    let admin_session = format!("admin-{}", Uuid::new_v4());
    let standalone_session = format!("standalone-{}", Uuid::new_v4());
    for (user, session_id) in [
        (owner, &owner_session),
        (other, &other_session),
        (admin, &admin_session),
        (other, &standalone_session),
    ] {
        upsert_test_session(&pool, user, bear_id, &slug, session_id).await;
        set_next_scripted_runtime_streams(session_id, vec![ScriptedRuntimeStream::Pending]);
    }
    let own_standalone = create_session_task(
        &pool,
        other,
        bear_id,
        &standalone_session,
        "Own standalone task",
    )
    .await;
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".to_string();
    config.llm_api_url = start_mock_openai_sse_server_asserting_requests(vec![
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
        MockLlmRequestAssertion::requiring(Vec::new()),
    ]);
    config.default_llm_model = "openai/bearwire-test-model".to_string();
    seed_test_bifrost_virtual_key(&pool, bear_id, &config).await;
    let state = test_state_with_config(pool.clone(), config);
    let bear = bears_db::get_bear(&pool, bear_id)
        .await
        .expect("load bear")
        .expect("bear exists");
    let policy = den_core::EffectivePolicy::compile(
        den_core::TrustProfile::Pair,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Connected,
    );
    let start = |user, session: String, task_id| {
        let state = state.clone();
        let bear = bear.clone();
        let capabilities = policy.capabilities.clone();
        async move {
            crate::methods::focused_execution::start_or_reconcile_session_task_execution(
                &state,
                user,
                bear,
                &session,
                task_id,
                &capabilities,
            )
            .await
        }
    };
    let foreign_task = private.tasks[0].id;
    for task_id in [foreign_task, Uuid::new_v4()] {
        let denied = start(other, other_session.clone(), task_id)
            .await
            .expect_err("foreign and missing task IDs must be indistinguishable");
        assert!(matches!(denied, den_http::errors::CustomError::NotFound(_)));
        assert!(!denied.to_string().contains("Private task"));
    }
    let other_anchor: Uuid =
        sqlx::query_scalar("SELECT id FROM client_sessions WHERE client_session_id = $1")
            .bind(&other_session)
            .fetch_one(&pool)
            .await
            .expect("load other session anchor");
    let attachments: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM bear_session_task_attachments WHERE task_id = $1")
            .bind(foreign_task)
            .fetch_one(&pool)
            .await
            .expect("count foreign attachments");
    assert_eq!(attachments, 0);
    let other_runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM turn_runs WHERE session_id = $1")
            .bind(&other_session)
            .fetch_one(&pool)
            .await
            .expect("count unauthorized runs");
    assert_eq!(other_runs, 0);
    let other_attempts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM docket_execution_attempts WHERE binding_id = $1")
            .bind(&other_session)
            .fetch_one(&pool)
            .await
            .expect("count unauthorized attempts");
    assert_eq!(other_attempts, 0);

    // A forged persisted selection must not bypass the already-selected launch path.
    sqlx::query("UPDATE client_sessions SET current_task_id = $2 WHERE id = $1")
        .bind(other_anchor)
        .bind(foreign_task)
        .execute(&pool)
        .await
        .expect("simulate forged selection");
    let denied = crate::methods::focused_execution::start_selected_session_task_execution(
        &state,
        other,
        bear.clone(),
        &other_session,
        &policy.capabilities,
    )
    .await
    .expect_err("selected foreign task cannot start");
    assert!(matches!(denied, den_http::errors::CustomError::NotFound(_)));
    assert!(!denied.to_string().contains("Private task"));
    let unchanged: (Option<Uuid>, i64, i64) = sqlx::query_as(
        "SELECT current_task_id,
                (SELECT COUNT(*) FROM turn_runs WHERE session_id = $2),
                (SELECT COUNT(*) FROM docket_execution_attempts WHERE binding_id = $2)
         FROM client_sessions WHERE id = $1",
    )
    .bind(other_anchor)
    .bind(&other_session)
    .fetch_one(&pool)
    .await
    .expect("inspect denied selection");
    assert_eq!(unchanged, (Some(foreign_task), 0, 0));
    sqlx::query("UPDATE client_sessions SET current_task_id = NULL WHERE id = $1")
        .bind(other_anchor)
        .execute(&pool)
        .await
        .expect("clear forged selection");

    let owner_start = start(owner, owner_session.clone(), foreign_task)
        .await
        .expect("owner may focus own private job task");
    assert_eq!(owner_start.task_id(), Some(foreign_task));
    assert!(owner_start.run_id().is_some());
    let admin_start = start(admin, admin_session.clone(), admin_private.tasks[0].id)
        .await
        .expect("Bear admin may focus a member's private job task");
    assert_eq!(admin_start.task_id(), Some(admin_private.tasks[0].id));
    assert!(admin_start.run_id().is_some());
    let public_start = start(other, other_session.clone(), public.tasks[0].id)
        .await
        .expect("member may focus a BearVisible job task");
    assert_eq!(public_start.task_id(), Some(public.tasks[0].id));
    let standalone_start = start(other, standalone_session.clone(), own_standalone)
        .await
        .expect("member may focus own attached standalone task");
    assert_eq!(standalone_start.task_id(), Some(own_standalone));

    bears_db::grant_membership(&pool, admin, bear_id, Some(bears_db::BEAR_ROLE_MEMBER))
        .await
        .expect("demote admin after starting private task");
    let denied_reuse = start(admin, admin_session.clone(), admin_private.tasks[0].id)
        .await
        .expect_err("already-running focus must recheck current visibility");
    assert!(matches!(
        denied_reuse,
        den_http::errors::CustomError::NotFound(_)
    ));
    assert!(!denied_reuse.to_string().contains("Private task"));
    let admin_runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM turn_runs WHERE session_id = $1")
            .bind(&admin_session)
            .fetch_one(&pool)
            .await
            .expect("count admin runs after denied reuse");
    assert_eq!(admin_runs, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn docket_rpc_job_visibility_filters_before_limit_and_canonical_source_lookup(
    pool: sqlx::PgPool,
) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    seed_docket_visibility_surface(&pool, bear_id, owner).await;
    let source = format!("source-{}", Uuid::new_v4());
    let other_source = format!("source-{}", Uuid::new_v4());
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &source, None, None)
        .await
        .expect("create owner's source");
    ensure_conversation_for_external_id(&pool, bear_id, Some(other), &other_source, None, None)
        .await
        .expect("create other member's source");
    let owned = visibility_job(
        &pool,
        bear_id,
        owner,
        TaskListVisibility::SameUser,
        Some(&source),
    )
    .await;
    let foreign = visibility_job(
        &pool,
        bear_id,
        other,
        TaskListVisibility::PrivateToProfile,
        Some(&other_source),
    )
    .await;
    let state = test_state(pool.clone());

    let listed = rpc_value(
        state.clone(),
        &owner_token,
        "docket.jobs.list",
        json!({"bear_slug": slug, "limit": 1}),
    )
    .await;
    assert_eq!(
        listed["result"]["jobs"].as_array().unwrap().len(),
        1,
        "{listed}"
    );
    assert_eq!(
        listed["result"]["jobs"][0]["id"],
        json!(owned.job.id),
        "{listed}"
    );
    for (token, expected) in [
        (&other_token, foreign.job.id),
        (&admin_token, foreign.job.id),
    ] {
        let response = rpc_value(
            state.clone(),
            token,
            "docket.jobs.list",
            json!({"bear_slug": slug, "limit": 1}),
        )
        .await;
        assert_eq!(
            response["result"]["jobs"][0]["id"],
            json!(expected),
            "{response}"
        );
    }
    let denied_source = rpc_value(
        state.clone(),
        &owner_token,
        "docket.jobs.list",
        json!({"bear_slug": slug, "conversation_id": other_source}),
    )
    .await;
    assert_docket_not_found(&denied_source);
    let session_id = format!("source-session-{}", Uuid::new_v4());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: owner,
            bear_id,
            bear_slug: slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: format!("runtime-{session_id}"),
            conversation_id: source.clone(),
            resolved_conversation_id: Some(other_source.clone()),
            client: "bearwire-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("seed session with a competing canonical source");
    let denied_session_source = rpc_value(
        state.clone(),
        &owner_token,
        "docket.jobs.list",
        json!({"bear_slug": slug, "session_id": session_id}),
    )
    .await;
    assert_docket_not_found(&denied_session_source);
    let allowed_source = rpc_value(
        state.clone(),
        &owner_token,
        "docket.jobs.list",
        json!({"bear_slug": slug, "conversation_id": source}),
    )
    .await;
    assert_eq!(
        allowed_source["result"]["jobs"][0]["id"],
        json!(owned.job.id),
        "{allowed_source}"
    );

    let public = visibility_job(&pool, bear_id, owner, TaskListVisibility::BearVisible, None).await;
    let visible = rpc_value(
        state.clone(),
        &other_token,
        "docket.jobs.list",
        json!({"bear_slug": slug}),
    )
    .await;
    let ids: Vec<Value> = visible["result"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["id"].clone())
        .collect();
    assert!(
        ids.contains(&json!(public.job.id)) && ids.contains(&json!(foreign.job.id)),
        "{visible}"
    );
    assert!(!ids.contains(&json!(owned.job.id)), "{visible}");
    for (token, job_id) in [
        (&owner_token, owned.job.id),
        (&admin_token, owned.job.id),
        (&other_token, public.job.id),
    ] {
        let response = rpc_value(
            state.clone(),
            token,
            "docket.jobs.diagnostics",
            json!({"bear_slug": slug, "job_id": job_id}),
        )
        .await;
        assert_eq!(
            response["result"]["job"]["job"]["id"],
            json!(job_id),
            "{response}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn docket_rpc_job_controls_reject_guessed_ids_without_side_effects(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear_id).await;
    let other_token = create_member_token(&pool, other, bear_id).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    seed_docket_visibility_surface(&pool, bear_id, owner).await;
    let private = visibility_job(&pool, bear_id, owner, TaskListVisibility::SameUser, None).await;
    let public = visibility_job(&pool, bear_id, owner, TaskListVisibility::BearVisible, None).await;
    let state = test_state(pool.clone());
    let task_id = private.tasks[0].id;
    for job_id in [private.job.id, Uuid::new_v4()] {
        let params = json!({"bear_slug": slug, "job_id": job_id});
        for (method, params) in [
            ("docket.jobs.diagnostics", params.clone()),
            ("docket.jobs.cancel_run", params.clone()),
            ("docket.jobs.execute", params.clone()),
            ("docket.jobs.reconcile", params.clone()),
            (
                "docket.jobs.settle_task",
                json!({"bear_slug": slug, "job_id": job_id, "task_id": task_id, "status": "done"}),
            ),
        ] {
            let denied = rpc_value(state.clone(), &other_token, method, params).await;
            assert_docket_not_found(&denied);
        }
    }
    let mismatched_task = rpc_value(
        state.clone(),
        &other_token,
        "docket.jobs.settle_task",
        json!({"bear_slug": slug, "job_id": public.job.id, "task_id": task_id, "status": "done"}),
    )
    .await;
    assert_docket_not_found(&mismatched_task);
    let session_id = format!("member-session-{}", Uuid::new_v4());
    upsert_test_session(&pool, other, bear_id, &slug, &session_id).await;
    let generic_settle = rpc_value(
        state.clone(),
        &other_token,
        "docket.session_tasks.settle",
        json!({"bear_slug": slug, "session_id": session_id, "task_id": task_id, "status": "done"}),
    )
    .await;
    assert_docket_not_found(&generic_settle);
    let unchanged = PgDocketService::from_pool(&pool)
        .get_job(bear_id, private.job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.current_run.as_ref().unwrap().state, "dispatched");
    assert_eq!(unchanged.task_states[0].status, "pending");
    assert!(unchanged.tasks[0].settled_by_entry_id.is_none());
    let admin_read = rpc_value(
        state.clone(),
        &admin_token,
        "docket.jobs.diagnostics",
        json!({"bear_slug": slug, "job_id": private.job.id}),
    )
    .await;
    assert_eq!(
        admin_read["result"]["job"]["job"]["id"],
        json!(private.job.id),
        "{admin_read}"
    );
    let owner_read = rpc_value(
        state.clone(),
        &owner_token,
        "docket.jobs.diagnostics",
        json!({"bear_slug": slug, "job_id": private.job.id}),
    )
    .await;
    assert!(owner_read.get("error").is_none(), "{owner_read}");
    let public_read = rpc_value(
        state.clone(),
        &other_token,
        "docket.jobs.diagnostics",
        json!({"bear_slug": slug, "job_id": public.job.id}),
    )
    .await;
    assert!(public_read.get("error").is_none(), "{public_read}");
    let cancelled = rpc_value(
        state.clone(),
        &other_token,
        "docket.jobs.cancel_run",
        json!({"bear_slug": slug, "job_id": public.job.id}),
    )
    .await;
    assert_eq!(
        cancelled["result"]["job"]["current_run"]["state"], "cancelled",
        "{cancelled}"
    );
    let diagnostics_denied = rpc_value(
        state.clone(),
        &other_token,
        "runtime.diagnostics.list",
        json!({"bear_slug": slug, "docket_job_id": private.job.id}),
    )
    .await;
    assert!(
        diagnostics_denied["error"]["data"]["error"]
            .as_str()
            .unwrap_or("")
            .contains("Authorization"),
        "{diagnostics_denied}"
    );
    let diagnostics_admin = rpc_value(
        state.clone(),
        &admin_token,
        "runtime.diagnostics.list",
        json!({"bear_slug": slug, "docket_job_id": private.job.id}),
    )
    .await;
    assert!(
        diagnostics_admin.get("error").is_none(),
        "{diagnostics_admin}"
    );
    bears_db::grant_membership(&pool, admin, bear_id, Some(bears_db::BEAR_ROLE_MEMBER))
        .await
        .expect("demote admin");
    let demoted = rpc_value(
        state.clone(),
        &admin_token,
        "docket.jobs.diagnostics",
        json!({"bear_slug": slug, "job_id": private.job.id}),
    )
    .await;
    assert_docket_not_found(&demoted);
    let demoted_runtime = rpc_value(
        state,
        &admin_token,
        "runtime.diagnostics.list",
        json!({"bear_slug": slug}),
    )
    .await;
    assert!(demoted_runtime.get("error").is_some(), "{demoted_runtime}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn human_bearwire_history_excludes_model_only_messages_and_tool_payloads(pool: sqlx::PgPool) {
    let user_id = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user_id, bear_id).await;
    let external_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &external_id,
        None,
        None,
    )
    .await
    .expect("create canonical conversation");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::User,
            role: Some(ConversationMessageRole::User),
            visibility: ConversationMessageVisibility::Default,
            content_text: "visible human message".to_string(),
            content_json: json!({}),
            provider_message_id: None,
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append visible user message");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::Assistant,
            role: Some(ConversationMessageRole::Assistant),
            visibility: ConversationMessageVisibility::HiddenFromUser,
            content_text: "private model message sentinel".to_string(),
            content_json: json!({}),
            provider_message_id: None,
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append hidden model message");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite::structured(
            ConversationMessageType::ToolCall,
            Some(ConversationMessageRole::Assistant),
            ConversationMessageVisibility::HiddenFromUser,
            "",
            json!({
                "event": "tool_request",
                "tool_call_id": "hidden-call",
                "tool_name": "fs_read_text_file",
                "args": {"path": "private-tool-arguments-sentinel"},
                "approval_required": false,
            }),
        ),
    )
    .await
    .expect("append hidden tool call");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite::structured(
            ConversationMessageType::ToolResult,
            Some(ConversationMessageRole::System),
            ConversationMessageVisibility::HiddenFromUser,
            "private-tool-result-text-sentinel",
            json!({
                "event": "tool_result",
                "tool_call_id": "hidden-call",
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "private-tool-result-text-sentinel",
                "structured_content": {"content": "private-tool-raw-output-sentinel"},
            }),
        ),
    )
    .await
    .expect("append hidden tool result");

    let model = list_projected_messages_page(
        &pool,
        conversation.id,
        None,
        20,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await
    .expect("load internal model projection");
    assert_eq!(model.len(), 4);
    let model_records = model
        .iter()
        .filter_map(|row| row.to_model_history_record())
        .collect::<Vec<_>>();
    assert_eq!(model_records.len(), 4);
    let model_text = format!("{model_records:?}");
    for secret in [
        "private model message sentinel",
        "private-tool-arguments-sentinel",
        "private-tool-raw-output-sentinel",
    ] {
        assert!(
            model_text.contains(secret),
            "model projection must retain {secret}: {model_text}"
        );
    }
    let user_rows = list_projected_messages_page(
        &pool,
        conversation.id,
        None,
        20,
        ConversationHistoryProjection::UserHistory,
    )
    .await
    .expect("load user projection");
    assert_eq!(user_rows.len(), 1);
    let state = test_state(pool);
    for (method, key) in [
        ("conversation.history", "messages"),
        ("conversation.surface_history", "surface_events"),
    ] {
        let response = rpc_value(
            state.clone(),
            &token,
            method,
            json!({
                "bear_slug": slug, "conversation_id": external_id,
                "include_surface_enrichment": false,
            }),
        )
        .await;
        assert!(response.get("error").is_none(), "{method}: {response}");
        let records = response["result"][key]
            .as_array()
            .expect("human history records");
        assert_eq!(records.len(), 1, "{method}: {response}");
        assert_eq!(records[0]["text"], "visible human message");
        assert!(matches!(
            serde_json::from_value::<SurfaceHistoryEvent>(records[0].clone()),
            Ok(SurfaceHistoryEvent::Message { .. })
        ));
        for secret in [
            "private model message sentinel",
            "private-tool-arguments-sentinel",
            "private-tool-result-text-sentinel",
            "private-tool-raw-output-sentinel",
        ] {
            assert!(
                !response.to_string().contains(secret),
                "{method} leaked {secret}: {response}"
            );
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn human_surface_enrichment_rejects_foreign_sessions_and_work_jobs(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let other = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let token = create_member_token(&pool, owner, bear_id).await;
    let _other_token = create_member_token(&pool, other, bear_id).await;
    let external_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let conversation =
        ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &external_id, None, None)
            .await
            .expect("create owner's canonical conversation");
    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::User,
            role: Some(ConversationMessageRole::User),
            visibility: ConversationMessageVisibility::Default,
            content_text: "visible canonical content".to_string(),
            content_json: json!({}),
            provider_message_id: None,
            source_event_id: None,
            created_at: None,
        },
    )
    .await
    .expect("append canonical content");
    let own_session = format!("session-{}", Uuid::new_v4().simple());
    let foreign_session = format!("session-{}", Uuid::new_v4().simple());
    for (user_id, session_id) in [(owner, &own_session), (other, &foreign_session)] {
        client_sessions::upsert_session(
            &pool,
            client_sessions::UpsertClientSession {
                user_id,
                bear_id,
                bear_slug: slug.clone(),
                client_session_id: session_id.clone(),
                runtime_session_id: format!("bearwire:{bear_id}:{session_id}"),
                conversation_id: external_id.clone(),
                resolved_conversation_id: None,
                client: "bearwire-test".to_string(),
                cwd: None,
                current_mode: None,
            },
        )
        .await
        .expect("seed matching external ID session");
    }
    bearwire_events::append_bearwire_event(
        &pool,
        &foreign_session,
        Some(bear_id),
        Some(other),
        BearWireEvent::ephemeral(
            "session_info_update",
            json!({"title": "foreign-session-title-sentinel"}),
        ),
    )
    .await
    .expect("append foreign session event");
    let state = test_state(pool.clone());
    let params = json!({"bear_slug": slug, "conversation_id": external_id, "include_surface_enrichment": true});
    let foreign = rpc_value(
        state.clone(),
        &token,
        "conversation.surface_history",
        params.clone(),
    )
    .await;
    assert!(foreign.get("error").is_none(), "{foreign}");
    assert!(
        foreign["result"]["surface_events"]
            .to_string()
            .contains("visible canonical content"),
        "{foreign}"
    );
    assert!(
        !foreign.to_string().contains(&foreign_session),
        "foreign session ID leaked: {foreign}"
    );
    assert!(
        !foreign
            .to_string()
            .contains("foreign-session-title-sentinel"),
        "foreign event leaked: {foreign}"
    );

    // The same human's session can still be unsafe if its bound Work job belongs
    // to another member. No session/work enrichment should be replayed in that case.
    let work_run_id = create_checkoutable_work_run(&pool, other, bear_id).await;
    checkout_work_run_for_session(&pool, work_run_id, bear_id, &own_session)
        .await
        .expect("associate foreign job with owner's session");
    bearwire_events::append_bearwire_event(
        &pool,
        &own_session,
        Some(bear_id),
        Some(owner),
        BearWireEvent::ephemeral(
            "run.completed",
            json!({"text": "foreign-work-activity-sentinel"}),
        ),
    )
    .await
    .expect("append work activity");
    // Make the owner's session the latest candidate, so this exercises job
    // ownership rather than only the different-session-user filter.
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: owner,
            bear_id,
            bear_slug: slug.clone(),
            client_session_id: own_session.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{own_session}"),
            conversation_id: external_id.clone(),
            resolved_conversation_id: None,
            client: "bearwire-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("refresh owner's session");
    let foreign_job = rpc_value(state, &token, "conversation.surface_history", params).await;
    assert!(foreign_job.get("error").is_none(), "{foreign_job}");
    assert!(
        foreign_job["result"]["surface_events"]
            .to_string()
            .contains("visible canonical content"),
        "{foreign_job}"
    );
    assert!(
        !foreign_job.to_string().contains(&own_session),
        "foreign job session metadata leaked: {foreign_job}"
    );
    assert!(
        !foreign_job
            .to_string()
            .contains("foreign-work-activity-sentinel"),
        "foreign job activity leaked: {foreign_job}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_session_id_collision_cannot_mutate_victims_attached_work(pool: sqlx::PgPool) {
    let victim = create_test_user(&pool).await;
    let attacker = create_test_user(&pool).await;
    let (bear_id, bear_slug) = create_test_bear(&pool).await;
    let (other_bear_id, other_bear_slug) = create_test_bear(&pool).await;
    let victim_token = create_member_token(&pool, victim, bear_id).await;
    let attacker_token = create_member_token(&pool, attacker, bear_id).await;
    let other_bear_token = create_member_token(&pool, victim, other_bear_id).await;
    let state = test_state(pool.clone());
    let session_id = format!("shared-work-{}", Uuid::new_v4().simple());
    let opened = rpc_value(
        state.clone(),
        &victim_token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    let work_run_id = create_checkoutable_work_run_for_target(
        &pool,
        victim,
        bear_id,
        WorkExecutionTarget::AttachedArmature {
            client_session_id: session_id.clone(),
        },
    )
    .await;
    let attached = den_docket::work_runs::get_work_run(&pool, work_run_id)
        .await
        .expect("load victim work run")
        .expect("victim run attached");
    assert_eq!(attached.id, work_run_id);
    assert_eq!(attached.attachment_state.as_deref(), Some("attached"));

    for (token, slug) in [
        (&attacker_token, &bear_slug),
        (&other_bear_token, &other_bear_slug),
    ] {
        for method in ["session.open", "session.close", "run.start"] {
            let mut params = json!({"bear_slug": slug, "session_id": session_id});
            if method == "run.start" {
                params["prompt"] = json!("must not start");
            }
            let denial = rpc_value(state.clone(), token, method, params).await;
            assert!(
                denial["error"]["data"]["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Not Found"),
                "forged {method}: {denial}"
            );
            let unchanged = den_docket::work_runs::get_work_run(&pool, work_run_id)
                .await
                .expect("reload victim run")
                .expect("victim run still attached");
            assert_eq!(unchanged.id, work_run_id);
            assert_eq!(
                unchanged.attachment_state, attached.attachment_state,
                "forged {method} touched victim attachment"
            );
            assert_eq!(
                unchanged.state, attached.state,
                "forged {method} touched victim run state"
            );
        }
    }
    assert!(
        client_sessions::find_for_user_bear_session(&pool, attacker, &bear_slug, &session_id)
            .await
            .expect("find attacker session")
            .is_none()
    );
    assert!(client_sessions::find_for_user_bear_session(
        &pool,
        victim,
        &other_bear_slug,
        &session_id
    )
    .await
    .expect("find other Bear session")
    .is_none());

    let owner_closed = rpc_value(
        state.clone(),
        &victim_token,
        "session.close",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(owner_closed["result"]["closed"], true, "{owner_closed}");
    assert_eq!(
        owner_closed["result"]["attached_work_disconnected"], true,
        "{owner_closed}"
    );
    let disconnected = den_docket::work_runs::get_work_run(&pool, work_run_id)
        .await
        .expect("load disconnected run")
        .expect("victim run retained");
    assert_eq!(
        disconnected.attachment_state.as_deref(),
        Some("disconnected")
    );
    let forged_reopen = rpc_value(
        state.clone(),
        &attacker_token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert!(forged_reopen.get("error").is_some(), "{forged_reopen}");
    let still_disconnected = den_docket::work_runs::get_work_run(&pool, work_run_id)
        .await
        .expect("reload disconnected run")
        .expect("victim run retained");
    assert_eq!(
        still_disconnected.attachment_state,
        disconnected.attachment_state
    );
    let owner_reopened = rpc_value(
        state.clone(),
        &victim_token,
        "session.open",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(
        owner_reopened["result"]["attached_work_reconnected"], true,
        "{owner_reopened}"
    );
    let forged_close = rpc_value(
        state.clone(),
        &attacker_token,
        "session.close",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert!(forged_close.get("error").is_some(), "{forged_close}");
    let still_attached = den_docket::work_runs::get_work_run(&pool, work_run_id)
        .await
        .expect("reload reconnected run")
        .expect("victim run retained");
    assert_eq!(still_attached.attachment_state.as_deref(), Some("attached"));
    let final_close = rpc_value(
        state,
        &victim_token,
        "session.close",
        json!({
            "bear_slug": bear_slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(
        final_close["result"]["attached_work_disconnected"], true,
        "{final_close}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_ambiguous_historical_session_id_denies_even_bear_admin(pool: sqlx::PgPool) {
    let admin = create_test_user(&pool).await;
    let member = create_test_user(&pool).await;
    let (bear_id, slug) = create_test_bear(&pool).await;
    let admin_token = create_token_for_bear(&pool, admin, bear_id).await;
    let member_token = create_member_token(&pool, member, bear_id).await;
    let state = test_state(pool.clone());
    let session_id = format!("ambiguous-{}", Uuid::new_v4().simple());
    let opened = rpc_value(
        state.clone(),
        &admin_token,
        "session.open",
        json!({
            "bear_slug": slug, "session_id": session_id,
        }),
    )
    .await;
    assert_eq!(opened["result"]["ok"], true, "{opened}");
    // Seed a pre-migration collision, then restore the database guard before any RPC.
    sqlx::query!("ALTER TABLE client_sessions DISABLE TRIGGER client_sessions_global_owner_guard")
        .execute(&pool)
        .await
        .unwrap();
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: member,
            bear_id,
            bear_slug: slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: format!("bearwire:{bear_id}:{session_id}"),
            conversation_id: format!("den-conv-{}", Uuid::new_v4().simple()),
            resolved_conversation_id: None,
            client: "bearwire-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("seed historical duplicate");
    sqlx::query!("ALTER TABLE client_sessions ENABLE TRIGGER client_sessions_global_owner_guard")
        .execute(&pool)
        .await
        .unwrap();
    for (token, label) in [(&admin_token, "admin"), (&member_token, "member")] {
        for method in ["session.open", "session.close", "run.start"] {
            let mut params = json!({"bear_slug": slug, "session_id": session_id});
            if method == "run.start" {
                params["prompt"] = json!("must not reuse ambiguous run");
            }
            let denial = rpc_value(state.clone(), token, method, params).await;
            assert!(
                denial["error"]["data"]["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Not Found"),
                "{label} {method} must fail closed: {denial}"
            );
        }
    }
    let admin_session =
        client_sessions::find_for_user_bear_session(&pool, admin, &slug, &session_id)
            .await
            .expect("reload admin session")
            .expect("admin session retained");
    assert!(
        admin_session.closed_at.is_none(),
        "denied close must not mark admin session closed"
    );
}
