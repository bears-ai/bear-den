use super::*;
use crate::{
    artifacts::{
        self, snapshot_retirement as retirement, ArtifactStorageKind, ArtifactVisibility,
        ReserveArtifactInput,
    },
    bears::db,
    cabinet,
};
use den_cabinet::{ActorScope, CreateItemRequest, ItemKind};
use sqlx::PgPool;
use uuid::Uuid;

async fn bare(pool: &PgPool) -> (UserId, BearId) {
    let user = UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users(username,email) VALUES('deleteowner','deleteowner') RETURNING id"
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    let bear = BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug: "deletion-bear",
                name: "Deletion Bear",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    );
    db::grant_membership(pool, user.get(), bear.as_uuid(), Some("admin"))
        .await
        .unwrap();
    (user, bear)
}
async fn copy(pool: &PgPool, owner: UserId, bear: BearId) -> (artifacts::ArtifactRef, Uuid) {
    let page = cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(owner),
            kind: ItemKind::Document,
            title: "PRIVATE DELETE EVIDENCE".into(),
            content: "Private retained original".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap();
    let job=sqlx::query_scalar!("INSERT INTO bear_jobs(bear_id,created_by_user_id,created_by_role,goal,lifecycle_intent,visibility) VALUES($1,$2,'ui','Private Job','cancelled','same_user') RETURNING id",bear.as_uuid(),owner.get()).fetch_one(pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let reference = cabinet::snapshots::capture_in_tx(
        &mut tx,
        pool,
        &ActorScope::user(owner),
        bear,
        &page.item.cabinet_ref,
        page.version.version_ref(),
    )
    .await
    .unwrap();
    artifacts::attach_docket_artifact_in_tx(
        &mut tx,
        artifacts::AttachDocketArtifactInput {
            artifact_ref: reference.as_str().into(),
            bear_id: bear.as_uuid(),
            target_kind: artifacts::DocketArtifactTargetKind::Job,
            target_id: job,
            role: artifacts::DocketArtifactRole::Source,
            metadata: serde_json::json!({}),
            created_by_user_id: Some(owner.get()),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (reference, job)
}
fn confirmation(owner: UserId, bear: BearId, view: BearDeletionPreview) -> ConfirmBearDeletion {
    ConfirmBearDeletion {
        actor: owner,
        bear_id: bear,
        expected: view.fingerprint,
        confirm_slug: view.slug,
        acknowledged: true,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn internal_compensation_can_delete_clean_bare_bear(pool: PgPool) {
    let (_, bear) = bare(&pool).await;
    db::delete_bear(&pool, bear.as_uuid()).await.unwrap();
    assert!(db::get_bear(&pool, bear.as_uuid()).await.unwrap().is_none());
}
#[sqlx::test(migrations = "../../migrations")]
async fn simple_retired_job_copy_resolves_common_hard_delete_but_not_internal_compensation(
    pool: PgPool,
) {
    let (owner, bear) = bare(&pool).await;
    let (reference, job) = copy(&pool, owner, bear).await;
    let before = preview(&pool, owner, bear).await.unwrap();
    assert!(before.blockers.contains(&BearDeletionBlocker::SavedCopies));
    assert!(!serde_json::to_string(&before)
        .unwrap()
        .contains("PRIVATE DELETE EVIDENCE"));
    assert!(db::delete_bear(&pool, bear.as_uuid()).await.is_err());
    assert!(delete_confirmed(&pool, confirmation(owner, bear, before))
        .await
        .is_err());
    let view = retirement::preview(&pool, owner, &reference).await.unwrap();
    retirement::retire(
        &pool,
        retirement::RetireCabinetSnapshot {
            actor: owner,
            reference: reference.clone(),
            expected: view.fingerprint,
            reason: "Job is settled".into(),
            acknowledged: true,
        },
    )
    .await
    .unwrap();
    assert!(db::delete_bear(&pool, bear.as_uuid()).await.is_err());
    let after = preview(&pool, owner, bear).await.unwrap();
    assert!(after.can_delete, "{:?}", after.blockers);
    delete_confirmed(&pool, confirmation(owner, bear, after))
        .await
        .unwrap();
    assert!(db::get_bear(&pool, bear.as_uuid()).await.unwrap().is_none());
    assert!(
        sqlx::query_scalar!("SELECT id FROM bear_jobs WHERE id=$1", job)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .is_none()
    );
    assert!(sqlx::query_scalar!(
        "SELECT id FROM artifacts WHERE artifact_ref=$1",
        reference.as_str()
    )
    .fetch_optional(&pool)
    .await
    .unwrap()
    .is_none());
}
#[sqlx::test(migrations = "../../migrations")]
async fn stale_inventory_membership_and_missing_confirmation_cannot_delete(pool: PgPool) {
    let (owner, bear) = bare(&pool).await;
    let before = preview(&pool, owner, bear).await.unwrap();
    sqlx::query!(
        "UPDATE bears SET name='Changed after preview' WHERE id=$1",
        bear.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(delete_confirmed(&pool, confirmation(owner, bear, before))
        .await
        .is_err());
    let view = preview(&pool, owner, bear).await.unwrap();
    let mut missing = confirmation(owner, bear, view);
    missing.acknowledged = false;
    assert!(delete_confirmed(&pool, missing).await.is_err());
    assert!(db::get_bear(&pool, bear.as_uuid()).await.unwrap().is_some());
    let peer = UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users(username,email) VALUES('deletepeer','deletepeer') RETURNING id"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
    );
    db::grant_membership(&pool, peer.get(), bear.as_uuid(), Some("member"))
        .await
        .unwrap();
    assert!(matches!(
        preview(&pool, peer, bear).await,
        Err(DenError::Authorization(_))
    ));
}
#[sqlx::test(migrations = "../../migrations")]
async fn shared_job_unknown_refs_and_registered_bytes_are_not_cascade_remediation(pool: PgPool) {
    let (owner, bear) = bare(&pool).await;
    let (reference, job) = copy(&pool, owner, bear).await;
    sqlx::query!(
        "UPDATE bear_jobs SET visibility='bear_visible' WHERE id=$1",
        job
    )
    .execute(&pool)
    .await
    .unwrap();
    let view = retirement::preview(&pool, owner, &reference).await.unwrap();
    retirement::retire(
        &pool,
        retirement::RetireCabinetSnapshot {
            actor: owner,
            reference,
            expected: view.fingerprint,
            reason: "Own private copy no longer needed".into(),
            acknowledged: true,
        },
    )
    .await
    .unwrap();
    let blocked = preview(&pool, owner, bear).await.unwrap();
    assert!(blocked
        .blockers
        .contains(&BearDeletionBlocker::RequiredReferences));
    assert!(delete_confirmed(&pool, confirmation(owner, bear, blocked))
        .await
        .is_err());
    assert!(db::get_bear(&pool, bear.as_uuid()).await.unwrap().is_some());
    let upload = artifacts::reserve_artifact(
        &pool,
        ReserveArtifactInput {
            bear_id: bear.as_uuid(),
            created_by_user_id: Some(owner.get()),
            owner_profile: den_core::RuntimeContextLabel::ChannelConversation,
            kind: "cabinet_file".into(),
            title: Some("PRIVATE FILE".into()),
            summary: None,
            content_type: None,
            storage_kind: ArtifactStorageKind::GarageArtifacts,
            visibility: ArtifactVisibility::SameUser,
            provenance: serde_json::json!({}),
            metadata: serde_json::json!({}),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let blocked = preview(&pool, owner, bear).await.unwrap();
    assert!(blocked
        .blockers
        .contains(&BearDeletionBlocker::ExternalBytes));
    let json = serde_json::to_string(&blocked).unwrap();
    assert!(!json.contains("PRIVATE FILE"));
    assert!(!json.contains(&upload.artifact_ref));
}
#[sqlx::test(migrations = "../../migrations")]
async fn live_turn_is_guarded_for_web_and_internal_callers(pool: PgPool) {
    let (owner, bear) = bare(&pool).await;
    sqlx::query!("INSERT INTO turn_runs(run_id,session_id,bear_id,user_id,state) VALUES('live-delete-turn','live-delete-session',$1,$2,'accepted')",bear.as_uuid(),owner.get()).execute(&pool).await.unwrap();
    let blocked = preview(&pool, owner, bear).await.unwrap();
    assert!(blocked.blockers.contains(&BearDeletionBlocker::LiveWork));
    assert!(delete_confirmed(&pool, confirmation(owner, bear, blocked))
        .await
        .is_err());
    assert!(db::delete_bear(&pool, bear.as_uuid()).await.is_err());
}
