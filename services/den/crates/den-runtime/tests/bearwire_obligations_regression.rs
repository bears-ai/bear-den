use bearwire_protocol::wire::BearWireEvent;
use den_runtime::{
    bearwire_events,
    surface_projection::{project_obligation_for_surface, SurfaceActionKind, TurnSurfaceKind},
    turn_obligations::{self, ExpectedResponderAction, TurnObligationKind},
    turn_runs, turn_steps, turn_waits,
};
use sqlx::Row;
use uuid::Uuid;

async fn create_user_and_bear(pool: &sqlx::PgPool) -> (i32, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("obl{}", &suffix[..16]);
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
    .bind("Obligation Test")
    .bind("test-passhash")
    .fetch_one(pool)
    .await
    .expect("create test user");

    let bear_id = Uuid::new_v4();
    let slug = format!("bear-{}", &suffix[..16]);
    sqlx::query(
        r"
        INSERT INTO bears (id, slug, name)
        VALUES ($1, $2, $3)
        ",
    )
    .bind(bear_id)
    .bind(slug)
    .bind("Obligation Test Bear")
    .execute(pool)
    .await
    .expect("create test bear");

    (user_id, bear_id)
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_session_bearwire_appends_are_commit_order_visible(pool: sqlx::PgPool) {
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let mut tx = pool.begin().await.expect("begin transaction");
    let first = bearwire_events::append_bearwire_event_on(
        &mut tx,
        &session_id,
        None,
        None,
        BearWireEvent::ephemeral("run.progress", serde_json::json!({ "order": "first" })),
    )
    .await
    .expect("append first event in transaction");

    let pool_for_task = pool.clone();
    let session_for_task = session_id.clone();
    let second_task = tokio::spawn(async move {
        bearwire_events::append_bearwire_event(
            &pool_for_task,
            &session_for_task,
            None,
            None,
            BearWireEvent::ephemeral("run.progress", serde_json::json!({ "order": "second" })),
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !second_task.is_finished(),
        "autocommit append must wait for same-session transactional append to commit"
    );

    let visible_before_commit =
        bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
            .await
            .expect("list before commit");
    assert!(
        visible_before_commit.is_empty(),
        "uncommitted lower-sequence event and blocked higher-sequence event must not be visible"
    );

    tx.commit().await.expect("commit first append");
    let second = second_task
        .await
        .expect("join second append")
        .expect("second append succeeds");
    assert!(second.sequence_no > first.sequence_no);

    let visible = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list visible events");
    assert_eq!(visible.len(), 2);
    assert_eq!(visible[0].sequence_no, first.sequence_no);
    assert_eq!(visible[0].event.data["order"], "first");
    assert_eq!(visible[1].sequence_no, second.sequence_no);
    assert_eq!(visible[1].event.data["order"], "second");
}

#[sqlx::test(migrations = "../../migrations")]
async fn permission_obligation_promotes_existing_tool_obligation(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let tool_call_id = "call-promote";
    let permission_id = "perm-promote";

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let tool = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        tool_call_id,
        None,
        serde_json::json!({ "phase": "tool" }),
    )
    .await
    .expect("create tool obligation");

    let permission = turn_obligations::upsert_permission_decision_obligation(
        &pool,
        &run_id,
        &session_id,
        permission_id,
        Some(tool_call_id),
        serde_json::json!({ "phase": "permission" }),
    )
    .await
    .expect("promote to permission obligation");

    assert_eq!(permission.id, tool.id);
    assert_eq!(permission.kind, "permission_decision");
    assert_eq!(permission.expected_responder_action, "permission_decision");
    assert_eq!(permission.tool_call_id.as_deref(), Some(tool_call_id));
    assert_eq!(permission.permission_id.as_deref(), Some(permission_id));

    let by_permission = turn_obligations::get_permission_obligation(&pool, &run_id, permission_id)
        .await
        .expect("load by permission id")
        .expect("permission obligation exists");
    assert_eq!(by_permission.id, permission.id);

    let waiting_for_tool = turn_obligations::mark_waiting_for_tool_result(&pool, permission.id)
        .await
        .expect("mark waiting for tool result")
        .expect("obligation still open");
    assert_eq!(waiting_for_tool.id, permission.id);
    assert_eq!(waiting_for_tool.kind, "tool_result");
    assert_eq!(waiting_for_tool.expected_responder_action, "tool_result");
    assert_eq!(waiting_for_tool.tool_call_id.as_deref(), Some(tool_call_id));
    assert_eq!(
        waiting_for_tool.permission_id.as_deref(),
        Some(permission_id)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn open_obligation_barrier_counts_only_unsettled_client_waits(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let first = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-first",
        None,
        serde_json::json!({ "tool": "first" }),
    )
    .await
    .expect("create first obligation");
    let second = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-second",
        None,
        serde_json::json!({ "tool": "second" }),
    )
    .await
    .expect("create second obligation");

    turn_obligations::mark_result_received(&pool, first.id, serde_json::json!({ "status": "ok" }))
        .await
        .expect("mark first received")
        .expect("first still open before receive");

    let open = turn_obligations::open_client_obligations_for_run(&pool, &run_id)
        .await
        .expect("list open obligations");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, second.id);
    assert_eq!(open[0].tool_call_id.as_deref(), Some("call-second"));

    turn_obligations::mark_result_received(&pool, second.id, serde_json::json!({ "status": "ok" }))
        .await
        .expect("mark second received")
        .expect("second still open before receive");

    let open = turn_obligations::open_client_obligations_for_run(&pool, &run_id)
        .await
        .expect("list open obligations after all received");
    assert!(open.is_empty(), "all tool results are received: {open:#?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn armature_owned_tool_call_creates_client_obligation(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let persisted = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-list",
            tool_name: "fs_list_directory",
            title: Some("List directory"),
            kind: Some("read"),
            arguments: &serde_json::json!({ "path": "." }),
            approval_request_id: None,
            approval_required: false,
            approval_reason: None,
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect("persist armature-owned tool event");

    let obligation = persisted
        .obligation
        .expect("armature-owned tool should create a client tool-result obligation");
    assert_eq!(obligation.kind, "tool_result");
    assert_eq!(obligation.expected_responder_action, "tool_result");
    assert_eq!(obligation.tool_call_id.as_deref(), Some("call-list"));
    assert_eq!(
        obligation.request_payload["execution_target"],
        "armature_local"
    );
    assert_eq!(
        obligation.request_payload["policy"]["execution_target"],
        "armature_local"
    );
    assert_eq!(
        obligation.request_payload["policy"]["approval_policy"],
        "never"
    );
    assert_eq!(obligation.request_payload["policy"]["risk"], "read_only");

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "waiting_for_client");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_tool_execution_owner_fails_closed(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let err = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-unknown",
            tool_name: "mystery_unowned_tool",
            title: Some("Mystery"),
            kind: Some("function"),
            arguments: &serde_json::json!({}),
            approval_request_id: None,
            approval_required: false,
            approval_reason: None,
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect_err("unknown tool owner must fail closed");

    assert!(turn_waits::descriptor_resolution_failed(&err));
    let open = turn_obligations::open_client_obligations_for_session(&pool, &session_id)
        .await
        .expect("list open obligations");
    assert!(open.is_empty(), "unknown tool must not create client waits");
}

#[sqlx::test(migrations = "../../migrations")]
async fn den_owned_tool_call_does_not_create_client_obligation(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-title",
            tool_name: "set_conversation_title",
            title: Some("Set conversation title"),
            kind: Some("function"),
            arguments: &serde_json::json!({ "title": "Debug run" }),
            approval_request_id: None,
            approval_required: false,
            approval_reason: None,
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect("persist den-owned tool event");

    let open = turn_obligations::open_client_obligations_for_session(&pool, &session_id)
        .await
        .expect("list open obligations");
    assert!(
        open.is_empty(),
        "Den-owned display tools must not create armature client waits: {open:#?}"
    );

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "tool_call.requested"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn den_owned_approval_required_tool_creates_permission_obligation(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let persisted = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-web-fetch",
            tool_name: "web_fetch",
            title: Some("Fetch web page"),
            kind: Some("function"),
            arguments: &serde_json::json!({ "url": "https://example.com" }),
            approval_request_id: Some("perm-web-fetch"),
            approval_required: true,
            approval_reason: Some("web_fetch requires approval for this URL"),
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect("persist den-owned approval wait without panic");

    assert!(persisted.effective_approval_required);
    let obligation = persisted
        .obligation
        .expect("Den-owned approval-required tool should create permission obligation");
    assert_eq!(obligation.kind, "permission_decision");
    assert_eq!(obligation.expected_responder_action, "permission_decision");
    assert_eq!(obligation.tool_call_id.as_deref(), Some("call-web-fetch"));
    assert_eq!(obligation.permission_id.as_deref(), Some("perm-web-fetch"));
    assert_eq!(obligation.request_payload["execution_target"], "den");

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "waiting_for_client");

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    let event = events
        .iter()
        .find(|event| event.event_type == "client.waiting")
        .expect("client.waiting event");
    assert_eq!(event.event.data["execution_target"], "den");
    assert_eq!(
        event.event.data["expected_client_method"],
        "client.permission.result"
    );
    assert_eq!(event.event.data["obligation_id"], obligation.id.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn armature_approval_required_tool_persists_policy_for_permission_reconstruction(
    pool: sqlx::PgPool,
) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let persisted = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-edit",
            tool_name: "fs_edit_file",
            title: Some("Edit file"),
            kind: Some("function"),
            arguments: &serde_json::json!({
                "path": "README.md",
                "old_text": "old",
                "new_text": "new"
            }),
            approval_request_id: Some("perm-edit"),
            approval_required: true,
            approval_reason: Some("Edit README.md"),
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect("persist armature approval wait");

    assert!(persisted.effective_approval_required);
    let obligation = persisted
        .obligation
        .expect("armature approval-required tool should create permission obligation");
    assert_eq!(obligation.kind, "permission_decision");
    assert_eq!(obligation.expected_responder_action, "permission_decision");
    assert_eq!(obligation.tool_call_id.as_deref(), Some("call-edit"));
    assert_eq!(obligation.permission_id.as_deref(), Some("perm-edit"));
    assert_eq!(
        obligation.request_payload["execution_target"],
        "armature_local"
    );
    assert_eq!(
        obligation.request_payload["policy"]["execution_target"],
        "armature_local"
    );
    assert_eq!(
        obligation.request_payload["policy"]["approval_policy"],
        "required"
    );
    assert_eq!(
        obligation.request_payload["policy"]["risk"],
        "writes_workspace"
    );

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    let event = events
        .iter()
        .find(|event| event.event_type == "client.waiting")
        .expect("client.waiting event");
    assert_eq!(event.event.data["execution_target"], "armature_local");
    assert_eq!(event.event.data["policy"]["approval_policy"], "required");
    assert_eq!(event.event.data["obligation_id"], obligation.id.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn terminal_turn_run_cannot_be_reopened_or_overwritten(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let cancelled = turn_runs::cancel_run(
        &pool,
        &session_id,
        &run_id,
        bear_id,
        user_id,
        "superseded_by_new_run",
        serde_json::json!({"run_id": run_id, "reason": "superseded_by_new_run"}),
    )
    .await
    .expect("cancel run");
    assert!(cancelled.is_some());

    let reopened =
        turn_runs::transition_run(&pool, &run_id, turn_runs::TurnRunState::Running, None)
            .await
            .expect("attempt reopen terminal run");
    assert!(reopened.is_none());
    let completed = turn_runs::complete_run(
        &pool,
        &session_id,
        &run_id,
        bear_id,
        user_id,
        Some("stale_completed"),
        serde_json::json!({"run_id": run_id}),
    )
    .await
    .expect("attempt overwrite terminal run");
    assert!(completed.is_none());

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "cancelled");
    assert_eq!(
        run.terminal_reason.as_deref(),
        Some("superseded_by_new_run")
    );

    let arguments = serde_json::json!({ "path": "." });
    let late_wait = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-after-terminal",
            tool_name: "fs_list_directory",
            title: Some("Late list"),
            kind: Some("read"),
            arguments: &arguments,
            approval_request_id: None,
            approval_required: false,
            approval_reason: None,
            event_run_id: Some(&run_id),
        },
    )
    .await
    .expect_err("late tool waits must not reopen terminal runs");
    assert!(matches!(
        late_wait,
        den_core::DenError::RunStateConflict {
            operation: "persist_client_wait",
            ..
        }
    ));
    assert!(
        turn_obligations::open_client_obligations_for_run(&pool, &run_id)
            .await
            .expect("list obligations after late wait")
            .is_empty()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_client_obligation_is_detected_without_early_settlement(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let obligation = turn_obligations::upsert_tool_result_obligation(
        &pool,
        &run_id,
        &session_id,
        "call-timeout",
        None,
        serde_json::json!({ "tool_name": "fs_list_directory" }),
    )
    .await
    .expect("create tool result obligation");

    sqlx::query(
        "UPDATE turn_obligations SET created_at = NOW() - INTERVAL '10 minutes' WHERE id = $1",
    )
    .bind(obligation.id)
    .execute(&pool)
    .await
    .expect("age obligation");

    let expired = turn_obligations::expire_open_client_obligations_for_session(&pool, &session_id)
        .await
        .expect("expire obligations");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, obligation.id);
    assert_eq!(expired[0].state, "waiting_for_client");
    assert!(expired[0].result_payload.is_none());

    let open = turn_obligations::open_client_obligations_for_session(&pool, &session_id)
        .await
        .expect("list open obligations");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, obligation.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn step_barrier_counts_only_obligations_for_same_step(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let first_step = turn_steps::ensure_active_step(&pool, &run_id)
        .await
        .expect("ensure first step");
    let first = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run_id,
        &session_id,
        Some(first_step.id),
        "call-first-step",
        None,
        serde_json::json!({ "tool": "first-step" }),
    )
    .await
    .expect("create first step obligation");
    assert_eq!(first.turn_step_id, Some(first_step.id));

    turn_steps::transition_step(&pool, first_step.id, turn_steps::TurnStepState::Continued)
        .await
        .expect("close first step");
    let second_step = turn_steps::ensure_active_step(&pool, &run_id)
        .await
        .expect("ensure second step");
    assert_ne!(first_step.id, second_step.id);
    let second = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run_id,
        &session_id,
        Some(second_step.id),
        "call-second-step",
        None,
        serde_json::json!({ "tool": "second-step" }),
    )
    .await
    .expect("create second step obligation");

    let open_first_step = turn_obligations::open_client_obligations_for_step(&pool, first_step.id)
        .await
        .expect("list first step open obligations");
    assert_eq!(open_first_step.len(), 1);
    assert_eq!(open_first_step[0].id, first.id);

    let open_second_step =
        turn_obligations::open_client_obligations_for_step(&pool, second_step.id)
            .await
            .expect("list second step open obligations");
    assert_eq!(open_second_step.len(), 1);
    assert_eq!(open_second_step[0].id, second.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn core_obligations_support_non_bearwire_channel_waits(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let step = turn_steps::ensure_active_step(&pool, &run_id)
        .await
        .expect("ensure step");

    let cases = [
        (
            TurnObligationKind::HumanInput,
            ExpectedResponderAction::HumanInput,
            "slack-thread-reply-1",
        ),
        (
            TurnObligationKind::ResourceBinding,
            ExpectedResponderAction::ResourceBinding,
            "calendar-oauth-binding-1",
        ),
        (
            TurnObligationKind::HandoffDecision,
            ExpectedResponderAction::HandoffDecision,
            "work-handoff-1",
        ),
    ];

    for (kind, action, responder_ref_id) in cases {
        let row = turn_obligations::create_turn_obligation_for_step(
            &pool,
            &run_id,
            &session_id,
            Some(step.id),
            kind,
            action,
            responder_ref_id,
            serde_json::json!({ "responder_ref_id": responder_ref_id }),
        )
        .await
        .expect("create non-BearWire obligation");
        assert_eq!(row.kind, kind.as_str());
        assert_eq!(row.expected_responder_action, action.as_str());
        assert_eq!(row.responder_ref_id.as_deref(), Some(responder_ref_id));
        assert_eq!(row.turn_step_id, Some(step.id));
    }

    let open = turn_obligations::open_client_obligations_for_step(&pool, step.id)
        .await
        .expect("list open obligations");
    assert_eq!(open.len(), 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn transactional_tool_wait_persists_step_obligation_and_event(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let permission_id = Some(format!("perm_{}", Uuid::new_v4().simple()));
    let title = Some("Read a file".to_string());
    let kind = Some("function".to_string());
    let arguments = serde_json::json!({ "path": "README.md" });
    let approval_reason = Some("read workspace file".to_string());
    let event_run_id = Some(run_id.clone());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let persisted = turn_waits::persist_bearwire_tool_call_wait_transactionally(
        &pool,
        turn_waits::PersistToolCallWaitInput {
            process_epoch_id: Uuid::new_v4(),
            session_id: &session_id,
            run_id: &run_id,
            bear_id,
            user_id,
            request_id: Uuid::new_v4(),
            tool_call_id: "call-transactional-wait",
            tool_name: "fs_read_text_file",
            title: title.as_deref(),
            kind: kind.as_deref(),
            arguments: &arguments,
            approval_request_id: permission_id.as_deref(),
            approval_required: true,
            approval_reason: approval_reason.as_deref(),
            event_run_id: event_run_id.as_deref(),
        },
    )
    .await
    .expect("persist wait transactionally");

    assert!(persisted.effective_approval_required);
    let obligation = persisted
        .obligation
        .as_ref()
        .expect("client permission wait should create obligation");
    assert_eq!(obligation.kind, "permission_decision");
    assert_eq!(obligation.expected_responder_action, "permission_decision");
    assert_eq!(
        obligation.permission_id.as_deref(),
        permission_id.as_deref()
    );
    assert_eq!(
        obligation.tool_call_id.as_deref(),
        Some("call-transactional-wait")
    );
    assert_eq!(obligation.turn_step_id, Some(persisted.turn_step_id));

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "waiting_for_client");

    let step_state: String = sqlx::query(
        r"
        SELECT state
        FROM turn_steps
        WHERE id = $1
        ",
    )
    .bind(persisted.turn_step_id)
    .fetch_one(&pool)
    .await
    .expect("load turn step")
    .get("state");
    assert_eq!(step_state, "waiting_for_client");

    let events = bearwire_events::list_bearwire_events_after(&pool, &session_id, None, 10)
        .await
        .expect("list events");
    let waiting = events
        .iter()
        .find(|row| row.event_type == "client.waiting")
        .expect("client.waiting event exists");
    assert_eq!(waiting.sequence_no, persisted.event_sequence);
    assert_eq!(
        waiting.event.data["obligation_id"],
        obligation.id.to_string()
    );
    assert_eq!(
        waiting.event.data["expected_client_method"],
        "client.permission.result"
    );
    assert_eq!(
        waiting.event.data["turn_step_id"],
        persisted.turn_step_id.to_string()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn web_chat_human_input_uses_core_surface_obligation_path(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let session_id = format!("web-chat-session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let responder_ref_id = format!("web-reply-{}", Uuid::new_v4().simple());

    turn_runs::create_run(&pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");

    let persisted = turn_waits::persist_surface_obligation_transactionally(
        &pool,
        turn_waits::PersistSurfaceObligationInput {
            session_id: &session_id,
            run_id: &run_id,
            kind: TurnObligationKind::HumanInput,
            expected_responder_action: ExpectedResponderAction::HumanInput,
            responder_ref_id: &responder_ref_id,
            request_payload: serde_json::json!({
                "surface": "web_chat",
                "prompt": "Please choose an option"
            }),
        },
    )
    .await
    .expect("persist web-chat human-input obligation");

    assert_eq!(persisted.obligation.kind, "human_input");
    assert_eq!(
        persisted.obligation.expected_responder_action,
        "human_input"
    );
    assert_eq!(
        persisted.obligation.responder_ref_id.as_deref(),
        Some(responder_ref_id.as_str())
    );
    assert_eq!(
        persisted.obligation.turn_step_id,
        Some(persisted.turn_step_id)
    );

    let run = turn_runs::get_run(&pool, &run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.state, "waiting_for_client");

    let projection =
        project_obligation_for_surface(&persisted.obligation, TurnSurfaceKind::WebChat);
    assert_eq!(projection.action_kind, SurfaceActionKind::ChatReply);
    assert_eq!(
        projection.obligation_id,
        persisted.obligation.id.to_string()
    );
    assert_eq!(
        projection.responder_ref_id.as_deref(),
        Some(responder_ref_id.as_str())
    );
    assert_eq!(
        projection.payload["turn_step_id"],
        persisted.turn_step_id.to_string()
    );
}
