use super::*;
use serde_json::Value;
#[path = "support.rs"]
mod support;
use std::{sync::atomic::Ordering, time::Duration};
use support::{Fixture, Reply};

async fn complete(turn: &mut NativeChannelTurn) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), turn.next_event())
            .await
            .expect("bounded channel event")
            .expect("terminal event")
            .expect("runtime event");
        if matches!(
            event,
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. })
        ) {
            assert_eq!(
                turn.terminal().unwrap().outcome,
                ChannelTurnOutcome::Completed
            );
            break;
        }
    }
}
async fn rows(
    pool: &PgPool,
    conversation: Uuid,
    projection: ConversationHistoryProjection,
) -> Vec<persistence::PersistedConversationMessage> {
    let mut rows =
        persistence::list_projected_messages_page(pool, conversation, None, 100, projection)
            .await
            .unwrap();
    rows.reverse();
    rows
}
async fn interaction_rows(
    pool: &PgPool,
    conversation: Uuid,
) -> Vec<persistence::PersistedConversationMessage> {
    let mut rows =
        crate::channel_lifecycle::interaction_history_page(pool, conversation, None, 100)
            .await
            .unwrap();
    rows.reverse();
    rows
}

fn text_count(request: &Value, role: &str, text: &str) -> usize {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == role && message["content"] == text)
        .count()
}

