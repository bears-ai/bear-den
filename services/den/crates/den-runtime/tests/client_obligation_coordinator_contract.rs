use den_runtime::{
    client_obligation_coordinator::{
        self, PermissionResultCoordinatorOutcome, ToolResultCoordinatorOutcome,
    },
    turn_obligations, turn_runs, turn_steps,
};
use serde_json::json;
use uuid::Uuid;

async fn finish_test_run(
    pool: &sqlx::PgPool,
    run: &turn_runs::TurnRunRow,
    state: turn_runs::TurnRunState,
) {
    let result = match state {
        turn_runs::TurnRunState::Completed => {
            turn_runs::complete_run(
                pool,
                &run.session_id,
                &run.run_id,
                run.bear_id,
                run.user_id,
                Some("test terminal"),
                json!({"run_id": run.run_id}),
            )
            .await
        }
        turn_runs::TurnRunState::Failed => {
            turn_runs::fail_run(
                pool,
                &run.session_id,
                &run.run_id,
                run.bear_id,
                run.user_id,
                "test terminal",
                json!({"run_id": run.run_id}),
            )
            .await
        }
        turn_runs::TurnRunState::Cancelled => {
            turn_runs::cancel_run(
                pool,
                &run.session_id,
                &run.run_id,
                run.bear_id,
                run.user_id,
                "test terminal",
                json!({"run_id": run.run_id}),
            )
            .await
        }
        _ => panic!("test terminal helper requires terminal state"),
    };
    result
        .expect("finish test run")
        .expect("test run was active");
}

async fn create_user_and_bear(pool: &sqlx::PgPool) -> (i32, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("coord{}", &suffix[..16]);
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
    .bind("Coordinator Test")
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
    .bind("Coordinator Test Bear")
    .execute(pool)
    .await
    .expect("create test bear");

    (user_id, bear_id)
}

async fn claim_tool_obligation(
    pool: &sqlx::PgPool,
    obligation: &turn_obligations::TurnObligationRow,
) -> String {
    let attempt_token_hash =
        turn_obligations::lease_attempt_token_hash(&Uuid::new_v4().to_string());
    turn_obligations::claim_tool_execution(
        pool,
        obligation.id,
        &obligation.run_id,
        &obligation.session_id,
        obligation.tool_call_id.as_deref().expect("tool call id"),
        &attempt_token_hash,
    )
    .await
    .expect("claim tool obligation")
    .expect("tool obligation was claimable");
    attempt_token_hash
}

async fn create_run_with_step(
    pool: &sqlx::PgPool,
) -> (turn_runs::TurnRunRow, turn_steps::TurnStepRow, String) {
    let (user_id, bear_id) = create_user_and_bear(pool).await;
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let run = turn_runs::create_run(pool, &run_id, &session_id, bear_id, user_id)
        .await
        .expect("create run");
    let step = turn_steps::ensure_active_step(pool, &run_id)
        .await
        .expect("ensure step");
    (run, step, session_id)
}

#[sqlx::test(migrations = "../../migrations")]
async fn multi_tool_step_continues_exactly_once_after_all_results_settle(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;

    let first = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-first",
        None,
        json!({ "tool_name": "fs_read_text_file", "arguments": { "path": "a" } }),
    )
    .await
    .expect("create first obligation");
    let second = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-second",
        None,
        json!({ "tool_name": "fs_list_directory", "arguments": { "path": "." } }),
    )
    .await
    .expect("create second obligation");

    let first_attempt = claim_tool_obligation(&pool, &first).await;
    let second_attempt = claim_tool_obligation(&pool, &second).await;

    let first_outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &first,
        &first_attempt,
        "tool",
        "call-first",
        json!({ "status": "ok", "content": "first" }),
    )
    .await
    .expect("settle first tool result");
    match first_outcome {
        ToolResultCoordinatorOutcome::WaitingForMoreClientResults {
            open_obligations, ..
        } => {
            assert_eq!(open_obligations.len(), 1);
            assert_eq!(open_obligations[0].id, second.id);
        }
        other => panic!("first result must not continue model: {other:?}"),
    }

    let second_outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &second,
        &second_attempt,
        "tool",
        "call-second",
        json!({ "status": "ok", "content": "second" }),
    )
    .await
    .expect("settle second tool result");
    assert!(matches!(
        second_outcome,
        ToolResultCoordinatorOutcome::ContinueModel { run: Some(_), .. }
    ));

    let duplicate_second = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &second,
        &second_attempt,
        "tool",
        "call-second",
        json!({ "status": "ok", "content": "second" }),
    )
    .await
    .expect("settle duplicate second tool result");
    assert!(matches!(
        duplicate_second,
        ToolResultCoordinatorOutcome::DuplicateIdentical { .. }
    ));

    let refreshed_run = turn_runs::get_run(&pool, &run.run_id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(refreshed_run.state, "continuing");
    let open = turn_obligations::open_client_obligations_for_step(&pool, step.id)
        .await
        .expect("list open obligations");
    assert!(open.is_empty());
    let result_count = turn_runs::client_result_count_for_run_kind(&pool, &run.run_id, "tool")
        .await
        .expect("count tool results");
    assert_eq!(result_count, 2, "duplicates must not add result rows");
}

