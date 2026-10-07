use bearwire_protocol::session::ExpectedWorkSource;
use den_docket::{work_runs, DocketExecutionAttemptRelease, DocketService, PgDocketService};
use den_http::errors::CustomError;
use den_runtime::turn_runs;
use den_service::{
    archived_conversations,
    bears::{db as bears_db, hats},
    client_sessions,
    conversation::{persistence, viewer::ConversationViewer},
    DenState,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    admit,
    helpers::{self, Fixture},
    require_existing_session, require_expected_work_source,
};

async fn transcript(pool: &PgPool, fixture: &Fixture, owner: Option<i32>, bound: bool) -> String {
    let external = format!("den-conv-{}", Uuid::new_v4().simple());
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        fixture.bear.as_uuid(),
        owner,
        &external,
        None,
        Some("Unchanged title"),
    )
    .await
    .unwrap();
    if bound {
        let hat = hats::ide_default_hat(pool, fixture.bear)
            .await
            .unwrap()
            .unwrap();
        hats::bindings::bind_conversation_hat(pool, fixture.bear, conversation.id, hat)
            .await
            .unwrap();
    }
    external
}

async fn session(pool: &PgPool, fixture: &Fixture, stored: &str, resolved: Option<&str>) {
    client_sessions::upsert_session(
        pool,
        client_sessions::UpsertClientSession {
            user_id: fixture.user.get(),
            bear_id: fixture.bear.as_uuid(),
            bear_slug: fixture.slug.clone(),
            client_session_id: fixture.session.clone(),
            runtime_session_id: "original-runtime".into(),
            conversation_id: stored.to_owned(),
            resolved_conversation_id: resolved.map(str::to_owned),
            client: "original-client".into(),
            cwd: Some("/original".into()),
            current_mode: None,
        },
    )
    .await
    .unwrap();
}

