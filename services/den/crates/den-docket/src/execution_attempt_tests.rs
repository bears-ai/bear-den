use crate::integration_tests::{seed_user_and_bear, test_pool, two_task_job};
use crate::{
    DocketExecutionAttemptAuthorize, DocketExecutionAttemptRelease, DocketExecutionAttemptStart,
    DocketExecutionAttemptState, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionAcquire, DocketFocusedExecutionBinding,
    DocketPairAwaitingUserQuestion, DocketPairAwaitingUserResume, DocketPairBoundedOutcome,
    DocketPairBoundedOutcomeReport, DocketPairContinuationDecision, DocketService, PgDocketService,
};
use uuid::Uuid;

fn session_authorization(
    bear_id: Uuid,
    task_id: Uuid,
    session_id: String,
    turn_run_id: String,
    authorization_key: Uuid,
) -> DocketExecutionAttemptAuthorize {
    DocketExecutionAttemptAuthorize {
        bear_id,
        task_id,
        binding: DocketFocusedExecutionBinding {
            kind: DocketExecutionBindingKind::ClientSession,
            id: session_id,
        },
        host: DocketExecutionHost {
            kind: DocketExecutionHostKind::TurnRun,
            run_id: turn_run_id,
        },
        authorization_key,
    }
}

#[tokio::test]
async fn focused_acquisition_reuses_binding_and_reattaches_host() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "focused-acquisition").await;
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let session_id = format!("pair-{}", Uuid::new_v4());
    let request = |run_id: String, key| DocketFocusedExecutionAcquire {
        bear_id,
        task_id: job.tasks[0].id,
        binding: DocketFocusedExecutionBinding {
            kind: DocketExecutionBindingKind::ClientSession,
            id: session_id.clone(),
        },
        host: DocketExecutionHost {
            kind: DocketExecutionHostKind::TurnRun,
            run_id,
        },
        acquisition_key: key,
    };

    let (first, concurrent) = tokio::join!(
        service.acquire_focused_execution(request("run-one".to_string(), Uuid::new_v4())),
        service.acquire_focused_execution(request("run-one".to_string(), Uuid::new_v4())),
    );
    let first = first.expect("acquire");
    let concurrent = concurrent.expect("concurrent acquire");
    assert_eq!(first.id, concurrent.id);

    let replay = service
        .acquire_focused_execution(request("run-one".to_string(), Uuid::new_v4()))
        .await
        .expect("reuse");
    assert_eq!(first.id, replay.id);

    let attached = service
        .acquire_focused_execution(request("run-two".to_string(), Uuid::new_v4()))
        .await
        .expect("reattach host");
    assert_eq!(first.id, attached.id);
    assert_eq!(attached.host.run_id, "run-two");
    assert_eq!(attached.fence_epoch, first.fence_epoch);

    let conflict = service
        .acquire_focused_execution(DocketFocusedExecutionAcquire {
            task_id: job.tasks[1].id,
            ..request("run-three".to_string(), Uuid::new_v4())
        })
        .await;
    assert!(conflict.is_err(), "binding cannot silently switch tasks");

    let conflicting_task_owner = service
        .acquire_focused_execution(DocketFocusedExecutionAcquire {
            bear_id,
            task_id: job.tasks[0].id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: format!("other-pair-{}", Uuid::new_v4()),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id: "other-run".to_string(),
            },
            acquisition_key: Uuid::new_v4(),
        })
        .await
        .expect_err("another binding cannot acquire a task with live authority");
    assert!(
        conflicting_task_owner
            .to_string()
            .contains("already owned by binding"),
        "task ownership conflict must not leak a database unique-index error: {conflicting_task_owner}"
    );

    let released = service
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: attached.id,
            fence_epoch: attached.fence_epoch,
            recovery_key: Uuid::new_v4(),
            recovery_reason: "host controller ended".to_string(),
        })
        .await
        .expect("release");
    assert_eq!(released.state, DocketExecutionAttemptState::Released);

    let reacquired = service
        .acquire_focused_execution(request("run-four".to_string(), Uuid::new_v4()))
        .await
        .expect("reacquire after release");
    assert_ne!(released.id, reacquired.id);
    assert_eq!(released.host.run_id, "run-two");
    assert_eq!(reacquired.host.run_id, "run-four");
    assert!(service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: released.id,
            fence_epoch: released.fence_epoch,
        })
        .await
        .is_err());
}

#[tokio::test]
async fn execution_attempt_authorization_and_start_are_idempotent_and_fenced() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "execution-attempt").await;
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let task_id = job.tasks[0].id;
    let authorization_key = Uuid::new_v4();
    let request = session_authorization(
        bear_id,
        task_id,
        format!("session-{}", Uuid::new_v4()),
        format!("run_{}", Uuid::new_v4()),
        authorization_key,
    );

    let authorized = service
        .authorize_execution_attempt(request.clone())
        .await
        .expect("authorize attempt");
    let replay = service
        .authorize_execution_attempt(request)
        .await
        .expect("replay authorization");
    assert_eq!(authorized.id, replay.id);
    assert_eq!(authorized.state, DocketExecutionAttemptState::Authorized);
    assert_eq!(authorized.fence_epoch, 1);

    let started = service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: authorized.id,
            fence_epoch: authorized.fence_epoch,
        })
        .await
        .expect("start attempt");
    assert_eq!(started.state, DocketExecutionAttemptState::Running);
    assert!(started.started_at.is_some());
    let restarted = service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: authorized.id,
            fence_epoch: authorized.fence_epoch,
        })
        .await
        .expect("replay start");
    assert_eq!(restarted.started_at, started.started_at);
    assert!(service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: authorized.id,
            fence_epoch: authorized.fence_epoch + 1,
        })
        .await
        .is_err());

    let conflicting = service
        .authorize_execution_attempt(session_authorization(
            bear_id,
            task_id,
            format!("other-session-{}", Uuid::new_v4()),
            format!("run_{}", Uuid::new_v4()),
            Uuid::new_v4(),
        ))
        .await;
    assert!(conflicting.is_err(), "only one live attempt may own a task");
}

