use super::*;
use den_docket::{
    TaskListItem, TaskListLocalProjection, TaskListProjection, TaskListSourceRef, TaskListSyncState,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn item(title: &str, status: TaskListItemStatus) -> TaskListUpdateItem {
    TaskListUpdateItem {
        id: title.to_string(),
        title: title.to_string(),
        summary: Some(format!("evidence: {title}")),
        status,
        blocked_reason: (status == TaskListItemStatus::Blocked).then(|| "waiting".to_string()),
        source_refs: Vec::new(),
    }
}

fn plan(status: &str, items: Vec<TaskListUpdateItem>) -> TaskListLocalProjection {
    TaskListLocalProjection {
        id: Uuid::nil(),
        bear_id: Uuid::nil(),
        title: "Complete Docket relational work management".to_string(),
        summary: "Acceptance criteria".to_string(),
        owner_profile: "pair".to_string(),
        visibility: "bear_visible".to_string(),
        status: status.to_string(),
        version: 1,
        current_item: items
            .iter()
            .find(|item| item.status == TaskListItemStatus::InProgress)
            .cloned(),
        items,
        source_conversation_id: None,
        source_client_session_id: None,
        handoff_intent_path: None,
        handoff_task_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn task_list_item(title: &str, status: TaskListItemStatus) -> TaskListItem {
    TaskListItem {
        id: title.to_string(),
        title: title.to_string(),
        summary: Some(format!("evidence: {title}")),
        status,
        blocked_reason: (status == TaskListItemStatus::Blocked)
            .then(|| "permission needed".to_string()),
        source_ref: TaskListSourceRef::local(Vec::new()),
        sync_state: TaskListSyncState::LocalOnly,
    }
}

fn task_list(status: &str, items: Vec<TaskListItem>) -> TaskListProjection {
    TaskListProjection {
        id: Uuid::nil(),
        bear_id: Uuid::nil(),
        title: "Implementation".to_string(),
        summary: "Acceptance criteria".to_string(),
        owner_profile: "pair".to_string(),
        visibility: "bear_visible".to_string(),
        status: status.to_string(),
        version: 1,
        source_ref: TaskListSourceRef::local(Vec::new()),
        current_item: items
            .iter()
            .find(|item| item.status == TaskListItemStatus::InProgress)
            .cloned(),
        items,
        source_conversation_id: None,
        source_client_session_id: None,
        handoff_intent_path: None,
        handoff_task_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn autonomous_resume_obligation_lists_next_incomplete_item() {
    let plan = plan(
        "active",
        vec![
            item(
                "Inventory schema and Docket API coupling",
                TaskListItemStatus::Completed,
            ),
            item(
                "Add lifecycle/dispatcher tests",
                TaskListItemStatus::Pending,
            ),
        ],
    );
    let text = autonomous_resume_obligation_text(&editor_capabilities(), &plan)
        .expect("autonomous reminder");
    assert!(text.contains("Add lifecycle/dispatcher tests"));
    assert!(text.contains("Do not provide a progress-only final answer"));
}

#[test]
fn autonomous_gate_blocks_progress_report_while_work_remains() {
    let plan = plan(
        "active",
        vec![
            item("done", TaskListItemStatus::Completed),
            item("remaining", TaskListItemStatus::InProgress),
        ],
    );
    let gate = autonomous_execution_gate_for_plan(
        &editor_capabilities(),
        Some(&plan),
        classify_autonomous_final_response(
            "What I changed: added one test. Remaining work: gate final answers.",
        ),
    );
    assert!(gate.is_active_autonomous_task);
    assert!(gate.has_incomplete_unblocked_items);
    assert!(!gate.may_stop);
}

#[test]
fn autonomous_gate_allows_completion_only_when_plan_complete() {
    let plan = plan(
        "completed",
        vec![item("done", TaskListItemStatus::Completed)],
    );
    let gate = autonomous_execution_gate_for_plan(
        &editor_capabilities(),
        Some(&plan),
        AutonomousFinalResponseKind::CompletionFinal,
    );
    assert!(gate.acceptance_criteria_met);
    assert!(gate.may_stop);
}

#[test]
fn autonomous_gate_allows_blocked_final_when_no_safe_path_remains() {
    let plan = plan(
        "blocked",
        vec![item("blocked", TaskListItemStatus::Blocked)],
    );
    let gate = autonomous_execution_gate_for_plan(
        &editor_capabilities(),
        Some(&plan),
        AutonomousFinalResponseKind::BlockedFinal,
    );
    assert!(gate.has_hard_blocker);
    assert!(gate.may_stop);
}

#[test]
fn pair_without_active_task_list_does_not_trigger_terminal_gate() {
    assert!(should_allow_terminal_response(
        &editor_capabilities(),
        None,
        "What I changed: added one test. Remaining work: more later."
    ));
}

#[test]
fn cancelled_remaining_task_allows_reasoned_non_action_final() {
    let task_list = task_list(
        "active",
        vec![
            task_list_item("Implement change", TaskListItemStatus::Completed),
            task_list_item("Commit changes", TaskListItemStatus::Cancelled),
        ],
    );

    let gate = autonomous_execution_gate_for_task_list(
        &editor_capabilities(),
        Some(&task_list),
        classify_autonomous_final_response(
            "I did not commit because there are no relevant changes to commit.",
        ),
    );

    assert!(gate.is_active_autonomous_task);
    assert!(!gate.has_incomplete_unblocked_items);
    assert!(gate.may_stop);
}

#[test]
fn blocked_list_state_allows_blocker_final() {
    let task_list = task_list(
        "blocked",
        vec![
            task_list_item("Implement change", TaskListItemStatus::Completed),
            task_list_item("Commit changes", TaskListItemStatus::Blocked),
        ],
    );

    let gate = autonomous_execution_gate_for_task_list(
        &editor_capabilities(),
        Some(&task_list),
        classify_autonomous_final_response(
            "I am blocked because committing requires explicit permission.",
        ),
    );

    assert!(gate.has_hard_blocker);
    assert!(gate.may_stop);
}

#[test]
fn scope_escalation_prose_does_not_allow_terminal_response_with_remaining_work() {
    let task_list = task_list(
        "active",
        vec![
            task_list_item(
                "Rename internal Docket model names",
                TaskListItemStatus::Completed,
            ),
            task_list_item(
                "Rename public den.work_plan tools",
                TaskListItemStatus::Pending,
            ),
        ],
    );

    let gate = autonomous_execution_gate_for_task_list(
        &editor_capabilities(),
        Some(&task_list),
        classify_autonomous_final_response(
            "Terminal status: requires scope escalation. Remaining work is a public API migration for public tool protocol names and needs a separate migration plan.",
        ),
    );

    assert!(gate.has_incomplete_unblocked_items);
    assert!(!gate.may_stop);
}

#[test]
fn scope_escalation_classifier_beats_progress_report_language() {
    assert_eq!(
        classify_autonomous_final_response(
            "Remaining work exists, but it is out of scope because it changes external tool contracts.",
        ),
        AutonomousFinalResponseKind::ScopeEscalationFinal
    );
}

#[test]
fn runtime_limit_blocked_final_forces_continuation_with_remaining_work() {
    let task_list = task_list(
        "active",
        vec![
            task_list_item(
                "Add runtime-limit terminal state",
                TaskListItemStatus::Completed,
            ),
            task_list_item("Commit task-focus batch", TaskListItemStatus::Pending),
        ],
    );

    let gate = autonomous_execution_gate_for_task_list(
        &editor_capabilities(),
        Some(&task_list),
        classify_autonomous_final_response(
            "Terminal status: blocked by runtime limits. The write budget is exhausted; continuing requires a fresh turn.",
        ),
    );

    assert!(gate.has_incomplete_unblocked_items);
    assert!(!gate.may_stop);
}

#[test]
fn runtime_limit_blocked_classifier_beats_progress_report_language() {
    assert_eq!(
        classify_autonomous_final_response(
            "Remaining work exists, but the tool budget and write budget are exhausted; resume in a fresh turn.",
        ),
        AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
    );
}

#[test]
fn task_focus_loop_detects_repeated_scope_objections_after_nudges() {
    let recent = [
        "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
        "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
        "Continue with: finish the active task list.",
        "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
    ];

    let detection = detect_task_focus_loop(&recent);

    assert!(detection.detected);
    assert_eq!(detection.continuation_nudges, 2);
    assert_eq!(detection.terminal_objections, 2);
    assert_eq!(
        detection.repeated_objection_kind,
        Some(AutonomousFinalResponseKind::ScopeEscalationFinal)
    );
}

#[test]
fn task_focus_loop_ignores_substantially_different_scope_objections() {
    let recent = [
        "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
        "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
        "Continue with: finish the active task list.",
        "Terminal status: requires scope escalation. Database migration ownership is outside scope for this plan.",
    ];

    let detection = detect_task_focus_loop(&recent);

    assert!(!detection.detected);
    assert_eq!(detection.continuation_nudges, 2);
    assert_eq!(detection.terminal_objections, 2);
    assert_eq!(detection.repeated_objection_kind, None);
}

#[test]
fn task_focus_loop_requires_substantially_same_terminal_objection() {
    let recent = [
        "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
        "Terminal status: blocked by runtime limits. The write budget is exhausted; continuing requires a fresh turn.",
        "Continue with: finish the active task list.",
        "Terminal status: requires scope escalation. Remaining public tool protocol names need a separate API migration plan.",
    ];

    let detection = detect_task_focus_loop(&recent);

    assert!(!detection.detected);
    assert_eq!(detection.continuation_nudges, 2);
    assert_eq!(detection.terminal_objections, 2);
    assert_eq!(detection.repeated_objection_kind, None);
}

#[test]
fn task_focus_loop_ignores_single_progress_report() {
    let recent = [
        "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.",
        "What I changed: updated one file. Remaining work: tests.",
    ];

    let detection = detect_task_focus_loop(&recent);

    assert!(!detection.detected);
    assert_eq!(detection.terminal_objections, 0);
}

#[test]
fn focused_gates_and_resume_obligation_use_capabilities_not_owner_labels() {
    use den_core::{ArmatureAvailability, EffectivePolicy, Governance, TurnExecutionOrigin};

    let editor = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let work = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let policies = [
        (
            EffectivePolicy::compile_for_origin(editor, Governance::Interactive).capabilities,
            true,
        ),
        (
            EffectivePolicy::compile_for_origin(work, Governance::AutonomousContinuation)
                .capabilities,
            true,
        ),
        (
            EffectivePolicy::compile_for_origin(editor, Governance::Observational).capabilities,
            false,
        ),
        (
            EffectivePolicy::compile_for_origin(work, Governance::Frozen).capabilities,
            false,
        ),
        (
            EffectivePolicy::compile_for_origin(
                TurnExecutionOrigin::ChannelConversation,
                Governance::Interactive,
            )
            .capabilities,
            false,
        ),
        (
            CapabilitySet::from_capabilities([BearCapability::ExecuteJob]),
            false,
        ),
    ];
    for (capabilities, may_execute) in policies {
        for owner_label in ["pair", "work", "chat", "watch", "curate", "forged-owner"] {
            let mut plan = plan("active", vec![item("Next", TaskListItemStatus::Pending)]);
            plan.owner_profile = owner_label.into();
            let mut list = task_list(
                "active",
                vec![task_list_item("Next", TaskListItemStatus::Pending)],
            );
            list.owner_profile = owner_label.into();
            for gate in [
                autonomous_execution_gate_for_plan(
                    &capabilities,
                    Some(&plan),
                    AutonomousFinalResponseKind::ProgressReport,
                ),
                autonomous_execution_gate_for_task_list(
                    &capabilities,
                    Some(&list),
                    AutonomousFinalResponseKind::ProgressReport,
                ),
            ] {
                assert_eq!(gate.is_active_autonomous_task, may_execute, "{owner_label}");
                assert_eq!(gate.may_stop, !may_execute, "{owner_label}");
            }
            assert_eq!(
                autonomous_resume_obligation_text(&capabilities, &plan).is_some(),
                may_execute
            );
            assert_eq!(
                should_allow_terminal_response(&capabilities, Some(&plan), "Remaining work"),
                !may_execute
            );
            assert_eq!(
                should_allow_terminal_response_for_task_list(
                    &capabilities,
                    Some(&list),
                    "Remaining work"
                ),
                !may_execute
            );
        }
    }
}

#[test]
fn stored_task_projection_reports_state_without_execution_authority() {
    use den_core::client_tools::ToolEnablementState;
    let policy = ResolvedSessionPolicy {
        mode_label: "Write",
        tool_enablement: ToolEnablementState::AllTools,
        plan_mode_state: None,
    };
    for owner_label in ["pair", "work", "watch", "forged-owner"] {
        let mut plan = plan("active", vec![item("Next", TaskListItemStatus::Pending)]);
        plan.owner_profile = owner_label.into();
        let value = turn_state_json(&policy, Some(&plan));
        assert_eq!(value["activity"]["owner_profile"], owner_label);
        let projection = &value["autonomous_execution"];
        assert_eq!(projection["execution_authority"], "not_evaluated");
        assert_eq!(projection["active"], false);
        assert!(projection["mode"].is_null());
        assert!(projection.get("continuation_policy").is_none());
        assert_eq!(projection["has_incomplete_unblocked_items"], true);
        assert_eq!(projection["next_incomplete_task_title"], "Next");
    }
}

fn editor_capabilities() -> CapabilitySet {
    den_core::EffectivePolicy::compile_for_origin(
        den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected,
        ),
        den_core::Governance::Interactive,
    )
    .capabilities
}