async fn snapshot(pool: &PgPool, fixture: &Fixture, targets: &[&str]) -> Value {
    let viewer = ConversationViewer::resolve(pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let mut transcripts = Vec::new();
    for target in targets {
        let canonical =
            persistence::get_conversation_for_external_id(pool, fixture.bear.as_uuid(), target)
                .await
                .unwrap();
        let details = if let Some(canonical) = &canonical {
            json!({
                "hat": hats::bindings::conversation_hat(pool, fixture.bear, canonical.id).await.unwrap().map(|id| id.as_uuid()),
                "model": format!("{:?}", persistence::get_conversation_model_state(pool, canonical.id).await.unwrap()),
                "messages": format!("{:?}", persistence::list_messages_page(pool, canonical.id, None, 100).await.unwrap()),
                "owned_live": viewer.may_read_own_source(pool, canonical.id).await.unwrap(),
            })
        } else {
            Value::Null
        };
        transcripts.push(json!({"target": target, "canonical": canonical, "details": details}));
    }
    json!({
        "session": client_sessions::find_for_user_bear_session_id(pool, fixture.user.get(), fixture.bear.as_uuid(), &fixture.session).await.unwrap(),
        "active_run": format!("{:?}", turn_runs::active_run_for_session(pool, &fixture.session).await.unwrap()),
        "events": format!("{:?}", den_runtime::bearwire_events::list_bearwire_events_after(pool, &fixture.session, None, 100).await.unwrap()),
        "transcripts": transcripts,
    })
}

async fn recovery_snapshot(pool: &PgPool, run_id: &str) -> Value {
    let row = turn_runs::technical_budget_recovery_snapshot(pool, run_id)
        .await
        .unwrap()
        .unwrap();
    json!({
        "run_id": row.run_id, "reason": row.reason, "snapshot": row.snapshot,
        "lease_id": row.recovery_lease_id, "lease_expires_at": row.recovery_lease_expires_at,
        "recovered_at": row.recovered_at, "created_at": row.created_at, "updated_at": row.updated_at,
    })
}

async fn denied_unchanged(
    pool: &PgPool,
    fixture: &Fixture,
    state: &DenState,
    requested: Option<&str>,
    expected: Option<ExpectedWorkSource>,
    targets: &[&str],
) -> CustomError {
    let before = snapshot(pool, fixture, targets).await;
    let error = crate::methods::run::run_start_result(state, &fixture.headers(), &json!({
        "bear_slug": fixture.slug, "session_id": fixture.session, "conversation_id": requested,
        "expected_work_source": expected, "prompt": "Must not write or infer",
        "client": "replacement-client", "cwd": "/replacement", "client_context": {"workspace_roots": ["/replacement"]},
    })).await.unwrap_err();
    assert!(
        matches!(error, CustomError::Authorization(_)),
        "expected an authority denial, not a model error: {error:?}"
    );
    assert_eq!(
        snapshot(pool, fixture, targets).await,
        before,
        "denied startup changed persistence"
    );
    error
}

async fn ordinary_fixture(pool: &PgPool) -> Fixture {
    let fixture = Fixture::new(pool).await;
    work_runs::bind_work_run_session(
        pool,
        fixture.expected.work_run_id,
        fixture.bear.as_uuid(),
        &format!("other-{}", Uuid::new_v4()),
    )
    .await
    .unwrap();
    fixture
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_start_requires_independent_live_owned_transcript(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let other = helpers::user(&pool).await;
    let state = helpers::model_ready_state(&pool, &fixture).await;
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    for (owner, inactive, archived_marker) in [
        (Some(other.get()), false, false),
        (None, false, false),
        (Some(fixture.user.get()), true, false),
        (Some(fixture.user.get()), false, true),
    ] {
        let target = transcript(&pool, &fixture, owner, false).await;
        let canonical =
            persistence::get_conversation_for_external_id(&pool, fixture.bear.as_uuid(), &target)
                .await
                .unwrap()
                .unwrap();
        assert!(
            viewer.may_access_external(&pool, &target).await.unwrap(),
            "the admin can inspect history"
        );
        if inactive {
            sqlx::query!(
                "UPDATE conversations SET status = 'archived' WHERE id = $1",
                canonical.id
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        if archived_marker {
            archived_conversations::set_archived(
                &pool,
                fixture.bear.as_uuid(),
                &target,
                Some(fixture.user.get()),
                "test",
                true,
            )
            .await
            .unwrap();
            assert!(
                viewer
                    .may_read_own_source(&pool, canonical.id)
                    .await
                    .unwrap(),
                "archive marker must deny an otherwise-active owned row"
            );
        }
        require_expected_work_source(
            &pool,
            fixture.bear,
            fixture.user,
            &fixture.session,
            fixture.expected,
        )
        .await
        .unwrap();
        for expected in [None, Some(fixture.expected)] {
            denied_unchanged(&pool, &fixture, &state, Some(&target), expected, &[&target]).await;
            let before = snapshot(&pool, &fixture, &[&target]).await;
            let error = admit(
                &pool,
                &viewer,
                fixture.bear,
                fixture.user,
                &fixture.session,
                &target,
                expected,
            )
            .await
            .err()
            .unwrap();
            assert!(matches!(error, CustomError::Authorization(_)));
            assert_eq!(snapshot(&pool, &fixture, &[&target]).await, before);
        }
    }
    // A live owned Work transcript does not need an ordinary conversation hat.
    let owned = transcript(&pool, &fixture, Some(fixture.user.get()), false).await;
    let source = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        &owned,
        Some(fixture.expected),
    )
    .await
    .unwrap();
    assert_eq!(
        source.turn_source,
        hats::turn_binding::NativeTurnSource::WorkRun(fixture.expected.work_run_id)
    );
    assert!(
        hats::bindings::conversation_hat(&pool, fixture.bear, source.conversation.id)
            .await
            .unwrap()
            .is_none()
    );
    let bear = bears_db::bear_for_user_by_slug(&pool, fixture.user.get(), &fixture.slug)
        .await
        .unwrap()
        .unwrap();
    super::super::preflight_pair_run_model(
        &state,
        &bear,
        &fixture.session,
        &owned,
        source.turn_source,
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_cannot_substitute_own_or_fresh_selection_for_read_only_session(pool: PgPool) {
    let fixture = ordinary_fixture(&pool).await;
    let other = helpers::user(&pool).await;
    let own = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
    let fresh = format!("new-acp-{}", Uuid::new_v4());
    let state = helpers::state(pool.clone()); // An unavailable model must not mask authority denials.
    for owner in [Some(other.get()), None, Some(fixture.user.get())] {
        let stored = transcript(&pool, &fixture, owner, false).await;
        session(&pool, &fixture, &stored, None).await;
        for requested in [&own, &fresh] {
            denied_unchanged(
                &pool,
                &fixture,
                &state,
                Some(requested),
                None,
                &[&stored, &own, &fresh],
            )
            .await;
        }
        denied_unchanged(&pool, &fixture, &state, None, None, &[&stored]).await;
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_denies_closed_archived_and_marker_only_sessions_before_model_preflight(
    pool: PgPool,
) {
    let fixture = ordinary_fixture(&pool).await;
    let stored = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
    let state = helpers::state(pool.clone());
    for archived in [false, true] {
        session(&pool, &fixture, &stored, None).await;
        let row = client_sessions::find_for_user_bear_session_id(
            &pool,
            fixture.user.get(),
            fixture.bear.as_uuid(),
            &fixture.session,
        )
        .await
        .unwrap()
        .unwrap();
        if archived {
            client_sessions::mark_archived(&pool, row.id).await.unwrap();
        } else {
            client_sessions::mark_closed(&pool, row.id).await.unwrap();
        }
        denied_unchanged(&pool, &fixture, &state, None, None, &[&stored]).await;
    }
    session(&pool, &fixture, &stored, None).await;
    archived_conversations::set_archived(
        &pool,
        fixture.bear.as_uuid(),
        &stored,
        Some(fixture.user.get()),
        "test",
        true,
    )
    .await
    .unwrap();
    denied_unchanged(&pool, &fixture, &state, None, None, &[&stored]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_start_requires_both_stored_and_resolved_sources_to_remain_live(pool: PgPool) {
    let fixture = ordinary_fixture(&pool).await;
    let other = helpers::user(&pool).await;
    let owned = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
    let foreign = transcript(&pool, &fixture, Some(other.get()), true).await;
    let unbound = transcript(&pool, &fixture, Some(fixture.user.get()), false).await;
    let missing = format!("den-conv-{}", Uuid::new_v4());
    let pending = format!("new-acp-{}", Uuid::new_v4());
    let state = helpers::state(pool.clone());
    for (stored, resolved) in [
        (&foreign, &owned),
        (&unbound, &owned),
        (&owned, &foreign),
        (&owned, &unbound),
        (&pending, &missing),
        (&missing, &owned),
    ] {
        session(&pool, &fixture, stored, Some(resolved)).await;
        denied_unchanged(
            &pool,
            &fixture,
            &state,
            Some(resolved),
            None,
            &[stored, resolved],
        )
        .await;
    }
    session(&pool, &fixture, &owned, Some(&owned)).await;
    archived_conversations::set_archived(
        &pool,
        fixture.bear.as_uuid(),
        &owned,
        Some(fixture.user.get()),
        "test",
        true,
    )
    .await
    .unwrap();
    denied_unchanged(&pool, &fixture, &state, Some(&owned), None, &[&owned]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn exact_work_start_never_attaches_pair_old_work_or_old_fence_with_valid_model(pool: PgPool) {
    for prior_kind in ["pair", "old_work", "old_fence"] {
        let mut fixture = Fixture::new(&pool).await;
        let stored = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
        session(&pool, &fixture, &stored, None).await;
        let prior = if prior_kind == "pair" {
            None
        } else {
            Some(fixture.expected)
        };
        let active_id = format!("run_{}", Uuid::new_v4().simple());
        turn_runs::create_run(
            &pool,
            &active_id,
            &fixture.session,
            fixture.bear.as_uuid(),
            fixture.user.get(),
        )
        .await
        .unwrap();
        turn_runs::transition_run(&pool, &active_id, turn_runs::TurnRunState::Running, None)
            .await
            .unwrap();
        let payload = super::super::technical_budget_recovery_start_payload(
            "original-client",
            None,
            &stored,
            "Original turn",
            None,
            None,
            None,
            prior,
        );
        let original = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
            fixture.session.clone(),
            fixture.bear.as_uuid(),
            fixture.user.get(),
            None,
            payload,
        ))
        .unwrap();
        turn_runs::claim_technical_budget_continuation(
            &pool,
            &active_id,
            "emergency_hard_step_limit",
            &original,
        )
        .await
        .unwrap();
        if prior_kind == "old_work" {
            let (replacement, _, _, _) = helpers::checkout(
                &pool,
                fixture.bear,
                fixture.user,
                fixture.token.id,
                &format!("replacement-{}", Uuid::new_v4()),
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
            fixture.expected = replacement;
        } else if prior_kind == "old_fence" {
            PgDocketService::from_pool(&pool)
                .release_execution_attempt(DocketExecutionAttemptRelease {
                    attempt_id: fixture.expected.execution_attempt_id,
                    fence_epoch: fixture.expected.fence_epoch,
                    recovery_key: Uuid::new_v4(),
                    recovery_reason: "new startup epoch".into(),
                })
                .await
                .unwrap();
            let checked = work_runs::checkout_work_run_for_session(
                &pool,
                fixture.expected.work_run_id,
                fixture.bear.as_uuid(),
                &fixture.session,
            )
            .await
            .unwrap();
            let attempt = checked.execution_attempt.unwrap();
            assert_eq!(attempt.id, fixture.expected.execution_attempt_id);
            assert!(attempt.fence_epoch > fixture.expected.fence_epoch);
            fixture.expected.fence_epoch = attempt.fence_epoch;
        }
        require_expected_work_source(
            &pool,
            fixture.bear,
            fixture.user,
            &fixture.session,
            fixture.expected,
        )
        .await
        .unwrap();
        let state = helpers::model_ready_state(&pool, &fixture).await;
        let bear = bears_db::bear_for_user_by_slug(&pool, fixture.user.get(), &fixture.slug)
            .await
            .unwrap()
            .unwrap();
        super::super::preflight_pair_run_model(
            &state,
            &bear,
            &fixture.session,
            &stored,
            hats::turn_binding::NativeTurnSource::WorkRun(fixture.expected.work_run_id),
        )
        .await
        .unwrap();
        let error = denied_unchanged(
            &pool,
            &fixture,
            &state,
            Some(&stored),
            Some(fixture.expected),
            &[&stored],
        )
        .await;
        assert!(
            matches!(error, CustomError::Authorization(message) if message.contains("cannot prove the active turn"))
        );
        assert_eq!(
            turn_runs::technical_budget_recovery_snapshot(&pool, &active_id)
                .await
                .unwrap()
                .unwrap()
                .snapshot,
            original
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn exact_work_active_turn_denial_precedes_transcript_creation(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let pending = format!("new-acp-{}", Uuid::new_v4());
    session(&pool, &fixture, &pending, None).await;
    turn_runs::create_run(
        &pool,
        &format!("run_{}", Uuid::new_v4().simple()),
        &fixture.session,
        fixture.bear.as_uuid(),
        fixture.user.get(),
    )
    .await
    .unwrap();
    let state = helpers::model_ready_state(&pool, &fixture).await;
    denied_unchanged(
        &pool,
        &fixture,
        &state,
        None,
        Some(fixture.expected),
        &[&pending],
    )
    .await;
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    assert!(viewer.list_visible(&pool, 200).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_default_and_resolved_alias_keep_the_session_selection(pool: PgPool) {
    let fixture = ordinary_fixture(&pool).await;
    let pending = format!("new-acp-{}", Uuid::new_v4());
    session(&pool, &fixture, &pending, None).await;
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let row = client_sessions::find_for_user_bear_session_id(
        &pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &fixture.session,
    )
    .await
    .unwrap()
    .unwrap();
    require_existing_session(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &row,
        Some(&pending),
    )
    .await
    .unwrap();
    let admitted = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        &pending,
        None,
    )
    .await
    .unwrap();
    let canonical = admitted.conversation.external_conversation_id.unwrap();
    assert_ne!(canonical, pending);
    assert_eq!(
        hats::bindings::conversation_hat(&pool, fixture.bear, admitted.conversation.id)
            .await
            .unwrap(),
        hats::ide_default_hat(&pool, fixture.bear).await.unwrap()
    );
    session(&pool, &fixture, &pending, Some(&canonical)).await;
    turn_runs::create_run(
        &pool,
        &format!("run_{}", Uuid::new_v4().simple()),
        &fixture.session,
        fixture.bear.as_uuid(),
        fixture.user.get(),
    )
    .await
    .unwrap();
    let state = helpers::model_ready_state(&pool, &fixture).await;
    let result = crate::methods::run::run_start_result(&state, &fixture.headers(), &json!({
        "bear_slug": fixture.slug, "session_id": fixture.session, "conversation_id": canonical, "prompt": "Continue existing source",
    })).await.unwrap();
    assert_eq!(result["reused"], true);
    let after = client_sessions::find_for_user_bear_session_id(
        &pool,
        fixture.user.get(),
        fixture.bear.as_uuid(),
        &fixture.session,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(after.conversation_id, pending);
    assert_eq!(after.resolved_conversation_id, Some(canonical));
}

#[sqlx::test(migrations = "../../migrations")]
async fn recovery_cannot_substitute_a_different_owned_transcript_before_leasing(pool: PgPool) {
    let fixture = ordinary_fixture(&pool).await;
    let stored = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
    let substituted = transcript(&pool, &fixture, Some(fixture.user.get()), true).await;
    session(&pool, &fixture, &stored, None).await;
    let active = format!("run_{}", Uuid::new_v4().simple());
    turn_runs::create_run(
        &pool,
        &active,
        &fixture.session,
        fixture.bear.as_uuid(),
        fixture.user.get(),
    )
    .await
    .unwrap();
    turn_runs::transition_run(&pool, &active, turn_runs::TurnRunState::Running, None)
        .await
        .unwrap();
    let payload = super::super::technical_budget_recovery_start_payload(
        "test",
        None,
        &substituted,
        "Stale source",
        None,
        None,
        None,
        None,
    );
    let original = serde_json::to_value(turn_runs::TechnicalBudgetRecoverySnapshot::new(
        fixture.session.clone(),
        fixture.bear.as_uuid(),
        fixture.user.get(),
        Some(fixture.task),
        payload,
    ))
    .unwrap();
    turn_runs::claim_technical_budget_continuation(
        &pool,
        &active,
        "emergency_hard_step_limit",
        &original,
    )
    .await
    .unwrap();
    let state = helpers::model_ready_state(&pool, &fixture).await;
    let before = snapshot(&pool, &fixture, &[&stored, &substituted]).await;
    let recovery_before = recovery_snapshot(&pool, &active).await;
    let error = crate::methods::run::run_recover_result(
        &state,
        &fixture.headers(),
        &json!({
            "bear_slug": fixture.slug, "run_id": active,
        }),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, CustomError::Authorization(message) if message.contains("cannot change the canonical session conversation"))
    );
    assert_eq!(
        snapshot(&pool, &fixture, &[&stored, &substituted]).await,
        before
    );
    assert_eq!(recovery_snapshot(&pool, &active).await, recovery_before);
}

#[test]
fn persisted_execution_target_is_parsed_once_into_the_existing_enum() {
    use den_docket::work_runs::WorkExecutionTarget;
    assert_eq!(
        super::parse_work_execution_target("sandbox", None).unwrap(),
        WorkExecutionTarget::Sandbox
    );
    assert_eq!(
        super::parse_work_execution_target("attached_armature", Some("owned-session")).unwrap(),
        WorkExecutionTarget::AttachedArmature {
            client_session_id: "owned-session".into()
        }
    );
    for (raw, assigned) in [
        ("sandbox", Some("unexpected")),
        ("attached_armature", None),
        ("attached_armature", Some("")),
        ("future_target", None),
    ] {
        assert!(matches!(
            super::parse_work_execution_target(raw, assigned),
            Err(CustomError::Authorization(_))
        ));
    }
}
