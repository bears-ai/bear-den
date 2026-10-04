use super::*;
use crate::agent_loop::source_admission::tests::{
    bind_client, fixture, step_context, work_fixture,
};
use den_service::{bears::db, client_sessions, conversation::persistence};

async fn assert_denied_before_effects(
    pool: &PgPool,
    session: &AgentLoopSession,
    binding: &RoleRuntimeBinding,
) {
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    SESSION_STORE.insert(session.clone());
    let approval = crate::agent_loop::create_native_approval(
        pool,
        session.bear_id,
        &session.conversation_id,
        &session.client_session_id,
        "source-call",
        "fs_read_text_file",
        &serde_json::json!({"path": "/workspace"}),
    )
    .await
    .unwrap();
    for continuation in [
        RuntimeContinuation::ToolResult {
            tool_call_id: "source-call".into(),
            approval_request_id: Some(approval.clone()),
            status: RuntimeToolResultStatus::Ok,
            content: "must not enter transcript".into(),
        },
        RuntimeContinuation::ApprovalDecision {
            approval_request_id: approval.clone(),
            tool_call_id: Some("source-call".into()),
            decision: den_protocol::RuntimeApprovalDecision::Approve,
            reason: None,
        },
        RuntimeContinuation::ApprovalDecision {
            approval_request_id: approval.clone(),
            tool_call_id: Some("source-call".into()),
            decision: den_protocol::RuntimeApprovalDecision::Deny,
            reason: None,
        },
        RuntimeContinuation::DocketBoundedSlice,
    ] {
        let result = continue_native_client_turn_event_stream(TurnContinueRequest {
            sqlx_pool: pool,
            config: &config,
            memory_stores: &stores,
            request_id: Uuid::new_v4(),
            run_id: session.run_id.as_deref(),
            client_session_id: &session.client_session_id,
            conversation: RuntimeConversationRef {
                id: session.conversation_id.clone(),
            },
            binding,
            continuation,
            stream_context: crate::turn_runner::default_tool_continue_stream_context(),
        })
        .await;
        assert!(matches!(
            result,
            Err(DenError::Authorization(_)) | Err(DenError::NotFound(_))
        ));
        let stored = SESSION_STORE.get(&session.session_key).unwrap();
        assert_eq!(stored.messages.len(), session.messages.len());
        assert_eq!(stored.request_id, session.request_id);
        assert_eq!(stored.step, session.step);
        assert!(stored.latest_context_budget.is_none());
    }
    let denied = record_native_client_tool_result(
        pool,
        &session.conversation_id,
        &session.client_session_id,
        "denied-request",
        session.run_id.as_deref(),
        "source-call",
        Some(&approval),
        RuntimeToolResultStatus::Ok,
        "must not persist".into(),
    )
    .await;
    assert!(matches!(
        denied,
        Err(DenError::Authorization(_)) | Err(DenError::NotFound(_))
    ));
    assert_eq!(
        SESSION_STORE.get(&session.session_key).unwrap().request_id,
        session.request_id
    );
    assert_eq!(
        SESSION_STORE
            .get(&session.session_key)
            .unwrap()
            .messages
            .len(),
        session.messages.len()
    );

    assert!(crate::agent_loop::load_transcript_messages(
        pool,
        session.bear_id,
        &session.conversation_id
    )
    .await
    .unwrap()
    .is_empty());
    let context = step_context(pool, session);
    assert!(
        run_agent_step_stream(&LlmClient::new(&config), session, Some(context.clone()))
            .await
            .is_err()
    );
    assert!(context
        .session_store
        .get(&session.session_key)
        .unwrap()
        .latest_context_budget
        .is_none());
    SESSION_STORE.remove(&session.session_key);
}

