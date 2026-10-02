use den_core::{BearProfile, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    integration_tests::{seed_client_session, seed_user_and_bear, test_pool, two_task_job},
    DocketEffortHint, DocketService, DocketSessionTaskSettlement, DocketTaskCreate,
    DocketTaskDifficulty, DocketTaskKind, DocketTaskScope, DocketTaskStatus, PgDocketService,
    RoutingStrategy, TaskListCheckoutRequest, TaskListCheckoutSource, TaskListVisibility,
};

async fn grant_membership(pool: &PgPool, user_id: i32, bear_id: Uuid, role: &str) {
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        user_id,
        bear_id,
        role,
    )
    .execute(pool)
    .await
    .expect("grant Bear membership");
}

fn checkout(job_id: Uuid, session_anchor_id: Uuid) -> TaskListCheckoutRequest {
    TaskListCheckoutRequest {
        source: TaskListCheckoutSource::DocketJob {
            job_id,
            parent_task_id: None,
        },
        session_anchor_id: Some(session_anchor_id),
    }
}

async fn standalone_task(
    service: &PgDocketService,
    bear_id: Uuid,
    session_anchor_id: Uuid,
    user_id: i32,
    title: &str,
) -> Uuid {
    service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: None,
            session_anchor_id: Some(session_anchor_id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: title.to_string(),
            body: format!("Private details for {title}"),
            completion_criteria: vec!["Done".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .expect("create standalone task")
        .id
}

#[tokio::test]
async fn exact_task_access_and_session_listing_exclude_historical_foreign_attachments() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket task access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "task-access-owner").await;
    let (viewer, foreign_bear) = seed_user_and_bear(&pool, "task-access-viewer").await;
    let admin = seed_user_and_bear(&pool, "task-access-admin").await.0;
    let outsider = seed_user_and_bear(&pool, "task-access-outsider").await.0;
    for (user_id, role) in [(viewer, "member"), (admin, "admin")] {
        grant_membership(&pool, user_id, bear_id, role).await;
    }
    let service = PgDocketService::from_pool(&pool);
    let private = service
        .create_job(
            two_task_job(owner, bear_id),
            crate::DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .expect("create SameUser job");
    let mut visible_create = two_task_job(owner, bear_id);
    visible_create.visibility = TaskListVisibility::BearVisible;
    let visible = service
        .create_job(
            visible_create,
            crate::DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .expect("create BearVisible job");
    let viewer_session = seed_client_session(&pool, viewer, bear_id).await;
    let owner_session = seed_client_session(&pool, owner, bear_id).await;
    let own_task = standalone_task(&service, bear_id, viewer_session, viewer, "own task").await;
    let foreign_task =
        standalone_task(&service, bear_id, owner_session, owner, "foreign task").await;

    // Historical trusted attachments must not grant a human access to another
    // member's job, even when attached to their own client session.
    service
        .attach_task_to_session(bear_id, private.tasks[0].id, viewer_session)
        .await
        .expect("historically attach private job task");
    service
        .attach_task_to_session(bear_id, visible.tasks[0].id, viewer_session)
        .await
        .expect("attach visible job task");
    service
        .attach_task_to_session(bear_id, visible.tasks[1].id, owner_session)
        .await
        .expect("attach another visible task to foreign session");
    // Reproduce a stale/foreign standalone binding. The task's original session
    // is not stored on bear_tasks after the attachment migration.
    sqlx::query!(
        "UPDATE bear_session_task_attachments SET session_id = $2 WHERE task_id = $1",
        foreign_task,
        viewer_session,
    )
    .execute(&pool)
    .await
    .expect("simulate historical foreign attachment");

    let trusted = service
        .list_session_tasks(bear_id, viewer_session)
        .await
        .expect("trusted list remains unfiltered");
    assert_eq!(trusted.len(), 4);
    let human = service
        .list_session_tasks_for_human(bear_id, viewer_session, viewer)
        .await
        .expect("viewer-scoped session list");
    let ids: Vec<_> = human.iter().map(|task| task.task.id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&own_task));
    assert!(ids.contains(&visible.tasks[0].id));
    assert!(human
        .iter()
        .all(|task| !task.task.title.contains("foreign")));

    let checked_out = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            viewer,
            checkout(visible.job.id, viewer_session),
        )
        .await
        .expect("checkout visible job")
        .expect("checkout projection");
    assert!(checked_out.items.iter().all(|item| {
        item.id != private.tasks[0].id.to_string() && item.id != foreign_task.to_string()
    }));

    for (user_id, task_id) in [
        (viewer, visible.tasks[0].id),
        (viewer, own_task),
        (owner, private.tasks[0].id),
        (admin, private.tasks[0].id),
    ] {
        service
            .authorize_task_for_human(bear_id, task_id, user_id)
            .await
            .expect("authorized exact task");
    }
    for (user_id, task_id) in [
        (viewer, private.tasks[0].id),
        (viewer, foreign_task),
        (owner, foreign_task),
        (admin, foreign_task),
        (outsider, visible.tasks[0].id),
        (viewer, Uuid::new_v4()),
    ] {
        assert!(matches!(
            service
                .authorize_task_for_human(bear_id, task_id, user_id)
                .await,
            Err(DenError::NotFound(_)),
        ));
    }
    assert!(matches!(
        service
            .authorize_task_for_human(foreign_bear, visible.tasks[0].id, viewer)
            .await,
        Err(DenError::NotFound(_)),
    ));
    assert_eq!(
        service
            .list_session_tasks(bear_id, owner_session)
            .await
            .expect("trusted list sees foreign session")
            .len(),
        1,
    );
    assert!(service
        .list_session_tasks_for_human(bear_id, owner_session, viewer)
        .await
        .expect("foreign client session is hidden")
        .is_empty());

    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        viewer,
        bear_id,
    )
    .execute(&pool)
    .await
    .expect("revoke viewer Bear membership");
    for task_id in [own_task, visible.tasks[0].id] {
        assert!(matches!(
            service
                .authorize_task_for_human(bear_id, task_id, viewer)
                .await,
            Err(DenError::NotFound(_)),
        ));
    }
    assert!(service
        .list_session_tasks_for_human(bear_id, viewer_session, viewer)
        .await
        .expect("revoked member gets no task details")
        .is_empty());
}

