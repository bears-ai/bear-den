use super::*;
use crate::{DocketService, PgDocketService};
use den_core::BearProfile;

#[sqlx::test(migrations = "../../migrations")]
async fn docket_question_requires_a_current_owned_human_session_not_a_role_label(pool: PgPool) {
    let (user, bear_id) = crate::integration_tests::seed_user_and_bear(&pool, "question").await;
    let (other_user, other_bear) =
        crate::integration_tests::seed_user_and_bear(&pool, "question-other").await;
    for (actor, bear) in [(user, bear_id), (other_user, bear_id)] {
        sqlx::query!(
            "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
            actor,
            bear,
            "member",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(crate::integration_tests::two_task_job(user, bear_id))
        .await
        .unwrap();
    let external = format!("den-conv-question-{}", Uuid::new_v4().simple());
    let conversation_id = sqlx::query_scalar!(
        "INSERT INTO conversations (bear_id, external_conversation_id, created_by_user_id)
         VALUES ($1, $2, $3) RETURNING id",
        bear_id,
        external,
        user,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let client_id = format!("client-question-{}", Uuid::new_v4().simple());
    let session_row_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO client_sessions (
            id, user_id, bear_id, bear_slug, client_session_id, runtime_session_id,
            conversation_id, client
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'integration-test')",
        session_row_id,
        user,
        bear_id,
        "question-test",
        client_id,
        "runtime-question-test",
        external,
    )
    .execute(&pool)
    .await
    .unwrap();
    let question = || DocketEntryCreate {
        bear_id,
        job_id: Some(job.job.id),
        task_id: None,
        run_id: None,
        scope: DocketEntryScope::JobNotebook,
        kind: DocketEntryKind::Question,
        summary: "Can we proceed?".into(),
        body: None,
        evidence_refs: vec![],
        related_task_ids: vec![],
        tags: vec![],
        question_client_session_id: Some(client_id.clone()),
        actor_role: BearProfile::Work,
        actor_user_id: Some(user),
        actor_agent_id: None,
    };
    let recorded = service.append_entry(question()).await.unwrap();
    assert_eq!(recorded.kind, "question");
    assert_eq!(recorded.by_role, "work"); // audit label is not the grant
    for attempted in [
        DocketEntryCreate {
            question_client_session_id: None,
            ..question()
        },
        DocketEntryCreate {
            question_client_session_id: Some("guessed-session".into()),
            ..question()
        },
        DocketEntryCreate {
            actor_user_id: Some(other_user),
            ..question()
        },
        DocketEntryCreate {
            bear_id: other_bear,
            ..question()
        },
    ] {
        assert!(matches!(
            service.append_entry(attempted).await,
            Err(DenError::Authorization(_))
        ));
    }
    let work_run_id = sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id, state, bearwire_session_id)
         VALUES ($1, $2, $3, 'running', $4) RETURNING id",
        bear_id,
        job.job.id,
        job.job.current_run_id.expect("created Job has a run"),
        client_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service.append_entry(question()).await,
        Err(DenError::Authorization(_))
    ));
    sqlx::query!(
        "UPDATE bear_work_runs SET state = 'failed' WHERE id = $1",
        work_run_id
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE client_sessions SET closed_at = NOW() WHERE id = $1",
        session_row_id
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service.append_entry(question()).await,
        Err(DenError::Authorization(_))
    ));
    sqlx::query!(
        "UPDATE client_sessions SET closed_at = NULL WHERE id = $1",
        session_row_id
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE conversations SET status = 'archived' WHERE id = $1",
        conversation_id
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service.append_entry(question()).await,
        Err(DenError::Authorization(_))
    ));
    sqlx::query!(
        "UPDATE conversations SET status = 'active' WHERE id = $1",
        conversation_id
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        user,
        bear_id
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service.append_entry(question()).await,
        Err(DenError::Authorization(_))
    ));
}
