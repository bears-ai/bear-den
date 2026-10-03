use super::*;
use den_docket::{TaskListItem, TaskListItemStatus, TaskListSourceRef, TaskListSyncState};
use time::OffsetDateTime;
use uuid::Uuid;

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
        title: "Focused work".to_string(),
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
fn no_focus_allows_final_even_with_cached_task_list() {
    let cached_task_list = task_list(
        "active",
        vec![task_list_item(
            "Stale cached task",
            TaskListItemStatus::Pending,
        )],
    );

    // ponytail: this models the final gate after focus resolution; the cached
    // task list is intentionally not passed because cache-only focus has no
    // completion authority. Upgrade path is an integration replay at the
    // focus resolver boundary.
    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: None,
        assistant_text: "Done.",
        recent_texts: &[],
    });

    assert_eq!(cached_task_list.items.len(), 1);
    assert!(matches!(
        decision,
        TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::NoActiveFocusedTask,
            ..
        }
    ));
}

#[test]
fn durable_focus_with_incomplete_task_forces_continuation() {
    let focused = task_list(
        "active",
        vec![task_list_item(
            "Implement next slice",
            TaskListItemStatus::Pending,
        )],
    );

    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text: "Progress made; stopping here.",
        recent_texts: &[],
    });

    assert!(matches!(
        decision,
        TurnCompletionDecision::Continue {
            reason: TurnCompletionContinueReason::FocusedWorkRemains,
            ref next_task,
            ..
        } if next_task == "Implement next slice"
    ));
}

#[test]
fn blocked_sibling_does_not_stop_an_executable_focused_task() {
    let focused = task_list(
        "active",
        vec![
            task_list_item("Needs approval", TaskListItemStatus::Blocked),
            task_list_item("Independent follow-up", TaskListItemStatus::Pending),
        ],
    );

    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text: "Blocked waiting for approval.",
        recent_texts: &[],
    });

    assert!(matches!(
        decision,
        TurnCompletionDecision::Continue {
            ref next_task,
            ..
        } if next_task == "Independent follow-up"
    ));
}

#[test]
fn scope_escalation_prose_does_not_stop_executable_focused_work() {
    let focused = task_list(
        "active",
        vec![task_list_item(
            "Implement the bounded change",
            TaskListItemStatus::Pending,
        )],
    );

    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text: "This requires a separate migration, so I will stop here.",
        recent_texts: &[],
    });

    assert!(matches!(
        decision,
        TurnCompletionDecision::Continue {
            final_response_kind: AutonomousFinalResponseKind::ScopeEscalationFinal,
            ..
        }
    ));
}

#[test]
fn mode_change_away_from_focused_clears_focus_before_completion_policy() {
    let previously_focused = task_list(
        "active",
        vec![task_list_item(
            "Do not continue stale focus",
            TaskListItemStatus::Pending,
        )],
    );

    // ponytail: mode resolution owns clearing stale focus; this is the
    // completion-policy boundary check. Upgrade path is a mode-transition
    // replay test if focus resolution grows persistence side effects.
    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: None,
        assistant_text: "Mode changed back to ordinary chat.",
        recent_texts: &[],
    });

    assert_eq!(previously_focused.items.len(), 1);
    assert!(matches!(
        decision,
        TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::NoActiveFocusedTask,
            ..
        }
    ));
}

#[test]
fn runtime_limit_final_does_not_complete_incomplete_focused_work() {
    let focused = task_list(
        "active",
        vec![
            task_list_item("Recheck Docket", TaskListItemStatus::Completed),
            task_list_item("Continue next slice", TaskListItemStatus::Pending),
        ],
    );
    let recent_texts = vec![
        "Hit the wall-clock finalization warning after rechecking Docket and advancing the job."
            .to_string(),
    ];

    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text:
            "Hit the wall-clock runtime limit finalization warning after rechecking Docket and advancing the job.",
        recent_texts: &recent_texts,
    });

    match decision {
        TurnCompletionDecision::Continue {
            reason,
            next_task,
            final_response_kind,
            ..
        } => {
            assert_eq!(
                reason,
                TurnCompletionContinueReason::RuntimeLimitIsNotFocusedCompletion
            );
            assert_eq!(
                final_response_kind,
                AutonomousFinalResponseKind::RuntimeLimitBlockedFinal
            );
            assert_eq!(next_task, "Continue next slice");
        }
        other => panic!("expected continuation, got {other:?}"),
    }
}

#[test]
fn completed_focused_work_enters_finalization_drain() {
    let focused = task_list(
        "completed",
        vec![task_list_item(
            "Recheck Docket",
            TaskListItemStatus::Completed,
        )],
    );
    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text: "Job completed. Working tree clean.",
        recent_texts: &[],
    });

    assert!(matches!(
        decision,
        TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::FocusedWorkCompleteFinalizationDrain,
            ..
        }
    ));
}

#[test]
fn repeated_terminal_objection_pauses_instead_of_completing_focused_work() {
    let focused = task_list(
        "active",
        vec![task_list_item(
            "Continue next slice",
            TaskListItemStatus::Pending,
        )],
    );
    let recent_texts = vec![
        "You are in autonomous implementation mode. The active task list still has incomplete, unblocked work. Do not final-answer yet.".to_string(),
        "I am blocked and cannot continue because the required approval is absent.".to_string(),
        "Continue with: Continue next slice.".to_string(),
        "I am blocked and cannot continue because the required approval is absent.".to_string(),
    ];

    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &editor_capabilities(),
        current_task_list: Some(&focused),
        assistant_text: recent_texts.last().unwrap(),
        recent_texts: &recent_texts,
    });

    assert!(matches!(
        decision,
        TurnCompletionDecision::Pause {
            reason: TurnCompletionPauseReason::RepeatedTerminalObjection,
            ..
        }
    ));
}

#[test]
fn completion_route_requires_focused_task_capability_not_job_capability() {
    let focused = task_list(
        "active",
        vec![task_list_item("Next", TaskListItemStatus::Pending)],
    );
    let capabilities = CapabilitySet::from_capabilities([den_core::BearCapability::ExecuteJob]);
    let decision = decide_turn_completion(TurnCompletionPolicyInput {
        capabilities: &capabilities,
        current_task_list: Some(&focused),
        assistant_text: "Remaining work",
        recent_texts: &[],
    });
    assert!(matches!(
        decision,
        TurnCompletionDecision::Complete {
            reason: TurnCompletionCompleteReason::NoActiveFocusedTask,
            ..
        }
    ));
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