#[sqlx::test(migrations = "../../migrations")]
async fn all_continuations_and_result_recording_recheck_membership_rebinding_and_closure(
    pool: PgPool,
) {
    let (mut session, canonical, _) = fixture(&pool).await;
    // Even a terminal budget boundary cannot bypass continuation admission.
    session.step = session.turn_budget.emergency_hard_steps;
    let binding = RoleRuntimeBinding {
        binding_id: NativeTurnSource::Conversation(canonical).binding_id(session.bear_id.into()),
        compatibility_backend: Some("native".into()),
    };
    db::revoke_membership(&pool, session.user_id.unwrap(), session.bear_id)
        .await
        .unwrap();
    assert_denied_before_effects(&pool, &session, &binding).await;
    db::grant_membership(
        &pool,
        session.user_id.unwrap(),
        session.bear_id,
        Some(db::BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    bind_client(&pool, &session, "another-conversation").await;
    assert_denied_before_effects(&pool, &session, &binding).await;
    bind_client(&pool, &session, &session.conversation_id).await;
    let client = client_sessions::find_for_user_bear_session_id(
        &pool,
        session.user_id.unwrap(),
        session.bear_id,
        &session.client_session_id,
    )
    .await
    .unwrap()
    .unwrap();
    client_sessions::mark_closed(&pool, client.id)
        .await
        .unwrap();
    assert_denied_before_effects(&pool, &session, &binding).await;
    bind_client(&pool, &session, &session.conversation_id).await;
    sqlx::query!(
        "UPDATE conversations SET status = 'archived' WHERE id = $1",
        canonical
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied_before_effects(&pool, &session, &binding).await;
    // Browser/channel loops have no editor row to check, but still check canonical closure.
    for origin in [
        den_core::TurnExecutionOrigin::ChannelConversation,
        den_core::TurnExecutionOrigin::BrowserTaskSession,
    ] {
        session.origin = origin;
        session.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, session.governance).context_label;
        assert_denied_before_effects(&pool, &session, &binding).await;
    }
    assert!(persistence::list_messages_page(&pool, canonical, None, 100)
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn zero_hat_replacement_source_cannot_replay_a_stored_turn(pool: PgPool) {
    let (session, canonical, _) = fixture(&pool).await;
    let binding = RoleRuntimeBinding {
        binding_id: NativeTurnSource::Conversation(canonical).binding_id(session.bear_id.into()),
        compatibility_backend: Some("native".into()),
    };
    persistence::delete_conversation_for_external_id(
        &pool,
        session.bear_id,
        &session.conversation_id,
    )
    .await
    .unwrap();
    let replacement = persistence::ensure_conversation_for_external_id(
        &pool,
        session.bear_id,
        session.user_id,
        &session.conversation_id,
        None,
        None,
    )
    .await
    .unwrap();
    assert_ne!(replacement.id, canonical);
    assert_denied_before_effects(&pool, &session, &binding).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_work_hat_exact_run_and_cancel_are_checked_before_every_continuation_effect(
    pool: PgPool,
) {
    let (session, hat) = work_fixture(&pool).await;
    let binding = RoleRuntimeBinding {
        binding_id: NativeTurnSource::WorkRun(session.work_run_id.unwrap())
            .binding_id(session.bear_id.into()),
        compatibility_backend: Some("native".into()),
    };
    let mut replay = session.clone();
    replay.work_run_id = Some(Uuid::new_v4());
    assert_denied_before_effects(&pool, &replay, &binding).await;
    den_service::bears::hats::manage::disable_work(&pool, session.bear_id.into(), hat)
        .await
        .unwrap();
    assert_denied_before_effects(&pool, &session, &binding).await;
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(work_runs::request_work_run_cancel(
        &pool,
        session.work_run_id.unwrap(),
        session.bear_id
    )
    .await
    .unwrap());
    assert_denied_before_effects(&pool, &session, &binding).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn replayed_binding_is_denied_for_every_continuation_kind_but_live_hat_preserves_context(
    pool: PgPool,
) {
    let (mut session, canonical, _) = fixture(&pool).await;
    session.step = session.turn_budget.emergency_hard_steps;
    session.messages.push(ChatMessage {
        role: "system".into(),
        content: Some("verified live hat context".into()),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    SESSION_STORE.insert(session.clone());
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let mut binding = RoleRuntimeBinding {
        binding_id: NativeTurnSource::Conversation(Uuid::new_v4())
            .binding_id(session.bear_id.into()),
        compatibility_backend: Some("native".into()),
    };
    for continuation in [
        RuntimeContinuation::DocketBoundedSlice,
        RuntimeContinuation::ApprovalDecision {
            approval_request_id: "must-not-touch".into(),
            tool_call_id: None,
            decision: den_protocol::RuntimeApprovalDecision::Approve,
            reason: None,
        },
        RuntimeContinuation::ToolResult {
            tool_call_id: "source-call".into(),
            approval_request_id: None,
            status: RuntimeToolResultStatus::Ok,
            content: "replayed".into(),
        },
    ] {
        assert!(matches!(
            continue_native_client_turn_event_stream(TurnContinueRequest {
                sqlx_pool: &pool,
                config: &config,
                memory_stores: &stores,
                request_id: Uuid::new_v4(),
                run_id: session.run_id.as_deref(),
                client_session_id: &session.client_session_id,
                conversation: RuntimeConversationRef {
                    id: session.conversation_id.clone()
                },
                binding: &binding,
                continuation,
                stream_context: crate::turn_runner::default_tool_continue_stream_context(),
            })
            .await,
            Err(DenError::Authorization(_))
        ));
        assert_eq!(
            SESSION_STORE
                .get(&session.session_key)
                .unwrap()
                .messages
                .len(),
            1
        );
    }
    binding.binding_id =
        NativeTurnSource::Conversation(canonical).binding_id(session.bear_id.into());
    let (_, mut stream) = continue_native_client_turn_event_stream(TurnContinueRequest {
        sqlx_pool: &pool,
        config: &config,
        memory_stores: &stores,
        request_id: Uuid::new_v4(),
        run_id: session.run_id.as_deref(),
        client_session_id: &session.client_session_id,
        conversation: RuntimeConversationRef {
            id: session.conversation_id.clone(),
        },
        binding: &binding,
        continuation: RuntimeContinuation::ToolResult {
            tool_call_id: "source-call".into(),
            approval_request_id: None,
            status: RuntimeToolResultStatus::Ok,
            content: "live result".into(),
        },
        stream_context: crate::turn_runner::default_tool_continue_stream_context(),
    })
    .await
    .unwrap();
    assert!(stream.next().await.unwrap().is_ok());
    let stored = SESSION_STORE.get(&session.session_key).unwrap();
    assert_eq!(
        stored.messages[0].content.as_deref(),
        Some("verified live hat context")
    );
    assert!(stored
        .messages
        .iter()
        .any(|message| message.content.as_deref() == Some("live result")));
    SESSION_STORE.remove(&session.session_key);
}