#[sqlx::test(migrations = "../../migrations")]
async fn tool_execution_error_is_a_settling_result_and_can_continue(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-missing-file",
        None,
        json!({ "tool_name": "fs_read_text_file", "arguments": { "path": "missing.md" } }),
    )
    .await
    .expect("create tool obligation");

    let attempt = claim_tool_obligation(&pool, &obligation).await;
    let outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &obligation,
        &attempt,
        "tool",
        "call-missing-file",
        json!({
            "status": "error",
            "error": {
                "code": -32002,
                "message": "Resource not found",
                "data": { "uri": "missing.md" }
            }
        }),
    )
    .await
    .expect("settle tool error result");

    assert!(matches!(
        outcome,
        ToolResultCoordinatorOutcome::ContinueModel { run: Some(_), .. }
    ));
    let stored =
        turn_obligations::get_tool_call_obligation(&pool, &run.run_id, "call-missing-file")
            .await
            .expect("load obligation")
            .expect("obligation exists");
    assert_eq!(stored.state, "continued");
    assert_eq!(stored.result_payload.as_ref().unwrap()["status"], "error");
}

#[sqlx::test(migrations = "../../migrations")]
async fn late_result_after_terminal_run_is_ignored_by_coordinator(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-late",
        None,
        json!({ "tool_name": "fs_read_text_file" }),
    )
    .await
    .expect("create tool obligation");
    finish_test_run(&pool, &run, turn_runs::TurnRunState::Failed).await;
    let failed_run = turn_runs::get_run(&pool, &run.run_id)
        .await
        .expect("load failed run")
        .expect("run exists");
    let failed_obligation =
        turn_obligations::get_tool_call_obligation(&pool, &run.run_id, "call-late")
            .await
            .expect("load failed obligation")
            .expect("obligation exists");
    assert_eq!(failed_obligation.id, obligation.id);

    let outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &failed_run,
        &failed_obligation,
        "",
        "tool",
        "call-late",
        json!({ "status": "ok", "content": "too late" }),
    )
    .await
    .expect("settle late tool result");

    assert!(matches!(
        outcome,
        ToolResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state
        } if run_state == "failed" && obligation_state == "failed"
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn late_tool_result_after_terminal_is_ignored(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-late-exact-name",
        None,
        json!({ "tool_name": "fs_read_text_file" }),
    )
    .await
    .expect("create tool obligation");
    finish_test_run(&pool, &run, turn_runs::TurnRunState::Completed).await;
    let completed_run = turn_runs::get_run(&pool, &run.run_id)
        .await
        .expect("load completed run")
        .expect("run exists");
    let completed_obligation =
        turn_obligations::get_tool_call_obligation(&pool, &run.run_id, "call-late-exact-name")
            .await
            .expect("load completed obligation")
            .expect("obligation exists");
    assert_eq!(completed_obligation.id, obligation.id);

    let outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &completed_run,
        &completed_obligation,
        "",
        "tool",
        "call-late-exact-name",
        json!({ "status": "ok", "content": "too late" }),
    )
    .await
    .expect("settle late tool result");

    assert!(matches!(
        outcome,
        ToolResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state
        } if run_state == "completed" && obligation_state == "continued"
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_conflicting_tool_result_is_owned_by_coordinator(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "call-conflict",
        None,
        json!({ "tool_name": "fs_read_text_file" }),
    )
    .await
    .expect("create tool obligation");

    let attempt = claim_tool_obligation(&pool, &obligation).await;
    let first = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &obligation,
        &attempt,
        "tool",
        "call-conflict",
        json!({ "status": "ok", "content": "one" }),
    )
    .await
    .expect("settle first result");
    assert!(matches!(
        first,
        ToolResultCoordinatorOutcome::ContinueModel { .. }
    ));

    let conflict = client_obligation_coordinator::record_and_settle_tool_result(
        &pool,
        &run,
        &obligation,
        &attempt,
        "tool",
        "call-conflict",
        json!({ "status": "ok", "content": "two" }),
    )
    .await
    .expect("detect duplicate conflict");
    assert!(matches!(
        conflict,
        ToolResultCoordinatorOutcome::DuplicateConflict { .. }
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_identical_permission_result_is_owned_by_coordinator(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_permission_decision_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "perm-duplicate",
        None,
        json!({ "tool_name": "den.web_fetch", "arguments": { "url": "https://example.test" } }),
    )
    .await
    .expect("create Den-hosted permission obligation");

    let payload = json!({ "decision": "denied", "reason": "no" });
    let first = client_obligation_coordinator::record_and_settle_permission_result(
        &pool,
        &run,
        &obligation,
        "denied",
        "permission",
        "perm-duplicate",
        payload.clone(),
    )
    .await
    .expect("settle first permission result");
    assert!(matches!(
        first,
        PermissionResultCoordinatorOutcome::ContinueModel { .. }
    ));

    let duplicate = client_obligation_coordinator::record_and_settle_permission_result(
        &pool,
        &run,
        &obligation,
        "denied",
        "permission",
        "perm-duplicate",
        payload,
    )
    .await
    .expect("settle duplicate permission result");
    assert!(matches!(
        duplicate,
        PermissionResultCoordinatorOutcome::DuplicateIdentical { .. }
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn permission_denial_path_continues_without_dispatching_local_tool(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_permission_decision_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "perm-denied",
        Some("call-denied"),
        json!({ "tool_name": "fs_edit_file", "arguments": { "path": "x" } }),
    )
    .await
    .expect("create permission obligation");

    let outcome = client_obligation_coordinator::record_and_settle_permission_result(
        &pool,
        &run,
        &obligation,
        "denied",
        "permission",
        "perm-denied",
        json!({ "decision": "denied", "reason": "test" }),
    )
    .await
    .expect("settle denied permission result");

    assert!(matches!(
        outcome,
        PermissionResultCoordinatorOutcome::ContinueModel { run: Some(_), .. }
    ));
    let stored = turn_obligations::get_permission_obligation(&pool, &run.run_id, "perm-denied")
        .await
        .expect("load permission obligation")
        .expect("permission obligation exists");
    assert_eq!(stored.state, "continued");
    assert_eq!(stored.kind, "permission_decision");
}

#[sqlx::test(migrations = "../../migrations")]
async fn den_hosted_approved_permission_continues_without_local_tool_dispatch(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_permission_decision_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "perm-web-fetch",
        Some("call-web-fetch"),
        json!({ "tool_name": "web_fetch", "arguments": { "url": "https://example.test" } }),
    )
    .await
    .expect("create Den-hosted permission obligation");

    let outcome = client_obligation_coordinator::record_and_settle_permission_result(
        &pool,
        &run,
        &obligation,
        "granted",
        "permission",
        "perm-web-fetch",
        json!({ "decision": "approved" }),
    )
    .await
    .expect("settle approved Den-hosted permission result");

    assert!(matches!(
        outcome,
        PermissionResultCoordinatorOutcome::ContinueModel { run: Some(_), .. }
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn continuation_claim_is_one_shot_from_waiting_for_client(pool: sqlx::PgPool) {
    let (run, _step, _session_id) = create_run_with_step(&pool).await;
    let waiting = turn_runs::transition_run(
        &pool,
        &run.run_id,
        turn_runs::TurnRunState::WaitingForClient,
        None,
    )
    .await
    .expect("mark run waiting")
    .expect("run exists");
    assert_eq!(waiting.state, "waiting_for_client");

    let winner = turn_runs::claim_run_continuation(&pool, &run.run_id, None)
        .await
        .expect("claim continuation");
    let loser = turn_runs::claim_run_continuation(&pool, &run.run_id, None)
        .await
        .expect("repeat continuation claim");

    assert!(
        winner.is_some(),
        "the first claimant owns continuation startup"
    );
    assert!(
        loser.is_none(),
        "a second claimant must not start a model stream"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn local_approved_permission_persists_recoverable_tool_obligation(pool: sqlx::PgPool) {
    let (run, step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_permission_decision_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(step.id),
        "perm-local",
        Some("call-local"),
        json!({
            "approval_required": true,
            "approval_request_id": "perm-local",
            "tool_call_id": "call-local",
            "tool_name": "fs_edit_file",
            "arguments": { "path": "README.md", "old_text": "a", "new_text": "b" },
            "execution_target": "armature_local",
            "policy": {
                "execution_target": "armature_local",
                "approval_required": true,
                "approval_policy": "required",
                "risk": "writes_workspace"
            }
        }),
    )
    .await
    .expect("create local permission obligation");

    let outcome = client_obligation_coordinator::record_and_settle_permission_result(
        &pool,
        &run,
        &obligation,
        "granted",
        "permission",
        "perm-local",
        json!({ "decision": "approved" }),
    )
    .await
    .expect("settle approved local permission result");

    match outcome {
        PermissionResultCoordinatorOutcome::DispatchLocalTool {
            tool_obligation,
            tool_call_id,
            tool_name,
            args,
            ..
        } => {
            assert_eq!(tool_obligation.id, obligation.id);
            assert_eq!(tool_obligation.kind, "tool_result");
            assert_eq!(tool_obligation.expected_responder_action, "tool_result");
            assert_eq!(tool_obligation.state, "waiting_for_client");
            assert_eq!(tool_obligation.permission_id.as_deref(), Some("perm-local"));
            assert_eq!(tool_obligation.tool_call_id.as_deref(), Some("call-local"));
            assert_eq!(tool_obligation.request_payload["approval_required"], false);
            assert_eq!(tool_obligation.request_payload["permission_granted"], true);
            assert_eq!(
                tool_obligation.request_payload["approval_request_id"],
                "perm-local"
            );
            assert_eq!(tool_call_id, "call-local");
            assert_eq!(tool_name, "fs_edit_file");
            assert_eq!(
                args,
                json!({ "path": "README.md", "old_text": "a", "new_text": "b" })
            );

            let open = turn_obligations::open_client_obligations_for_step(&pool, step.id)
                .await
                .expect("fetch durable open obligations");
            assert_eq!(open.len(), 1);
            assert_eq!(open[0].id, obligation.id);
            assert_eq!(open[0].expected_responder_action, "tool_result");
            assert_eq!(open[0].state, "waiting_for_client");
            assert_eq!(open[0].request_payload["approval_required"], false);
            assert_eq!(open[0].request_payload["permission_granted"], true);
            assert_eq!(open[0].request_payload["tool_call_id"], "call-local");
            assert_eq!(open[0].request_payload["tool_name"], "fs_edit_file");
            assert_eq!(open[0].request_payload["arguments"]["new_text"], "b");
        }
        other => panic!("approved local permission should dispatch tool: {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn stale_wrong_step_result_is_detected_before_settlement(pool: sqlx::PgPool) {
    let (run, first_step, session_id) = create_run_with_step(&pool).await;
    let obligation = turn_obligations::upsert_tool_result_obligation_for_step(
        &pool,
        &run.run_id,
        &session_id,
        Some(first_step.id),
        "call-step-bound",
        None,
        json!({ "tool_name": "fs_read_text_file" }),
    )
    .await
    .expect("create first-step obligation");
    turn_steps::transition_step(&pool, first_step.id, turn_steps::TurnStepState::Continued)
        .await
        .expect("close first step");
    let second_step = turn_steps::ensure_active_step(&pool, &run.run_id)
        .await
        .expect("ensure second step");
    assert_ne!(first_step.id, second_step.id);

    let err = client_obligation_coordinator::record_and_settle_tool_result_for_step(
        &pool,
        &run,
        &obligation,
        Some(second_step.id),
        "",
        "tool",
        "call-step-bound",
        json!({ "status": "ok", "content": "wrong step" }),
    )
    .await
    .expect_err("wrong step result should be rejected");
    assert!(
        err.to_string().contains("turn_step_id mismatch"),
        "unexpected error: {err}"
    );
}
