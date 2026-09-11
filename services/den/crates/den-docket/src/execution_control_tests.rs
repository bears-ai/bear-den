use uuid::Uuid;

use crate::{
    model::DocketExecutionTaskControl, DocketExecutionBinding, DocketExecutionControl,
    DocketExecutionControlReference, DocketExecutionControlResult, DocketExecutionControlState,
    DocketExecutionGate, DocketExecutionNextAction, DocketExecutionReason,
};

fn control(
    next_action: DocketExecutionNextAction,
    reason: Option<DocketExecutionReason>,
    claimed_task_id: Option<Uuid>,
) -> DocketExecutionControl {
    DocketExecutionControl {
        run_id: Uuid::new_v4(),
        run_state: "running".to_owned(),
        task: DocketExecutionTaskControl {
            selected_task_id: claimed_task_id,
            focused_task_id: claimed_task_id,
            claimed_task_id,
            current_task_id: claimed_task_id,
        },
        next_action,
        retryable: false,
        reason,
    }
}

#[test]
fn execution_control_gate_allows_the_persisted_task_claim() {
    let task_id = Uuid::new_v4();
    let control = control(
        DocketExecutionNextAction::WorkCurrentTask,
        None,
        Some(task_id),
    );
    assert_eq!(
        control.gate(),
        DocketExecutionGate::Allowed {
            task_id,
            binding: DocketExecutionBinding::Session {
                job_run_id: control.run_id,
            },
        }
    );
}

#[test]
fn established_control_requires_persisted_boundary_continuation_and_authority() {
    let reference = |kind: &str| DocketExecutionControlReference {
        kind: kind.to_owned(),
        id: Uuid::new_v4().to_string(),
        persisted: true,
    };
    let established = DocketExecutionControlResult::continuation_established(
        Uuid::new_v4(),
        Some(Uuid::new_v4()),
        Uuid::new_v4(),
        Uuid::new_v4(),
        reference("client_session"),
        reference("tool_wait"),
        reference("waiting_for_tool"),
    );
    assert_eq!(
        established.state,
        DocketExecutionControlState::ContinuationEstablished
    );
    assert!(established.is_established());
}