#[tokio::test]
async fn settled_standalone_task_remains_readable_only_with_creator_and_own_session_provenance() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket settled-task access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "settled-task-owner").await;
    let other = seed_user_and_bear(&pool, "settled-task-other").await.0;
    let admin = seed_user_and_bear(&pool, "settled-task-admin").await.0;
    for (user_id, role) in [(other, "member"), (admin, "admin")] {
        grant_membership(&pool, user_id, bear_id, role).await;
    }
    let service = PgDocketService::from_pool(&pool);
    let owner_session = seed_client_session(&pool, owner, bear_id).await;
    let other_session = seed_client_session(&pool, other, bear_id).await;
    let task_id = standalone_task(&service, bear_id, owner_session, owner, "finished task").await;

    let settled = service
        .settle_session_task(DocketSessionTaskSettlement {
            bear_id,
            task_id,
            session_anchor_id: owner_session,
            status: DocketTaskStatus::Done,
            outcome_disposition: None,
            result_summary: Some("Finished the standalone task".to_string()),
            result_refs: None,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(owner),
            actor_agent_id: None,
        })
        .await
        .expect("settle and release standalone task");
    assert!(settled.task.settled_by_entry_id.is_some());
    service
        .authorize_task_for_human(bear_id, task_id, owner)
        .await
        .expect("creator can still read settled task through own released attachment");
    for user_id in [other, admin] {
        assert!(matches!(
            service
                .authorize_task_for_human(bear_id, task_id, user_id)
                .await,
            Err(DenError::NotFound(_)),
        ));
    }
    assert!(service
        .list_session_tasks_for_human(bear_id, owner_session, owner)
        .await
        .expect("settled task is not actionable")
        .is_empty());
    assert!(service
        .list_session_tasks(bear_id, owner_session)
        .await
        .expect("released task is absent from trusted active list")
        .is_empty());
    assert!(matches!(
        service
            .attach_task_to_session(bear_id, task_id, owner_session)
            .await,
        Err(DenError::ValidationError(_)),
    ));
    assert!(service
        .list_session_tasks_for_human(bear_id, owner_session, owner)
        .await
        .expect("failed reattachment cannot expose task for selection")
        .is_empty());

    sqlx::query!(
        "UPDATE bear_session_task_attachments SET session_id = $2 WHERE task_id = $1",
        task_id,
        other_session,
    )
    .execute(&pool)
    .await
    .expect("simulate historical foreign session binding");
    for user_id in [owner, other, admin] {
        assert!(matches!(
            service
                .authorize_task_for_human(bear_id, task_id, user_id)
                .await,
            Err(DenError::NotFound(_)),
        ));
    }
    sqlx::query!(
        "UPDATE bear_session_task_attachments SET session_id = $2 WHERE task_id = $1",
        task_id,
        owner_session,
    )
    .execute(&pool)
    .await
    .expect("restore owner session provenance");
    sqlx::query!(
        "UPDATE bear_tasks SET created_by_user_id = NULL WHERE id = $1",
        task_id,
    )
    .execute(&pool)
    .await
    .expect("simulate missing task creator");
    assert!(matches!(
        service
            .authorize_task_for_human(bear_id, task_id, owner)
            .await,
        Err(DenError::NotFound(_)),
    ));
}

