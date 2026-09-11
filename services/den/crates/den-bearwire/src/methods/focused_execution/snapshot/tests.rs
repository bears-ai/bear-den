use super::*;

fn active_facts(run_state: TurnRunState) -> FocusedExecutionFacts {
    let session_id = ClientSessionId::new("session-1").unwrap();
    let run_id = TurnRunId::new("run-1").unwrap();
    let task_id = Uuid::from_u128(1);
    FocusedExecutionFacts {
        session_id: session_id.clone(),
        task_id: Some(task_id),
        binding: Some(DocketFocusedExecutionBinding {
            kind: DocketExecutionBindingKind::ClientSession,
            id: session_id.to_string(),
        }),
        run: Some(FocusedExecutionRun {
            id: run_id.clone(),
            state: run_state,
            terminal_reason: None,
        }),
        attempt: Some(FocusedExecutionAttempt {
            id: Uuid::from_u128(2),
            state: DocketExecutionAttemptState::Running,
            fence_epoch: 1,
        }),
        attempt_task_id: Some(task_id),
        host: Some(DocketExecutionHost {
            kind: DocketExecutionHostKind::TurnRun,
            run_id: run_id.to_string(),
        }),
        controller: ControllerDisposition::Live,
        open_obligations: 0,
    }
}

fn reduced(facts: FocusedExecutionFacts) -> FocusedExecutionState {
    reduce_focused_execution(facts, FocusedExecutionLaunchState::AlreadyRunning).state
}

#[test]
fn reducer_covers_every_lifecycle_phase_and_invariant() {
    let mut unfocused = active_facts(TurnRunState::Running);
    unfocused.task_id = None;
    unfocused.binding = None;
    unfocused.run = None;
    unfocused.attempt = None;
    unfocused.attempt_task_id = None;
    unfocused.host = None;
    unfocused.controller = ControllerDisposition::NotApplicable;

    let mut selected = unfocused.clone();
    selected.task_id = Some(Uuid::from_u128(1));

    let mut queued = active_facts(TurnRunState::Accepted);
    queued.controller = ControllerDisposition::Queued;
    let mut claimed = active_facts(TurnRunState::Accepted);
    claimed.controller = ControllerDisposition::Claimed;
    let mut recovering = active_facts(TurnRunState::Running);
    recovering.controller = ControllerDisposition::Recovering;

    let mut terminal = active_facts(TurnRunState::Completed);
    terminal.attempt.as_mut().unwrap().state = DocketExecutionAttemptState::Settled;
    terminal.controller = ControllerDisposition::Missing;
    let mut ended_authority_with_live_host = active_facts(TurnRunState::Running);
    ended_authority_with_live_host
        .attempt
        .as_mut()
        .unwrap()
        .state = DocketExecutionAttemptState::Settled;
    ended_authority_with_live_host.controller = ControllerDisposition::NotApplicable;

    let phase_cases = [
        (
            "unfocused",
            unfocused.clone(),
            FocusedExecutionState::Unfocused,
        ),
        (
            "selected",
            selected.clone(),
            FocusedExecutionState::Selected,
        ),
        ("queued", queued, FocusedExecutionState::Starting),
        ("claimed", claimed, FocusedExecutionState::Starting),
        (
            "native started",
            active_facts(TurnRunState::Accepted),
            FocusedExecutionState::Starting,
        ),
        (
            "running",
            active_facts(TurnRunState::Running),
            FocusedExecutionState::Running,
        ),
        (
            "waiting",
            active_facts(TurnRunState::WaitingForClient),
            FocusedExecutionState::WaitingForClient,
        ),
        (
            "continuing",
            active_facts(TurnRunState::Continuing),
            FocusedExecutionState::Continuing,
        ),
        ("recovering", recovering, FocusedExecutionState::Recovering),
        ("terminal", terminal, FocusedExecutionState::Terminal),
        (
            "ended authority with live host run",
            ended_authority_with_live_host,
            FocusedExecutionState::Terminal,
        ),
    ];
    for (name, facts, expected) in phase_cases {
        assert_eq!(reduced(facts), expected, "phase case {name}");
    }

    let mut run_without_selection = unfocused.clone();
    run_without_selection.run = active_facts(TurnRunState::Running).run;

    let mut attempt_without_run = selected.clone();
    attempt_without_run.attempt = active_facts(TurnRunState::Running).attempt;

    let mut active_without_attempt = active_facts(TurnRunState::Running);
    active_without_attempt.attempt = None;

    let mut host_mismatch = active_facts(TurnRunState::Running);
    host_mismatch.host.as_mut().unwrap().run_id = "different-run".to_string();

    let mut terminal_with_live_authority = active_facts(TurnRunState::Failed);
    terminal_with_live_authority.controller = ControllerDisposition::Missing;

    let mut controller_missing = active_facts(TurnRunState::Running);
    controller_missing.controller = ControllerDisposition::Missing;

    let mut controller_without_authority = selected;
    controller_without_authority.controller = ControllerDisposition::Live;

    let violation_cases = [
        (
            "run without selection",
            run_without_selection,
            FocusedExecutionInvariantViolation::RunWithoutSelection,
        ),
        (
            "attempt without run",
            attempt_without_run,
            FocusedExecutionInvariantViolation::AttemptWithoutRun,
        ),
        (
            "active run without attempt",
            active_without_attempt,
            FocusedExecutionInvariantViolation::ActiveRunWithoutAttempt,
        ),
        (
            "host mismatch",
            host_mismatch,
            FocusedExecutionInvariantViolation::HostMismatch,
        ),
        (
            "terminal with live authority",
            terminal_with_live_authority,
            FocusedExecutionInvariantViolation::TerminalRunWithLiveAttemptOrOpenObligations,
        ),
        (
            "running without controller",
            controller_missing,
            FocusedExecutionInvariantViolation::RunningWithoutController,
        ),
        (
            "controller without authority",
            controller_without_authority,
            FocusedExecutionInvariantViolation::ControllerWithoutDurableAuthority,
        ),
    ];
    for (name, facts, violation) in violation_cases {
        assert_eq!(
            reduced(facts),
            FocusedExecutionState::Inconsistent { violation },
            "invariant case {name}"
        );
    }
}
