//! Recovery safety, durable retry and owner-only upload-history route checks.
//! Global-queue assertions need fresh databases, not a reused fixture corpus.

use super::uploads::ByteStore;
use super::{get_page, login_cookie, post_form, seed_member, test_app_with_state};
use axum::http::StatusCode;
use den_cabinet::{ActorScope, AttachmentRole, CabinetItemRef, CabinetPolicy};
use den_core::ids::{BearId, UserId};
use den_service::{
    artifacts::{
        self, cleanup as registry, ArtifactAccessLevel, ArtifactReader, ArtifactRef,
        AttachArtifactInput,
    },
    bears::db as bears_db,
    cabinet,
    cabinet::uploads::{self, PendingUpload, UploadAudience, UploadInput},
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const BYTES: &[u8] = b"Recovery fixture bytes";
struct Fixture {
    owner: i32,
    bear: Uuid,
    page: CabinetItemRef,
    scope: ActorScope,
    pending: PendingUpload,
}

async fn fixture(pool: &sqlx::PgPool) -> Fixture {
    let (owner, bear, _, _) = seed_member(pool).await;
    let scope = ActorScope::user(UserId::new(owner));
    let page = super::knowledge::page(pool, owner, "RECOVERY PAGE", "Source").await;
    let pending = uploads::prepare(
        pool,
        &scope,
        &page,
        UploadInput {
            bear_id: BearId::new(bear),
            title: "PRIVATE RECOVERY FILE".into(),
            content_type: "text/plain".into(),
            bytes: BYTES,
            role: AttachmentRole::Data,
            audience: UploadAudience::Private,
        },
    )
    .await
    .unwrap();
    Fixture {
        owner,
        bear,
        page,
        scope,
        pending,
    }
}

async fn deadline(pool: &sqlx::PgPool, pending: &PendingUpload, expires: OffsetDateTime) {
    sqlx::query!(
        "UPDATE artifacts SET expires_at=$2,updated_at=$3 WHERE artifact_ref=$1",
        pending.location().artifact_ref,
        expires,
        OffsetDateTime::now_utc() - Duration::minutes(5)
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn removed(pool: &sqlx::PgPool, pending: &PendingUpload) -> bool {
    sqlx::query_scalar!(
        "SELECT content_removed_at FROM artifacts WHERE artifact_ref=$1",
        pending.location().artifact_ref
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .is_some()
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_migration_defaults_and_terminal_constraint_are_valid(pool: sqlx::PgPool) {
    let item = fixture(&pool).await;
    assert!(
        !removed(&pool, &item.pending).await,
        "new and existing-compatible rows default to no removal acknowledgement"
    );
    let index = sqlx::query!(
        r#"SELECT i.indisvalid AS "valid!",i.indpred IS NOT NULL AS "partial!"
        FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid
        JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname='public' AND c.relname='artifacts_cabinet_cleanup_queue'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(index.valid && index.partial);
    let store = ByteStore::start().await;
    let state = store.app_state(&pool);
    let media = state.media.as_ref().unwrap();
    for published in [false, true] {
        if published {
            media
                .write_artifact(&pool, &item.pending, BYTES)
                .await
                .unwrap();
            uploads::publish(&pool, &item.scope, &item.pending)
                .await
                .unwrap();
        }
        let error = sqlx::query!(
            "UPDATE artifacts SET content_removed_at=$2 WHERE artifact_ref=$1",
            item.pending.location().artifact_ref,
            OffsetDateTime::now_utc()
        )
        .execute(&pool)
        .await
        .expect_err("live content cannot be acknowledged as removed");
        let database = error.as_database_error().unwrap();
        assert_eq!(database.code().as_deref(), Some("23514"));
        assert_eq!(
            database.constraint(),
            Some("artifact_content_removed_terminal")
        );
        assert!(!removed(&pool, &item.pending).await);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_recovers_interrupted_uploads_only_after_the_write_grace(pool: sqlx::PgPool) {
    let item = fixture(&pool).await;
    let store = ByteStore::start().await;
    let state = store.app_state(&pool);
    let media = state.media.as_ref().unwrap();
    media
        .write_artifact(&pool, &item.pending, BYTES)
        .await
        .unwrap();
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    deadline(&pool, &item.pending, now - Duration::minutes(1)).await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    assert!(store.contains(&item.pending.location().storage_key).await);
    assert!(
        media
            .write_artifact(&pool, &item.pending, BYTES)
            .await
            .is_err(),
        "expired receipts cannot mint fresh writes"
    );
    assert!(uploads::publish(&pool, &item.scope, &item.pending)
        .await
        .is_err());
    deadline(
        &pool,
        &item.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        1
    );
    assert!(!store.contains(&item.pending.location().storage_key).await);
    assert!(removed(&pool, &item.pending).await);
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now + Duration::minutes(2), 10)
            .await
            .unwrap(),
        0
    );
    let reference = ArtifactRef::parse(&item.pending.location().artifact_ref).unwrap();
    assert!(artifacts::authorize_for_reader(
        &pool,
        &reference,
        ArtifactReader::Human(UserId::new(item.owner)),
        ArtifactAccessLevel::Content
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_protects_page_and_snapshot_retention_and_published_replay_is_refused(
    pool: sqlx::PgPool,
) {
    let item = fixture(&pool).await;
    let store = ByteStore::start().await;
    let state = store.app_state(&pool);
    let media = state.media.as_ref().unwrap();
    media
        .write_artifact(&pool, &item.pending, BYTES)
        .await
        .unwrap();
    let attached = uploads::publish(&pool, &item.scope, &item.pending)
        .await
        .unwrap();
    assert!(media
        .write_artifact(&pool, &item.pending, BYTES)
        .await
        .is_err());
    let now = OffsetDateTime::now_utc();
    deadline(
        &pool,
        &item.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    assert!(store.contains(&item.pending.location().storage_key).await);
    cabinet::attachments::unlink(&pool, &item.scope, &item.page, &attached)
        .await
        .unwrap();
    let link = artifacts::attach_artifact(
        &pool,
        AttachArtifactInput {
            artifact_ref: item.pending.location().artifact_ref.clone(),
            bear_id: item.bear,
            target_kind: "cabinet_snapshot".into(),
            target_id: item.page.as_str().into(),
            role: "citation".into(),
            metadata: serde_json::json!({}),
            created_by_user_id: Some(item.owner),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    assert!(!removed(&pool, &item.pending).await);
    assert!(store.contains(&item.pending.location().storage_key).await);
    let retained = sqlx::query_scalar!("SELECT id FROM artifact_links WHERE id=$1", link.id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(
        retained,
        Some(link.id),
        "unknown snapshot citations remain protected"
    );

    // A cabinet_file carrying a snapshot citation is not an eligible document snapshot.
    // Exercise successful cleanup on a separate, genuinely unretained upload instead.
    let unretained = fixture(&pool).await;
    media
        .write_artifact(&pool, &unretained.pending, BYTES)
        .await
        .unwrap();
    deadline(
        &pool,
        &unretained.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        1
    );
    assert!(removed(&pool, &unretained.pending).await);
    assert!(
        !store
            .contains(&unretained.pending.location().storage_key)
            .await
    );
    assert!(!removed(&pool, &item.pending).await);
    assert!(store.contains(&item.pending.location().storage_key).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_retries_failed_deletes_and_crash_after_delete_before_ack(pool: sqlx::PgPool) {
    let item = fixture(&pool).await;
    let store = ByteStore::start().await;
    let state = store.app_state(&pool);
    let media = state.media.as_ref().unwrap();
    media
        .write_artifact(&pool, &item.pending, BYTES)
        .await
        .unwrap();
    assert!(uploads::abandon(&pool, &item.pending).await.unwrap());
    let now = OffsetDateTime::now_utc();
    deadline(
        &pool,
        &item.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    store.fail_deletes(true).await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    assert!(!removed(&pool, &item.pending).await);
    assert!(store.contains(&item.pending.location().storage_key).await);
    assert!(
        registry::claim_due(&pool, now, 10)
            .await
            .unwrap()
            .is_empty(),
        "automatic retry is throttled"
    );
    store.missing_bucket_deletes().await;
    assert_eq!(
        crate::cabinet::cleanup::run_batch(
            &state,
            now + registry::RETRY_DELAY + Duration::seconds(1),
            10
        )
        .await
        .unwrap(),
        0
    );
    assert!(
        !removed(&pool, &item.pending).await,
        "a storage 404 is not a successful DELETE acknowledgement"
    );
    assert!(store.contains(&item.pending.location().storage_key).await);
    store.fail_deletes(false).await;
    let reference = ArtifactRef::parse(&item.pending.location().artifact_ref).unwrap();
    let ticket = registry::claim_owned(&pool, now, UserId::new(item.owner), &reference)
        .await
        .unwrap();
    media.remove_retired_artifact(&ticket).await.unwrap();
    assert!(
        !removed(&pool, &item.pending).await,
        "simulate death before DB acknowledgement"
    );
    assert_eq!(
        crate::cabinet::cleanup::run_batch(
            &state,
            now + registry::RETRY_DELAY + Duration::seconds(1),
            10
        )
        .await
        .unwrap(),
        1
    );
    assert!(removed(&pool, &item.pending).await);
    registry::acknowledge(&pool, &ticket, now).await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_history_and_manual_retry_are_owner_and_membership_scoped(pool: sqlx::PgPool) {
    let item = fixture(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, item.bear, Some("admin"))
        .await
        .unwrap();
    let store = ByteStore::start().await;
    let state = store.app_state(&pool);
    state
        .media
        .as_ref()
        .unwrap()
        .write_artifact(&pool, &item.pending, BYTES)
        .await
        .unwrap();
    let app = test_app_with_state(pool.clone(), state).await;
    let cookie = login_cookie(&app, item.owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let reference = &item.pending.location().artifact_ref;
    let url = format!("/cabinet/uploads/{reference}/cleanup");
    let future = post_form(&app, &cookie, &url, String::new()).await;
    assert_eq!(future.status(), StatusCode::NOT_FOUND);
    let (status, html) = get_page(&app, &cookie, "/cabinet/uploads").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("PRIVATE RECOVERY FILE"));
    assert!(html.contains("RECOVERY PAGE"));
    assert!(!html.contains("Retry file removal"));
    let (_, peer_html) = get_page(&app, &peer_cookie, "/cabinet/uploads").await;
    assert!(!peer_html.contains("PRIVATE RECOVERY FILE"));
    assert!(!peer_html.contains(reference));
    let now = OffsetDateTime::now_utc();
    deadline(
        &pool,
        &item.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    let denied = post_form(&app, &peer_cookie, &url, String::new()).await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let (page_owner, _, _, _) = seed_member(&pool).await;
    let foreign_page = super::knowledge::page(
        &pool,
        page_owner,
        "HIDDEN RECOVERY SOURCE",
        "Private source",
    )
    .await;
    cabinet::pages::configure(
        &pool,
        &ActorScope::user(UserId::new(page_owner)),
        &foreign_page,
        CabinetPolicy::default(),
        &[page_owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE artifacts SET provenance=$2 WHERE artifact_ref=$1",
        reference,
        serde_json::json!({"cabinet_ref":foreign_page})
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, html) = get_page(&app, &cookie, "/cabinet/uploads").await;
    assert!(html.contains("Retry file removal"));
    assert!(!html.contains("HIDDEN RECOVERY SOURCE"));
    assert!(!html.contains(foreign_page.as_str()));
    let done = post_form(&app, &cookie, &url, String::new()).await;
    assert_eq!(done.status(), StatusCode::SEE_OTHER);
    let (_, html) = get_page(&app, &cookie, "/cabinet/uploads").await;
    assert!(html.contains("File removed"));
    bears_db::revoke_membership(&pool, item.owner, item.bear)
        .await
        .unwrap();
    let (_, html) = get_page(&app, &cookie, "/cabinet/uploads").await;
    assert!(!html.contains("PRIVATE RECOVERY FILE"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_never_trusts_noncanonical_keys_or_starts_without_storage(pool: sqlx::PgPool) {
    let item = fixture(&pool).await;
    let now = OffsetDateTime::now_utc();
    deadline(
        &pool,
        &item.pending,
        now - registry::WRITE_GRACE - Duration::minutes(1),
    )
    .await;
    let state = super::test_state(pool.clone());
    assert_eq!(
        crate::cabinet::cleanup::run_batch(&state, now, 10)
            .await
            .unwrap(),
        0
    );
    assert!(!removed(&pool, &item.pending).await);
    sqlx::query!(
        "UPDATE artifacts SET storage_key=$2 WHERE artifact_ref=$1",
        item.pending.location().artifact_ref,
        "chat/do-not-delete"
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(registry::claim_due(&pool, now, 10).await.is_err());
    assert!(!removed(&pool, &item.pending).await);
    // No bytes were written for this fixture; verify a repaired pointer can
    // be retried after the noncanonical key was safely refused.
    sqlx::query!(
        "UPDATE artifacts SET storage_key=$2 WHERE artifact_ref=$1",
        item.pending.location().artifact_ref,
        None::<&str>
    )
    .execute(&pool)
    .await
    .unwrap();
    let reference = ArtifactRef::parse(&item.pending.location().artifact_ref).unwrap();
    let ticket = registry::claim_owned(&pool, now, UserId::new(item.owner), &reference)
        .await
        .unwrap();
    registry::acknowledge(&pool, &ticket, now).await.unwrap();
}