#[tokio::test]
async fn session_visibility_is_filtered_before_limit() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket visibility limit test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "task-limit-owner").await;
    let viewer = seed_user_and_bear(&pool, "task-limit-viewer").await.0;

    grant_membership(&pool, viewer, bear_id, "member").await;
    let service = PgDocketService::from_pool(&pool);
    let private = service
        .create_job(
            two_task_job(owner, bear_id),
            crate::DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .expect("create SameUser job");
    let mut visible_create = two_task_job(owner, bear_id);
    visible_create.visibility = TaskListVisibility::BearVisible;
    let visible = service
        .create_job(
            visible_create,
            crate::DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .expect("create BearVisible job");
    let session_id = seed_client_session(&pool, viewer, bear_id).await;
    // More than the viewer limit of private rows sort ahead of the public rows.
    sqlx::query!(
        r#"
        INSERT INTO bear_tasks (
            bear_id, job_id, sibling_order, title, body, completion_criteria,
            created_by_role, created_by_user_id
        )
        SELECT $1, $2, n - 1000, 'private filler ' || n, 'private body',
               '["done"]'::jsonb, 'pair', $3
        FROM generate_series(1, 510) AS n
        "#,
        bear_id,
        private.job.id,
        owner,
    )
    .execute(&pool)
    .await
    .expect("seed private tasks before public tasks");
    service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            owner,
            checkout(private.job.id, session_id),
        )
        .await
        .expect("historically attach private job tasks");
    let projection = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            viewer,
            checkout(visible.job.id, session_id),
        )
        .await
        .expect("checkout visible job")
        .expect("visible projection survives private rows ahead of limit");
    assert_eq!(projection.items.len(), visible.tasks.len());
    assert!(projection.items.iter().all(|item| visible
        .tasks
        .iter()
        .any(|task| item.id == task.id.to_string())));
    let tasks = service
        .list_session_tasks_for_human(bear_id, session_id, viewer)
        .await
        .expect("list only public tasks before limit");
    assert_eq!(tasks.len(), visible.tasks.len());
    assert!(tasks
        .iter()
        .all(|task| task.task.job_id == Some(visible.job.id)));
}
