use bearwire_protocol::session::ExpectedWorkSource;
use den_core::{ArmatureAvailability, TurnExecutionOrigin};
use den_docket::{
    work_runs, DocketExecutionAttemptAuthorize, DocketExecutionAttemptRelease,
    DocketExecutionAttemptStart, DocketExecutionBindingKind, DocketExecutionHost,
    DocketExecutionHostKind, DocketFocusedExecutionBinding, DocketService, PgDocketService,
};
use den_service::{
    bears::{db as bears_db, hats},
    client_sessions,
    conversation::{persistence, viewer::ConversationViewer},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::helpers::{self, Fixture};
use super::{admit, require_expected_work_source};

async fn require(
    pool: &PgPool,
    fixture: &Fixture,
    expected: ExpectedWorkSource,
) -> Result<(), den_http::errors::CustomError> {
    require_expected_work_source(pool, fixture.bear, fixture.user, &fixture.session, expected).await
}

async fn rejected_before_writes(pool: &PgPool, fixture: &Fixture, expected: ExpectedWorkSource) {
    let conversation_id = format!("expected-work-history-{}", Uuid::new_v4().simple());
    let state = helpers::state(pool.clone());
    let params = json!({
        "bear_slug": fixture.slug,
        "session_id": fixture.session,
        "conversation_id": conversation_id,
        "prompt": "Must not infer or create state",
        "expected_work_source": expected,
    });
    assert!(
        crate::methods::run::run_start_result(&state, &fixture.headers(), &params)
            .await
            .is_err()
    );
    assert!(client_sessions::find_for_user_bear_session_id(
        pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &fixture.session
    )
    .await
    .unwrap()
    .is_none());
    assert!(persistence::get_conversation_for_external_id(
        pool,
        fixture.bear.as_uuid(),
        &conversation_id
    )
    .await
    .unwrap()
    .is_none());
    assert!(
        den_runtime::turn_runs::active_run_for_session(pool, &fixture.session)
            .await
            .unwrap()
            .is_none()
    );
    let viewer = ConversationViewer::resolve(pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    assert!(admit(
        pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        &conversation_id,
        Some(expected)
    )
    .await
    .is_err());
    assert!(persistence::get_conversation_for_external_id(
        pool,
        fixture.bear.as_uuid(),
        &conversation_id
    )
    .await
    .unwrap()
    .is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_admits_exact_checkout_without_open_session(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    require(&pool, &fixture, fixture.expected).await.unwrap();
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let source = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        "new-work-test",
        Some(fixture.expected),
    )
    .await
    .unwrap();
    assert_eq!(
        source.origin,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected)
    );
    assert_eq!(
        source.turn_source,
        hats::turn_binding::NativeTurnSource::WorkRun(fixture.expected.work_run_id)
    );
    let hats::memory_binding::ResolvedMemoryBinding::Bound(grant) =
        hats::memory_binding::for_work_run(&pool, fixture.bear, fixture.expected.work_run_id)
            .await
            .unwrap();
    assert_eq!(grant.hat_id(), Some(fixture.hat));
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_missing_binding_never_uses_valid_ide_default(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    // An initial successful read must not turn a later missing association into Pair authority.
    require(&pool, &fixture, fixture.expected).await.unwrap();
    work_runs::bind_work_run_session(
        &pool,
        fixture.expected.work_run_id,
        fixture.bear.as_uuid(),
        "moved-work-session",
    )
    .await
    .unwrap();
    assert!(
        work_runs::get_live_work_run_by_session(&pool, &fixture.session)
            .await
            .unwrap()
            .is_none()
    );
    assert!(hats::ide_default_hat(&pool, fixture.bear)
        .await
        .unwrap()
        .is_some());
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let ordinary = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        "new-valid-ide-default",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        ordinary.origin,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_replaced_binding_never_uses_valid_ide_default(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    require(&pool, &fixture, fixture.expected).await.unwrap();
    let (replacement, _, _, _) = helpers::checkout(
        &pool,
        fixture.bear,
        fixture.user,
        fixture.token.id,
        "replacement-session",
    )
    .await;
    work_runs::bind_work_run_session(
        &pool,
        replacement.work_run_id,
        fixture.bear.as_uuid(),
        &fixture.session,
    )
    .await
    .unwrap();
    assert_eq!(
        work_runs::get_live_work_run_by_session(&pool, &fixture.session)
            .await
            .unwrap()
            .unwrap()
            .id,
        replacement.work_run_id
    );
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    require(&pool, &fixture, replacement).await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_wrong_bear_run_attempt_and_fence(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let other = Fixture::new(&pool).await;
    assert!(require_expected_work_source(
        &pool,
        other.bear,
        fixture.user,
        &fixture.session,
        fixture.expected
    )
    .await
    .is_err());
    assert!(require_expected_work_source(
        &pool,
        fixture.bear,
        fixture.user,
        &other.session,
        fixture.expected
    )
    .await
    .is_err());
    for expected in [
        ExpectedWorkSource {
            work_run_id: Uuid::new_v4(),
            ..fixture.expected
        },
        ExpectedWorkSource {
            work_run_id: other.expected.work_run_id,
            ..fixture.expected
        },
        ExpectedWorkSource {
            execution_attempt_id: Uuid::new_v4(),
            ..fixture.expected
        },
        ExpectedWorkSource {
            execution_attempt_id: other.expected.execution_attempt_id,
            ..fixture.expected
        },
        ExpectedWorkSource {
            fence_epoch: fixture.expected.fence_epoch + 1,
            ..fixture.expected
        },
        ExpectedWorkSource {
            fence_epoch: 0,
            ..fixture.expected
        },
        ExpectedWorkSource {
            fence_epoch: -1,
            ..fixture.expected
        },
    ] {
        rejected_before_writes(&pool, &fixture, expected).await;
    }
    require(&pool, &fixture, fixture.expected).await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_released_and_replaced_attempt(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let docket = PgDocketService::from_pool(&pool);
    docket
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: fixture.expected.execution_attempt_id,
            fence_epoch: fixture.expected.fence_epoch,
            recovery_key: Uuid::new_v4(),
            recovery_reason: "replace exact attempt".into(),
        })
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    let replacement = docket
        .authorize_execution_attempt(DocketExecutionAttemptAuthorize {
            bear_id: fixture.bear.as_uuid(),
            task_id: fixture.task,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::WorkAssignment,
                id: fixture.expected.work_run_id.to_string(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::WorkRun,
                run_id: fixture.expected.work_run_id.to_string(),
            },
            authorization_key: Uuid::new_v4(),
        })
        .await
        .unwrap();
    docket
        .start_execution_attempt(DocketExecutionAttemptStart {
            attempt_id: replacement.id,
            fence_epoch: replacement.fence_epoch,
        })
        .await
        .unwrap();
    assert_ne!(replacement.id, fixture.expected.execution_attempt_id);
    // A new authorization row can have the same fence; the attempt UUID must also match.
    assert_eq!(replacement.fence_epoch, fixture.expected.fence_epoch);
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_cancelled_and_terminal_run(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    work_runs::request_work_run_cancel(&pool, fixture.expected.work_run_id, fixture.bear.as_uuid())
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    work_runs::finalize_work_run(
        &pool,
        fixture.expected.work_run_id,
        work_runs::WorkRunState::Cancelled,
        work_runs::WorkRunFinalize {
            result_summary: None,
            result_refs: None,
            usage: None,
            error: None,
        },
    )
    .await
    .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_revoked_hat_and_surface(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    hats::manage::replace_surfaces(&pool, fixture.bear, fixture.hat, &[])
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    let fixture = Fixture::new(&pool).await;
    hats::manage::disable_work(&pool, fixture.bear, fixture.hat)
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_revoked_token_and_wrong_token_provenance(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    den_http::armature_tokens::revoke_for_user(&pool, fixture.user.get(), fixture.token.id)
        .await
        .unwrap();
    // Check the guard itself, not just authentication of a revoked presented token.
    assert!(require(&pool, &fixture, fixture.expected).await.is_err());
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    let fixture = Fixture::new(&pool).await;
    let other = Fixture::new(&pool).await;
    work_runs::merge_work_run_result_refs(
        &pool,
        fixture.expected.work_run_id,
        &json!({"armature_token_id": other.token.id}),
    )
    .await
    .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    work_runs::merge_work_run_result_refs(
        &pool,
        fixture.expected.work_run_id,
        &json!({
            "armature_token_id": fixture.token.id, "armature_token_user_id": other.user.get(),
        }),
    )
    .await
    .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    work_runs::merge_work_run_result_refs(
        &pool,
        fixture.expected.work_run_id,
        &json!({"armature_token_id": null}),
    )
    .await
    .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_other_session_owner_and_job_actor(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let other_user = helpers::user(&pool).await;
    bears_db::grant_membership(
        &pool,
        other_user.get(),
        fixture.bear.as_uuid(),
        Some(bears_db::BEAR_ROLE_ADMIN),
    )
    .await
    .unwrap();
    assert!(require_expected_work_source(
        &pool,
        fixture.bear,
        other_user,
        &fixture.session,
        fixture.expected
    )
    .await
    .is_err());
    client_sessions::upsert_session(
        &pool,
        client_sessions::UpsertClientSession {
            user_id: other_user.get(),
            bear_id: fixture.bear.as_uuid(),
            bear_slug: fixture.slug.clone(),
            client_session_id: fixture.session.clone(),
            runtime_session_id: "foreign-owner".into(),
            conversation_id: "new-foreign-conversation".into(),
            resolved_conversation_id: None,
            client: "test".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .unwrap();
    assert!(require(&pool, &fixture, fixture.expected).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn expected_work_source_rejects_changed_job_run_and_pending_checkpoint(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    PgDocketService::from_pool(&pool)
        .check_work_boundary(den_docket::DocketWorkBoundaryCheck {
            bear_id: fixture.bear.as_uuid(),
            attempt_id: fixture.expected.execution_attempt_id,
            fence_epoch: fixture.expected.fence_epoch,
            boundary_key: Uuid::new_v4(),
            signal: Some(den_docket::DocketWorkBoundarySignal::NearKo),
        })
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
    let fixture = Fixture::new(&pool).await;
    PgDocketService::from_pool(&pool)
        .cancel_job_run(fixture.bear.as_uuid(), fixture.job)
        .await
        .unwrap();
    rejected_before_writes(&pool, &fixture, fixture.expected).await;
}

#[test]
fn expected_work_source_recovery_preserves_only_the_supplied_expectation() {
    let expected = ExpectedWorkSource {
        work_run_id: Uuid::new_v4(),
        execution_attempt_id: Uuid::new_v4(),
        fence_epoch: 3,
    };
    let payload = super::super::technical_budget_recovery_start_payload(
        "test",
        None,
        "conversation",
        "prompt",
        None,
        None,
        None,
        Some(expected),
    );
    let decoded: super::super::TechnicalBudgetRecoveryStartPayload =
        serde_json::from_value(payload).unwrap();
    assert_eq!(decoded.expected_work_source, Some(expected));
    let old_payload = json!({"client": "test", "cwd": null, "conversation_id": "conversation", "prompt": "prompt", "prompt_context": null, "client_context": null, "requested_mode": null});
    let decoded: super::super::TechnicalBudgetRecoveryStartPayload =
        serde_json::from_value(old_payload).unwrap();
    assert_eq!(decoded.expected_work_source, None);
}
