use super::*;
use crate::{
    artifacts::{
        self, ArtifactReader, AttachDocketArtifactInput, DocketArtifactRole,
        DocketArtifactTargetKind,
    },
    bears::db,
    cabinet,
};
use den_cabinet::{ActorScope, CreateItemRequest, ItemKind};
use sqlx::PgPool;
use uuid::Uuid;

mod audit_guards;
mod concurrency;
mod migrations;
mod references;
mod source_races;

struct Fixture {
    owner: UserId,
    peer: UserId,
    bear: BearId,
    reference: ArtifactRef,
    job: Uuid,
    page: den_cabinet::CabinetItemRef,
}
async fn user(pool: &PgPool, label: &str) -> UserId {
    UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users(username,email) VALUES($1,$1) RETURNING id",
            label
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}
async fn fixture(pool: &PgPool) -> Fixture {
    let owner = user(pool, "snapshotowner").await;
    let peer = user(pool, "snapshotpeer").await;
    let bear = BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug: "snapshot-retirement",
                name: "Retirement Bear",
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
    db::grant_membership(pool, owner.get(), bear.as_uuid(), Some("admin"))
        .await
        .unwrap();
    db::grant_membership(pool, peer.get(), bear.as_uuid(), Some("admin"))
        .await
        .unwrap();
    let page = cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(owner),
            kind: ItemKind::Document,
            title: "PRIVATE SAVED TITLE".into(),
            content: "Immutable original content".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap();
    let job=sqlx::query_scalar!("INSERT INTO bear_jobs(bear_id,created_by_user_id,created_by_role,goal,visibility) VALUES($1,$2,'ui','Source evidence','same_user') RETURNING id",bear.as_uuid(),owner.get()).fetch_one(pool).await.unwrap();
    let run=sqlx::query_scalar!("INSERT INTO bear_job_runs(job_id,state,finished_at) VALUES($1,'completed',NOW()) RETURNING id",job).fetch_one(pool).await.unwrap();
    let task=sqlx::query_scalar!("INSERT INTO bear_tasks(bear_id,job_id,title,body,created_by_role) VALUES($1,$2,'Completed source task','Source task','ui') RETURNING id",bear.as_uuid(),job).fetch_one(pool).await.unwrap();
    sqlx::query!("INSERT INTO bear_task_run_state(run_id,task_id,status,finished_at) VALUES($1,$2,'done',NOW())",run,task).execute(pool).await.unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        job,
        run
    )
    .execute(pool)
    .await
    .unwrap();
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
        AttachDocketArtifactInput {
            artifact_ref: reference.as_str().into(),
            bear_id: bear.as_uuid(),
            target_kind: DocketArtifactTargetKind::Job,
            target_id: job,
            role: DocketArtifactRole::Source,
            metadata: serde_json::json!({}),
            created_by_user_id: Some(owner.get()),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    Fixture {
        owner,
        peer,
        bear,
        reference,
        job,
        page: page.item.cabinet_ref,
    }
}
fn request(f: &Fixture, expected: InventoryFingerprint) -> RetireCabinetSnapshot {
    RetireCabinetSnapshot {
        actor: f.owner,
        reference: f.reference.clone(),
        expected,
        reason: "Job settled; source copy no longer needed".into(),
        acknowledged: true,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn retirement_preserves_original_payload_links_page_and_is_idempotent(pool: PgPool) {
    let f = fixture(&pool).await;
    let original =
        artifacts::json_content_for_reader(&pool, &f.reference, ArtifactReader::Human(f.owner))
            .await
            .unwrap();
    let links=sqlx::query_scalar!("SELECT jsonb_agg(to_jsonb(l)-ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'] ORDER BY l.id) FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$1",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    let page_before = cabinet::read(
        &pool,
        den_cabinet::ReadRequest {
            scope: ActorScope::user(f.owner),
            cabinet_ref: f.page.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    assert!(view.can_retire());
    let receipt = retire(&pool, request(&f, view.fingerprint.clone()))
        .await
        .unwrap();
    let replay = retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    assert_eq!(replay.retired_at, receipt.retired_at);
    let raw=sqlx::query_scalar!("SELECT p.payload FROM artifact_json_payloads p JOIN artifacts a ON a.id=p.artifact_id WHERE a.artifact_ref=$1",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(raw, original);
    let after_links=sqlx::query_scalar!("SELECT jsonb_agg(to_jsonb(l)-ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'] ORDER BY l.id) FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$1",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(links, after_links);
    let page_after = cabinet::read(
        &pool,
        den_cabinet::ReadRequest {
            scope: ActorScope::user(f.owner),
            cabinet_ref: f.page,
            version_ref: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        page_before.version.version_ref(),
        page_after.version.version_ref()
    );
    assert!(artifacts::json_content_for_reader(
        &pool,
        &f.reference,
        ArtifactReader::Human(f.owner)
    )
    .await
    .is_err());
    let history = history(&pool, f.owner, None, Some(f.bear), Some(f.job))
        .await
        .unwrap();
    assert!(history.copies[0].retired);
    assert!(!history.copies[0].readable);
    assert!(history.copies[0].receipt.is_some());
    assert!(sqlx::query!("UPDATE artifact_json_payloads SET payload='{}'::jsonb WHERE artifact_id=(SELECT id FROM artifacts WHERE artifact_ref=$1)",f.reference.as_str()).execute(&pool).await.is_err());
    assert!(sqlx::query!(
        "DELETE FROM artifacts WHERE artifact_ref=$1",
        f.reference.as_str()
    )
    .execute(&pool)
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn creator_membership_and_job_authority_are_independent_of_admin_privilege(pool: PgPool) {
    let f = fixture(&pool).await;
    assert!(matches!(
        preview(&pool, f.peer, &f.reference).await,
        Err(DenError::NotFound(_))
    ));
    assert!(history(&pool, f.peer, None, Some(f.bear), None)
        .await
        .unwrap()
        .copies
        .is_empty());
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    let mut forged = request(&f, view.fingerprint.clone());
    forged.actor = f.peer;
    assert!(matches!(
        retire(&pool, forged).await,
        Err(DenError::NotFound(_))
    ));
    db::grant_membership(&pool, f.owner.get(), f.bear.as_uuid(), Some("member"))
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET created_by_user_id=$2 WHERE id=$1",
        f.job,
        f.peer.get()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobAuthority)
    );
    db::revoke_membership(&pool, f.owner.get(), f.bear.as_uuid())
        .await
        .unwrap();
    assert!(matches!(
        preview(&pool, f.owner, &f.reference).await,
        Err(DenError::NotFound(_))
    ));
    assert!(history(&pool, f.owner, None, Some(f.bear), None)
        .await
        .unwrap()
        .copies
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn active_archived_stale_and_missing_consent_cannot_release_retention(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    for state in ["dispatched", "running", "paused", "blocked"] {
        sqlx::query!("UPDATE bear_job_runs SET state=$2 WHERE id=(SELECT current_run_id FROM bear_jobs WHERE id=$1)",f.job,state).execute(&pool).await.unwrap();
        assert_eq!(
            preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
            Some(RetirementBlocker::JobNotSettled)
        );
        assert!(retire(&pool, request(&f, view.fingerprint.clone()))
            .await
            .is_err());
    }
    sqlx::query!("UPDATE bear_job_runs SET state='completed' WHERE id=(SELECT current_run_id FROM bear_jobs WHERE id=$1)",f.job).execute(&pool).await.unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET lifecycle_intent='archived' WHERE id=$1",
        f.job
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobNotSettled)
    );
    sqlx::query!(
        "UPDATE bear_jobs SET lifecycle_intent='cancelled' WHERE id=$1",
        f.job
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(retire(&pool, request(&f, view.fingerprint)).await.is_err());
    let current = preview(&pool, f.owner, &f.reference).await.unwrap();
    let mut invalid = request(&f, current.fingerprint.clone());
    invalid.acknowledged = false;
    assert!(retire(&pool, invalid).await.is_err());
    let mut invalid = request(&f, current.fingerprint.clone());
    invalid.reason = " ".into();
    assert!(retire(&pool, invalid).await.is_err());
    retire(&pool, request(&f, current.fingerprint))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn extra_links_and_notebook_evidence_block_then_closed_refs_cannot_be_added(pool: PgPool) {
    let f = fixture(&pool).await;
    let additional=sqlx::query_scalar!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role) SELECT id,'unknown_target','opaque','evidence' FROM artifacts WHERE artifact_ref=$1 RETURNING id",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::RequiredReferences)
    );
    sqlx::query!("DELETE FROM artifact_links WHERE id=$1", additional)
        .execute(&pool)
        .await
        .unwrap();
    let entry=sqlx::query_scalar!("INSERT INTO bear_docket_entries(job_id,scope,kind,summary,evidence_refs,by_role,by_user_id) VALUES($1,'job_notebook','finding','Required copy',$2,'ui',$3) RETURNING id",f.job,serde_json::json!([f.reference.as_str()]),f.owner.get()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::RequiredReferences)
    );
    sqlx::query!("DELETE FROM bear_docket_entries WHERE id=$1", entry)
        .execute(&pool)
        .await
        .unwrap();
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    assert!(sqlx::query!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role) SELECT id,'unknown_target','late','evidence' FROM artifacts WHERE artifact_ref=$1",f.reference.as_str()).execute(&pool).await.is_err());
    assert!(sqlx::query!("INSERT INTO bear_docket_entries(job_id,scope,kind,summary,evidence_refs,by_role,by_user_id) VALUES($1,'job_notebook','finding','Late copy',$2,'ui',$3)",f.job,serde_json::json!([f.reference.as_str()]),f.owner.get()).execute(&pool).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn completed_job_with_current_docket_or_focused_execution_is_not_settled(pool: PgPool) {
    let f = fixture(&pool).await;
    let run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs(job_id,state) VALUES($1,'paused') RETURNING id",
        f.job
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        f.job,
        run
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobNotSettled)
    );
    sqlx::query!(
        "UPDATE bear_job_runs SET state='completed' WHERE id=$1",
        run
    )
    .execute(&pool)
    .await
    .unwrap();
    let task=sqlx::query_scalar!("INSERT INTO bear_tasks(bear_id,job_id,title,body,created_by_role) VALUES($1,$2,'Focused','Focused source','ui') RETURNING id",f.bear.as_uuid(),f.job).fetch_one(&pool).await.unwrap();
    sqlx::query!("INSERT INTO bear_task_run_state(run_id,task_id,status) SELECT $1,id,'done' FROM bear_tasks WHERE job_id=$2",run,f.job).execute(&pool).await.unwrap();
    sqlx::query!("INSERT INTO docket_execution_attempts(bear_id,task_id,binding_kind,binding_id,host_kind,host_run_id,fence_epoch,authorization_key,state) VALUES($1,$2,'client_session','test-session','pair','test-run',1,$3,'authorized')",f.bear.as_uuid(),task,Uuid::new_v4()).execute(&pool).await.unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobNotSettled)
    );
}

#[test]
fn fingerprint_boundary_and_generic_errors_do_not_expose_evidence() {
    assert!(InventoryFingerprint::try_from("not-a-preview".to_string()).is_err());
    for blocker in [
        RetirementBlocker::RequiredReferences,
        RetirementBlocker::JobAuthority,
        RetirementBlocker::JobNotSettled,
    ] {
        assert!(!blocker.explanation().contains("PRIVATE SAVED TITLE"));
        assert!(!blocker.explanation().contains("artifact_"));
    }
}
