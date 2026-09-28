use den_core::{
    config::Config,
    ids::{BearId, UserId},
    DenError,
};
use den_memory::MemoryStoreManager;
use den_protocol::{
    RoleRuntimeBinding, RuntimeConversationBackend, RuntimeConversationRef, RuntimeHistoryPage,
};
use den_runtime::{
    native_runtime::NativeRuntimeConversationBackend,
    turn_runner::{materialize_runtime_conversation_if_needed, TurnStartRequest},
};
use den_service::{
    bears::db::{create_bear, grant_membership, revoke_membership, BearParams, BEAR_ROLE_MEMBER},
    client_sessions,
    conversation::{persistence::ensure_conversation_for_external_id, viewer::ConversationViewer},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn setup(pool: &PgPool) -> (i32, i32, Uuid, String) {
    let suffix = Uuid::new_v4().simple().to_string();
    let slug = format!("turn-{}", &suffix[..16]);
    let bear_id = create_bear(
        pool,
        BearParams {
            slug: &slug,
            name: "Turn materialization",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut users = Vec::new();
    for index in 0..2 {
        let username = format!("turn{index}{}", &suffix[..16]);
        let user_id = sqlx::query_scalar!(
            "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
            username
        )
        .fetch_one(pool)
        .await
        .unwrap();
        grant_membership(pool, user_id, bear_id, Some(BEAR_ROLE_MEMBER))
            .await
            .unwrap();
        users.push(user_id);
    }
    (users[0], users[1], bear_id, slug)
}

fn request<'a>(
    pool: &'a PgPool,
    config: &'a Config,
    stores: &'a MemoryStoreManager,
    binding: &'a RoleRuntimeBinding,
    user_id: i32,
    bear_id: Uuid,
    slug: &'a str,
    session_id: &'a str,
    upstream_target: &'a str,
) -> TurnStartRequest<'a> {
    TurnStartRequest {
        sqlx_pool: pool,
        config,
        memory_stores: stores,
        request_id: Uuid::new_v4(),
        run_id: None,
        checkpoint_audit_context: None,
        user_id,
        session_id,
        bear_id,
        bear_slug: slug,
        client: "acp-zed",
        cwd: None,
        workspace_roots: None,
        binding,
        conversation_selection: "new-acp-zed-pending",
        upstream_target,
        prompt: "hello",
        prompt_context: None,
        client_tools: None,
        runtime_context: None,
        runtime_context_len: 0,
        technical_budget_recovery_start_payload: None,
        stream_tokens: false,
        api_style: None,
        supports_reasoning_effort: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_acp_turn_is_owned_and_reconnection_can_read_it(pool: PgPool) {
    let (user_id, _, bear_id, slug) = setup(&pool).await;
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let binding = RoleRuntimeBinding {
        binding_id: format!("den-native:{bear_id}:pair"),
        compatibility_backend: Some("runtime:native".to_string()),
    };
    let backend = NativeRuntimeConversationBackend::with_pool(pool.clone());
    let generated = backend.create_conversation(&binding).await.unwrap();
    assert!(generated.id.starts_with("den-conv-"));
    assert!(
        den_service::conversation::persistence::get_conversation_for_external_id(
            &pool,
            bear_id,
            &generated.id,
        )
        .await
        .unwrap()
        .is_none()
    );
    let other_binding = RoleRuntimeBinding {
        binding_id: "other-runtime-binding".to_string(),
        compatibility_backend: None,
    };
    let other_generated = backend.create_conversation(&other_binding).await.unwrap();
    assert!(other_generated.id.starts_with("den-conv-"));
    assert!(
        den_service::conversation::persistence::get_conversation_for_external_id(
            &pool,
            bear_id,
            &other_generated.id,
        )
        .await
        .unwrap()
        .is_none()
    );

    let session_id = format!("session-{}", Uuid::new_v4());
    let materialized = materialize_runtime_conversation_if_needed(
        &backend,
        &request(
            &pool,
            &config,
            &stores,
            &binding,
            user_id,
            bear_id,
            &slug,
            &session_id,
            "new-acp-zed-pending",
        ),
    )
    .await
    .unwrap();
    assert!(materialized.created);
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool,
        bear_id,
        &materialized.conversation_id,
    )
    .await
    .unwrap()
    .unwrap();
    let owner = sqlx::query_scalar!(
        "SELECT created_by_user_id FROM conversations WHERE id = $1",
        canonical.id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner, Some(user_id));

    let restored =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, &session_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        restored.resolved_conversation_id.as_deref(),
        Some(materialized.conversation_id.as_str())
    );
    let viewer = ConversationViewer::resolve(&pool, BearId::new(bear_id), UserId::new(user_id))
        .await
        .unwrap()
        .unwrap();
    assert!(viewer
        .may_access_external(&pool, &materialized.conversation_id)
        .await
        .unwrap());
    backend
        .verify_conversation_belongs_to_binding(&binding, &materialized.conversation_id)
        .await
        .unwrap();
    let reconnected = materialize_runtime_conversation_if_needed(
        &backend,
        &request(
            &pool,
            &config,
            &stores,
            &binding,
            user_id,
            bear_id,
            &slug,
            &session_id,
            restored.resolved_conversation_id.as_deref().unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(!reconnected.created);
    assert_eq!(reconnected.conversation_id, materialized.conversation_id);
}

struct CollidingBackend(String);

#[allow(async_fn_in_trait)]
impl RuntimeConversationBackend for CollidingBackend {
    async fn create_conversation(
        &self,
        _: &RoleRuntimeBinding,
    ) -> Result<RuntimeConversationRef, DenError> {
        Ok(RuntimeConversationRef { id: self.0.clone() })
    }

    async fn verify_conversation_belongs_to_binding(
        &self,
        _: &RoleRuntimeBinding,
        _: &str,
    ) -> Result<(), DenError> {
        Ok(())
    }

    async fn load_history(
        &self,
        _: &RoleRuntimeBinding,
        _: &RuntimeConversationRef,
    ) -> Result<RuntimeHistoryPage, DenError> {
        Ok(RuntimeHistoryPage {
            records: vec![],
            raw_payload: None,
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn collision_and_revoked_membership_never_bind_a_session(pool: PgPool) {
    let (owner, other, bear_id, slug) = setup(&pool).await;
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let binding = RoleRuntimeBinding {
        binding_id: format!("den-native:{bear_id}:pair"),
        compatibility_backend: None,
    };
    let id = format!("den-conv-{}", Uuid::new_v4().simple());
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), &id, None, None)
        .await
        .unwrap();
    let backend = CollidingBackend(id.clone());
    let session_id = format!("session-{}", Uuid::new_v4());
    let result = materialize_runtime_conversation_if_needed(
        &backend,
        &request(
            &pool,
            &config,
            &stores,
            &binding,
            other,
            bear_id,
            &slug,
            &session_id,
            "new-acp-zed-pending",
        ),
    )
    .await;
    assert!(matches!(result, Err(DenError::Authorization(_))));
    assert!(
        client_sessions::find_for_user_bear_session_id(&pool, other, bear_id, &session_id)
            .await
            .unwrap()
            .is_none()
    );
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        &pool, bear_id, &id,
    )
    .await
    .unwrap()
    .unwrap();
    let owner_after = sqlx::query_scalar!(
        "SELECT created_by_user_id FROM conversations WHERE id = $1",
        canonical.id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner_after, Some(owner));

    revoke_membership(&pool, owner, bear_id).await.unwrap();
    let result = materialize_runtime_conversation_if_needed(
        &backend,
        &request(
            &pool,
            &config,
            &stores,
            &binding,
            owner,
            bear_id,
            &slug,
            &session_id,
            "new-acp-zed-pending",
        ),
    )
    .await;
    assert!(matches!(result, Err(DenError::Authorization(_))));
    assert!(
        client_sessions::find_for_user_bear_session_id(&pool, owner, bear_id, &session_id)
            .await
            .unwrap()
            .is_none()
    );
}
