use den_core::DenError;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    integration_tests::{seed_user_and_bear, test_pool, two_task_job},
    task_list_projection_from_docket_job, DocketService, PgDocketService, TaskListSourceRef,
    TaskListSyncRequest, TaskListVisibility,
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

#[tokio::test]
async fn human_sync_rejects_forged_same_user_job_before_loading_or_mutating_it() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket sync access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "sync-access-owner").await;
    let (member, other_bear) = seed_user_and_bear(&pool, "sync-access-member").await;
    grant_membership(&pool, owner, bear_id, "member").await;
    grant_membership(&pool, member, bear_id, "member").await;
    let service = PgDocketService::from_pool(&pool);
    let private = service
        .create_job(two_task_job(owner, bear_id))
        .await
        .expect("create private job with current run");
    assert!(private.job.current_run_id.is_some());
    let original = service
        .get_job(bear_id, private.job.id)
        .await
        .expect("read job as trusted test")
        .expect("private job");
    let mut forged = task_list_projection_from_docket_job(&original, None);
    forged.items[0].title = "unauthorized title change".to_string();
    forged.items[0].summary = Some("unauthorized body change".to_string());
    forged.updated_at = time::OffsetDateTime::now_utc();

    for task_list in [
        forged.clone(),
        // The item-level typed job ID must be checked if the list source lacks one.
        crate::TaskListProjection {
            source_ref: TaskListSourceRef::local(vec![]),
            ..forged.clone()
        },
    ] {
        let error = service
            .sync_task_list_for_human(bear_id, member, TaskListSyncRequest { task_list })
            .await
            .expect_err("another member cannot sync a SameUser job");
        assert!(matches!(error, DenError::NotFound(_)));
        assert!(!error.to_string().contains(&original.tasks[0].body));
    }
    let mut cross_bear = forged.clone();
    cross_bear.bear_id = other_bear;
    assert!(matches!(
        service
            .sync_task_list_for_human(
                bear_id,
                owner,
                TaskListSyncRequest {
                    task_list: cross_bear,
                },
            )
            .await,
        Err(DenError::NotFound(_)),
    ));
    let mut missing = forged.clone();
    missing.source_ref.docket_job_id = Some(Uuid::new_v4().to_string());
    assert!(matches!(
        service
            .sync_task_list_for_human(bear_id, owner, TaskListSyncRequest { task_list: missing })
            .await,
        Err(DenError::NotFound(_)),
    ));

    let after = service
        .get_job(bear_id, private.job.id)
        .await
        .expect("read job after denial as trusted test")
        .expect("private job still exists");
    assert_eq!(after.tasks.len(), original.tasks.len());
    assert_eq!(after.tasks[0].title, original.tasks[0].title);
    assert_eq!(after.tasks[0].body, original.tasks[0].body);
    assert_eq!(after.tasks[0].updated_at, original.tasks[0].updated_at);
    assert_eq!(
        after
            .task_states
            .iter()
            .map(|state| (state.task_id, &state.status, state.updated_at))
            .collect::<Vec<_>>(),
        original
            .task_states
            .iter()
            .map(|state| (state.task_id, &state.status, state.updated_at))
            .collect::<Vec<_>>(),
    );

    // Rendered refs must never substitute for a typed job ID.
    let mut local = forged;
    local.source_ref = TaskListSourceRef::local(vec![format!("docket_job:{}", private.job.id)]);
    for item in &mut local.items {
        item.source_ref.docket_job_id = None;
    }
    let outcome = service
        .sync_task_list_for_human(bear_id, member, TaskListSyncRequest { task_list: local })
        .await
        .expect("non-Docket projection needs review, not a job read");
    assert!(outcome.review_required);
    assert!(!outcome.applied);
    assert!(outcome.conflicts.is_empty());
}

#[tokio::test]
async fn human_sync_allows_owner_admin_and_bear_visible_member() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket sync access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "sync-allow-owner").await;
    let member = seed_user_and_bear(&pool, "sync-allow-member").await.0;
    let admin = seed_user_and_bear(&pool, "sync-allow-admin").await.0;
    for (user_id, role) in [(owner, "member"), (member, "member"), (admin, "admin")] {
        grant_membership(&pool, user_id, bear_id, role).await;
    }
    let service = PgDocketService::from_pool(&pool);
    let owner_job = service
        .create_job(two_task_job(owner, bear_id))
        .await
        .expect("create owner's SameUser job");
    let admin_job = service
        .create_job(two_task_job(owner, bear_id))
        .await
        .expect("create admin-readable SameUser job");
    let mut public_create = two_task_job(owner, bear_id);
    public_create.visibility = TaskListVisibility::BearVisible;
    let public_job = service
        .create_job(public_create)
        .await
        .expect("create BearVisible job");
    for (viewer, job) in [
        (owner, &owner_job),
        (admin, &admin_job),
        (member, &public_job),
    ] {
        let title = format!("synced by {viewer}");
        let mut task_list = task_list_projection_from_docket_job(job, None);
        task_list.items[0].title = title.clone();
        task_list.updated_at = time::OffsetDateTime::now_utc();
        let outcome = service
            .sync_task_list_for_human(bear_id, viewer, TaskListSyncRequest { task_list })
            .await
            .expect("authorized human sync");
        assert!(outcome.applied);
        assert!(!outcome.review_required);
        let refreshed = service
            .get_job(bear_id, job.job.id)
            .await
            .expect("read synced job")
            .expect("synced job exists");
        assert_eq!(refreshed.tasks[0].title, title);
    }
}