#[tokio::test]
async fn pair_bounded_outcomes_are_fenced_and_choose_canonical_yields() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "pair-bounded-outcome").await;
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let authorized = service
        .authorize_execution_attempt(session_authorization(
            bear_id,
            job.tasks[0].id,
            format!("session-{}", Uuid::new_v4()),
            format!("run_{}", Uuid::new_v4()),
            Uuid::new_v4(),
        ))
        .await
        .expect("authorize attempt");
    let running = service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: authorized.id,
            fence_epoch: authorized.fence_epoch,
        })
        .await
        .expect("start attempt");
    let progress = DocketPairBoundedOutcomeReport {
        attempt_id: running.id,
        fence_epoch: running.fence_epoch,
        outcome: DocketPairBoundedOutcome::Progress,
        awaiting_user_question: None,
    };
    let continued = service
        .report_pair_bounded_outcome(progress.clone())
        .await
        .expect("progress");
    assert_eq!(continued.decision, DocketPairContinuationDecision::Continue);
    assert_eq!(
        continued.attempt.state,
        DocketExecutionAttemptState::Running
    );
    assert_eq!(
        service
            .report_pair_bounded_outcome(progress)
            .await
            .expect("replay")
            .decision,
        DocketPairContinuationDecision::Continue
    );
    assert!(service
        .report_pair_bounded_outcome(DocketPairBoundedOutcomeReport {
            attempt_id: running.id,
            fence_epoch: running.fence_epoch + 1,
            outcome: DocketPairBoundedOutcome::AwaitingUser,
            awaiting_user_question: None,
        })
        .await
        .is_err());
    let question_key = Uuid::new_v4();
    let awaiting = service
        .report_pair_bounded_outcome(DocketPairBoundedOutcomeReport {
            attempt_id: running.id,
            fence_epoch: running.fence_epoch,
            outcome: DocketPairBoundedOutcome::AwaitingUser,
            awaiting_user_question: Some(DocketPairAwaitingUserQuestion {
                question_key,
                question_reference: "docket-entry:question".to_string(),
            }),
        })
        .await
        .expect("await user");
    assert_eq!(awaiting.decision, DocketPairContinuationDecision::AwaitUser);
    assert_eq!(
        awaiting.attempt.state,
        DocketExecutionAttemptState::AwaitingUser
    );
    let resume = DocketPairAwaitingUserResume {
        attempt_id: running.id,
        fence_epoch: running.fence_epoch,
        question_key,
        response_key: Uuid::new_v4(),
        response_reference: "docket-entry:response".to_string(),
    };
    let resumed = service
        .resume_pair_awaiting_user(resume.clone())
        .await
        .expect("authenticated resume");
    assert_eq!(resumed.state, DocketExecutionAttemptState::Authorized);
    assert_eq!(
        service
            .resume_pair_awaiting_user(resume)
            .await
            .expect("idempotent resume")
            .state,
        DocketExecutionAttemptState::Authorized
    );
    assert!(service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: running.id,
            fence_epoch: running.fence_epoch,
        })
        .await
        .is_ok());
}

#[tokio::test]
async fn released_running_attempt_is_fenced_idempotent_and_not_startable() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "execution-attempt-release").await;
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let authorized = service
        .authorize_execution_attempt(session_authorization(
            bear_id,
            job.tasks[0].id,
            format!("session-{}", Uuid::new_v4()),
            format!("run_{}", Uuid::new_v4()),
            Uuid::new_v4(),
        ))
        .await
        .expect("authorize attempt");
    let running = service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: authorized.id,
            fence_epoch: authorized.fence_epoch,
        })
        .await
        .expect("start attempt");
    let release = DocketExecutionAttemptRelease {
        attempt_id: running.id,
        fence_epoch: running.fence_epoch,
        recovery_key: Uuid::new_v4(),
        recovery_reason: "pair owner disappeared".to_string(),
    };

    let released = service
        .release_execution_attempt(release.clone())
        .await
        .expect("release attempt");
    assert_eq!(released.state, DocketExecutionAttemptState::Released);
    assert!(released.released_at.is_some());
    assert_eq!(
        service
            .release_execution_attempt(release)
            .await
            .expect("idempotent release")
            .id,
        running.id
    );
    assert!(service
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: running.id,
            fence_epoch: running.fence_epoch,
        })
        .await
        .is_err());
    assert!(service
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: running.id,
            fence_epoch: running.fence_epoch + 1,
            recovery_key: Uuid::new_v4(),
            recovery_reason: "stale reconciler".to_string(),
        })
        .await
        .is_err());
}
