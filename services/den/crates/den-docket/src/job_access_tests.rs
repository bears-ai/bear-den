use den_core::{BearProfile, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    integration_tests::{seed_client_session, seed_user_and_bear, test_pool, two_task_job},
    DocketService, PgDocketService, TaskListCheckoutRequest, TaskListCheckoutSource,
    TaskListVisibility,
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

fn checkout(job_id: Uuid, session_anchor_id: Option<Uuid>) -> TaskListCheckoutRequest {
    TaskListCheckoutRequest {
        source: TaskListCheckoutSource::DocketJob {
            job_id,
            parent_task_id: None,
        },
        session_anchor_id,
    }
}

async fn attachment_count(pool: &PgPool, task_id: Uuid) -> i64 {
    sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_session_task_attachments WHERE task_id = $1",
        task_id,
    )
    .fetch_one(pool)
    .await
    .expect("count task attachments")
}

#[tokio::test]
async fn checkout_rechecks_exact_job_and_membership_before_projection_or_attachment() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "checkout-owner").await;
    let member = seed_user_and_bear(&pool, "checkout-member").await.0;
    let admin = seed_user_and_bear(&pool, "checkout-admin").await.0;
    let nonmember = seed_user_and_bear(&pool, "checkout-nonmember").await.0;
    for (user_id, role) in [(member, "member"), (admin, "admin")] {
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
    let session_id = seed_client_session(&pool, member, bear_id).await;
    let private_task_id = private.tasks[0].id;

    for session_anchor_id in [None, Some(session_id)] {
        let error = service
            .checkout_task_list(
                bear_id,
                BearProfile::Pair,
                member,
                checkout(private.job.id, session_anchor_id),
            )
            .await
            .expect_err("another member must not project or attach SameUser tasks");
        assert!(matches!(error, DenError::NotFound(_)));
    }
    assert_eq!(attachment_count(&pool, private_task_id).await, 0);
    assert!(service
        .list_session_tasks(bear_id, session_id)
        .await
        .expect("read session tasks")
        .is_empty());

    let owner_projection = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            owner,
            checkout(private.job.id, None),
        )
        .await
        .expect("owner can read SameUser job")
        .expect("owner projection");
    assert_eq!(owner_projection.id, private.job.id);
    let admin_projection = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            admin,
            checkout(private.job.id, None),
        )
        .await
        .expect("Bear admin can read SameUser job")
        .expect("admin projection");
    assert_eq!(admin_projection.id, private.job.id);

    let visible_projection = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            member,
            checkout(visible.job.id, Some(session_id)),
        )
        .await
        .expect("Bear member can attach BearVisible job")
        .expect("visible projection");
    assert!(visible_projection
        .items
        .iter()
        .any(|item| item.id == visible.tasks[0].id.to_string()));
    let attached_task_ids = service
        .list_session_tasks(bear_id, session_id)
        .await
        .expect("read attached visible tasks")
        .iter()
        .map(|task| task.task.id)
        .collect::<Vec<_>>();
    assert!(attached_task_ids.contains(&visible.tasks[0].id));

    for session_anchor_id in [None, Some(session_id)] {
        let error = service
            .checkout_task_list(
                bear_id,
                BearProfile::Pair,
                member,
                checkout(private.job.id, session_anchor_id),
            )
            .await
            .expect_err("private job must stay hidden even when session has other tasks");
        assert!(matches!(error, DenError::NotFound(_)));
    }
    assert_eq!(attachment_count(&pool, private_task_id).await, 0);
    assert_eq!(
        service
            .list_session_tasks(bear_id, session_id)
            .await
            .expect("read session after denied checkout")
            .iter()
            .map(|task| task.task.id)
            .collect::<Vec<_>>(),
        attached_task_ids,
    );

    for job_id in [private.job.id, visible.job.id] {
        let error = service
            .authorize_job_for_human(bear_id, job_id, nonmember)
            .await
            .expect_err("even public Bear jobs require current membership");
        assert!(matches!(error, DenError::NotFound(_)));
    }
    for job_id in [Uuid::new_v4(), private.job.id] {
        let scope = if job_id == private.job.id {
            Uuid::new_v4()
        } else {
            bear_id
        };
        let error = service
            .authorize_job_for_human(scope, job_id, admin)
            .await
            .expect_err("missing or cross-Bear job must not be visible");
        assert!(matches!(error, DenError::NotFound(_)));
    }
}

#[tokio::test]
async fn revoked_membership_denies_owner_and_admin_but_local_projection_is_unchanged() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Docket access test; database unavailable");
        return;
    };
    let (owner, bear_id) = seed_user_and_bear(&pool, "checkout-revoked").await;
    let admin = seed_user_and_bear(&pool, "checkout-revoked-admin").await.0;

    grant_membership(&pool, admin, bear_id, "admin").await;
    let service = PgDocketService::from_pool(&pool);
    let job = service
        .create_job(
            two_task_job(owner, bear_id),
            crate::DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .expect("create job");
    let session_id = seed_client_session(&pool, owner, bear_id).await;
    let projection = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            owner,
            checkout(job.job.id, None),
        )
        .await
        .expect("owner read before revocation")
        .expect("job projection");

    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        owner,
        bear_id,
    )
    .execute(&pool)
    .await
    .expect("revoke owner");
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        admin,
        bear_id,
    )
    .execute(&pool)
    .await
    .expect("revoke admin");
    for user_id in [owner, admin] {
        let error = service
            .checkout_task_list(
                bear_id,
                BearProfile::Pair,
                user_id,
                checkout(job.job.id, Some(session_id)),
            )
            .await
            .expect_err("revoked membership must deny attachment");
        assert!(matches!(error, DenError::NotFound(_)));
    }
    assert_eq!(attachment_count(&pool, job.tasks[0].id).await, 0);

    let local = service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            owner,
            TaskListCheckoutRequest {
                source: TaskListCheckoutSource::LocalProjection(Box::new(projection.clone())),
                session_anchor_id: Some(session_id),
            },
        )
        .await
        .expect("local projections retain their existing behavior")
        .expect("local projection");
    assert_eq!(local.id, projection.id);
    assert_eq!(local.items.len(), projection.items.len());
    assert_eq!(attachment_count(&pool, job.tasks[0].id).await, 0);
}

#[test]
fn unknown_visibility_fails_closed_even_for_owner_or_bear_admin() {
    for (owner, user, role) in [(1, 1, Some("member")), (1, 2, Some("admin"))] {
        assert!(!crate::db::job_visible_to_human(
            "future_visibility",
            owner,
            user,
            role,
        ));
    }
}