#[sqlx::test(migrations = "../../migrations")]
async fn captured_first_and_second_requests_and_history_have_exactly_one_current_input_and_answer(
    pool: PgPool,
) {
    let fixture = Fixture::new(
        &pool,
        vec![Reply::Text("first answer"), Reply::Text("second answer")],
    )
    .await;
    let mut first = fixture.start(&pool, "first question").await;
    complete(&mut first).await;
    drop(first);
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::UserHistory,
    )
    .await;
    assert_eq!(
        history.len(),
        2,
        "only one canonical user and one assistant row"
    );
    assert_eq!(history[0].content_text, "first question");
    assert_eq!(history[1].content_text, "first answer");
    let mut second = fixture.start(&pool, "second question").await;
    complete(&mut second).await;
    drop(second);
    let requests = fixture.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(text_count(&requests[0], "user", "first question"), 1);
    assert_eq!(text_count(&requests[1], "user", "first question"), 1);
    assert_eq!(text_count(&requests[1], "assistant", "first answer"), 1);
    assert_eq!(text_count(&requests[1], "user", "second question"), 1);
    drop(requests);
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::UserHistory,
    )
    .await;
    assert_eq!(history.len(), 4);
    let interaction = interaction_rows(&pool, fixture.canonical).await;
    assert_eq!(
        interaction
            .iter()
            .filter_map(turn_lifecycle::terminal_record)
            .count(),
        2
    );
    assert_eq!(
        rows(
            &pool,
            fixture.canonical,
            ConversationHistoryProjection::ModelTranscript
        )
        .await
        .len(),
        4,
        "lifecycle records must never enter model replay"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn completed_tool_exchange_is_awaited_before_terminal_and_next_model_request(pool: PgPool) {
    let fixture = Fixture::new(
        &pool,
        vec![
            Reply::Tools,
            Reply::Text("after tool"),
            Reply::Text("next answer"),
        ],
    )
    .await;
    let mut turn = fixture.start(&pool, "inspect session").await;
    complete(&mut turn).await;
    drop(turn);
    assert_eq!(fixture.invoker.calls.load(Ordering::SeqCst), 1);
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await;
    assert_eq!(
        history
            .iter()
            .filter(|row| row.tool_request_payload().unwrap().is_some())
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|row| row.tool_result_payload().unwrap().is_some())
            .count(),
        1
    );
    let mut next = fixture.start(&pool, "follow up").await;
    complete(&mut next).await;
    drop(next);
    let requests = fixture.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let messages = requests[2]["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(
                |message| message["role"] == "tool" && message["tool_call_id"] == "completed-call"
            )
            .count(),
        1
    );
    assert_eq!(text_count(&requests[2], "assistant", "after tool"), 1);
    assert_eq!(text_count(&requests[2], "user", "follow up"), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancellation_awaits_completed_results_and_marks_only_unfinished_tools_incomplete(
    pool: PgPool,
) {
    let fixture = Fixture::new(
        &pool,
        vec![Reply::PartialTools, Reply::Text("after cancellation")],
    )
    .await;
    let mut turn = fixture.start(&pool, "do two reads").await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                ()=fixture.invoker.pending.notified()=>break,
                event=turn.next_event()=>{event.expect("stream still running").unwrap();}
            }
        }
    })
    .await
    .unwrap();
    let terminal = turn
        .stop(ChannelTurnOutcome::Cancelled, None)
        .await
        .unwrap();
    assert_eq!(terminal.outcome, ChannelTurnOutcome::Cancelled);
    drop(turn);
    let interaction = interaction_rows(&pool, fixture.canonical).await;
    let model_history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await;
    let results: Vec<_> = model_history
        .iter()
        .filter_map(|row| row.tool_result_payload().unwrap())
        .collect();
    assert_eq!(results.len(), 2, "interaction rows: {interaction:?}");
    let completed = fixture
        .invoker
        .completed_call
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let pending = fixture
        .invoker
        .pending_call
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert!(results.iter().any(|result| result.tool_call_id.as_deref()
        == Some(completed.as_str())
        && result.status == den_core::tools::result_compaction::ToolResultStatus::Ok));
    assert!(results.iter().any(
        |result| result.tool_call_id.as_deref() == Some(pending.as_str())
            && result.status == den_core::tools::result_compaction::ToolResultStatus::Incomplete
    ));
    assert_eq!(
        interaction
            .iter()
            .filter_map(turn_lifecycle::terminal_record)
            .next_back()
            .unwrap()
            .outcome,
        ChannelTurnOutcome::Cancelled
    );
    assert_eq!(fixture.invoker.calls.load(Ordering::SeqCst), 2);
    let mut next = fixture
        .start(&pool, "continue conversation, not tools")
        .await;
    complete(&mut next).await;
    drop(next);
    assert_eq!(
        fixture.invoker.calls.load(Ordering::SeqCst),
        2,
        "no abandoned tool may be re-executed"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn runtime_timeout_preserves_completed_batch_results_and_partial_text(pool: PgPool) {
    let fixture = Fixture::new(&pool, vec![Reply::PartialTools]).await;
    let mut turn = fixture.start(&pool, "time out while reading").await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                () = fixture.invoker.pending.notified() => break,
                event = turn.next_event() => { event.expect("active stream").unwrap(); }
            }
        }
    })
    .await
    .unwrap();
    turn.stream.expire_deadline_for_test();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), turn.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(
            event,
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed { .. })
        ) {
            break;
        }
    }
    assert_eq!(turn.terminal().unwrap().outcome, ChannelTurnOutcome::Failed);
    drop(turn);
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await;
    let completed = fixture
        .invoker
        .completed_call
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert!(history
        .iter()
        .filter_map(|row| row.tool_result_payload().unwrap())
        .any(
            |result| result.tool_call_id.as_deref() == Some(completed.as_str())
                && result.status == den_core::tools::result_compaction::ToolResultStatus::Ok
        ));
    assert_eq!(
        history
            .iter()
            .filter(|row| row.content_text == "Working…")
            .count(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_text_cancel_failure_and_empty_success_have_canonical_terminal_records(pool: PgPool) {
    let fixture = Fixture::new(&pool, vec![Reply::Empty, Reply::Failure]).await;
    let mut cancelled = fixture.start(&pool, "cancel before text").await;
    cancelled
        .stop(ChannelTurnOutcome::Cancelled, None)
        .await
        .unwrap();
    drop(cancelled);
    let mut empty = fixture.start(&pool, "empty completion").await;
    complete(&mut empty).await;
    drop(empty);
    let mut failed = fixture.start(&pool, "provider failure").await;
    loop {
        match tokio::time::timeout(Duration::from_secs(10), failed.next_event())
            .await
            .unwrap()
        {
            Some(Ok(RuntimeStreamEvent::Semantic(
                RuntimeSemanticEvent::Error { .. } | RuntimeSemanticEvent::TurnFailed { .. },
            )))
            | Some(Err(_)) => break,
            Some(Ok(_)) => {}
            None => panic!("failure must be explicit"),
        }
    }
    assert_eq!(
        failed.terminal().unwrap().outcome,
        ChannelTurnOutcome::Failed
    );
    drop(failed);
    let visible = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::UserHistory,
    )
    .await;
    assert_eq!(visible.len(), 3, "no synthetic empty assistant messages");
    let interaction = interaction_rows(&pool, fixture.canonical).await;
    let terminal: Vec<_> = interaction
        .iter()
        .filter_map(turn_lifecycle::terminal_record)
        .collect();
    assert_eq!(
        terminal
            .iter()
            .map(|terminal| terminal.outcome)
            .collect::<Vec<_>>(),
        vec![
            ChannelTurnOutcome::Cancelled,
            ChannelTurnOutcome::Completed,
            ChannelTurnOutcome::Failed
        ]
    );
    assert!(terminal
        .iter()
        .all(|terminal| terminal.assistant_message_id.is_none()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_source_lease_blocks_armature_and_browser_admission_until_finalization(
    pool: PgPool,
) {
    let fixture = Fixture::new(&pool, vec![Reply::Text("one controller")]).await;
    let mut turn = fixture.start(&pool, "native owns this source").await;
    assert!(
        ConversationTurnLease::acquire(&pool, fixture.canonical)
            .await
            .is_err(),
        "browser acquires this exact shared lease before writing input"
    );
    let source = &fixture.source;
    let binding = den_protocol::RoleRuntimeBinding {
        binding_id: den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(
            fixture.canonical,
        )
        .binding_id(source.bear_id.into()),
        compatibility_backend: Some("native".into()),
    };
    let request = crate::turn_runner::TurnStartRequest {
        sqlx_pool: &pool,
        config: &fixture.config,
        memory_stores: &fixture.stores,
        request_id: Uuid::new_v4(),
        run_id: Some("competing-armature"),
        checkpoint_audit_context: None,
        user_id: source.user_id.unwrap(),
        session_id: &source.client_session_id,
        bear_id: source.bear_id,
        bear_slug: &source.bear_slug,
        client: "test-armature",
        cwd: None,
        workspace_roots: None,
        binding: &binding,
        conversation_selection: &source.conversation_id,
        upstream_target: &source.conversation_id,
        prompt: "must not be written",
        prompt_context: None,
        client_tools: None,
        runtime_context: None,
        runtime_context_len: 0,
        stream_tokens: true,
        api_style: None,
        supports_reasoning_effort: Some(false),
        technical_budget_recovery_start_payload: None,
    };
    assert!(
        super::super::turn::start_native_turn_event_stream(request, source.origin)
            .await
            .is_err()
    );
    assert_eq!(
        fixture.provider.requests.lock().unwrap().len(),
        0,
        "a blocked competitor must not issue a model request"
    );
    complete(&mut turn).await;
    drop(turn);
    let lease = ConversationTurnLease::acquire(&pool, fixture.canonical)
        .await
        .unwrap();
    drop(lease);
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::UserHistory,
    )
    .await;
    assert!(!history
        .iter()
        .any(|row| row.content_text == "must not be written"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn waiting_run_lease_survives_registry_removal_until_live_stream_releases_it(pool: PgPool) {
    let source = Uuid::new_v4();
    let session = format!("lease-session-{}", Uuid::new_v4());
    let lease = ConversationTurnLease::acquire(&pool, source).await.unwrap();
    super::super::source_leases::insert(&session, "waiting-run", lease.clone());
    let live = super::super::source_leases::get(&session, Some("waiting-run")).unwrap();
    drop(lease);
    assert!(ConversationTurnLease::acquire(&pool, source).await.is_err());
    super::super::source_leases::remove(&session, "waiting-run");
    assert!(
        ConversationTurnLease::acquire(&pool, source).await.is_err(),
        "cancellation cannot release an old controller's live lease"
    );
    drop(live);
    assert!(ConversationTurnLease::acquire(&pool, source).await.is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn detached_armature_transcript_write_retains_exclusion_after_controller_exit(pool: PgPool) {
    let fixture = Fixture::new(&pool, Vec::new()).await;
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let lease = ConversationTurnLease::acquire(&pool, fixture.canonical)
        .await
        .unwrap();
    let blocker = single.acquire().await.unwrap();
    crate::agent_loop::spawn_persist_native_agent_step(
        single.clone(),
        fixture.source.bear_id,
        fixture.source.user_id,
        fixture.source.conversation_id.clone(),
        fixture.source.client_session_id.clone(),
        Some(Uuid::new_v4().to_string()),
        "detached canonical answer".into(),
        &[],
        None,
        Some(lease),
    );
    assert!(ConversationTurnLease::acquire(&pool, fixture.canonical)
        .await
        .is_err());
    drop(blocker);
    let next = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(lease) = ConversationTurnLease::acquire(&pool, fixture.canonical).await {
                break lease;
            }
        }
    })
    .await
    .unwrap();
    let history = rows(
        &pool,
        fixture.canonical,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await;
    assert_eq!(
        history
            .iter()
            .filter(|row| row.content_text == "detached canonical answer")
            .count(),
        1
    );
    drop(next);
}

#[sqlx::test(migrations = "../../migrations")]
async fn source_lease_does_not_starve_a_single_connection_pool(pool: PgPool) {
    let fixture = Fixture::new(&pool, Vec::new()).await;
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let lease = ConversationTurnLease::acquire(&single, fixture.canonical)
        .await
        .unwrap();
    let record = tokio::time::timeout(
        Duration::from_secs(2),
        persistence::get_conversation_by_id(&single, fixture.canonical),
    )
    .await
    .expect("lease must not occupy the only query slot")
    .unwrap();
    assert!(record.is_some());
    drop(lease);
}
