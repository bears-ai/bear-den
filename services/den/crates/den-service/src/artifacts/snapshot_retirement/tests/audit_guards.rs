use super::*;

async fn audit(pool: &PgPool, f: &Fixture) -> serde_json::Value {
    sqlx::query_scalar!(r#"SELECT jsonb_build_object('artifact',to_jsonb(a),'payload',p.payload,
        'citation',to_jsonb(l)) AS "audit!" FROM artifacts a JOIN artifact_json_payloads p ON p.artifact_id=a.id
        JOIN artifact_links l ON l.artifact_id=a.id AND l.target_kind='cabinet_snapshot'
        WHERE a.artifact_ref=$1"#,f.reference.as_str()).fetch_one(pool).await.unwrap()
}
fn guard(error: sqlx::Error, constraint: &str) {
    let sqlx::Error::Database(cause) = error else {
        panic!("expected a database constraint refusal");
    };
    assert_eq!(cause.code().as_deref(), Some("23514"));
    assert_eq!(cause.constraint(), Some(constraint));
}

#[sqlx::test(migrations = "../../migrations")]
async fn finalized_snapshot_cannot_reset_to_pending_then_rewrite_payload_or_registry(pool: PgPool) {
    let f = fixture(&pool).await;
    let original = audit(&pool, &f).await;
    guard(
        sqlx::query!(
            "UPDATE artifacts SET lifecycle='pending' WHERE artifact_ref=$1",
            f.reference.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "snapshot_registry_immutable",
    );
    guard(sqlx::query!("UPDATE artifact_json_payloads SET payload='{}'::jsonb WHERE artifact_id=(SELECT id FROM artifacts WHERE artifact_ref=$1)",f.reference.as_str()).execute(&pool).await.unwrap_err(),"snapshot_payload_immutable");
    guard(sqlx::query!("UPDATE artifacts SET title='Rewritten audit',content_sha256=repeat('b',64) WHERE artifact_ref=$1",f.reference.as_str()).execute(&pool).await.unwrap_err(),"snapshot_registry_immutable");
    guard(
        sqlx::query!(
            "UPDATE artifacts SET deleted_at=NOW() WHERE artifact_ref=$1",
            f.reference.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "snapshot_registry_immutable",
    );
    // A genuinely unchanged finalized write is not a lifecycle transition.
    sqlx::query!(
        "UPDATE artifacts SET lifecycle='finalized' WHERE artifact_ref=$1",
        f.reference.as_str()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(original, audit(&pool, &f).await);
}
#[sqlx::test(migrations = "../../migrations")]
async fn validated_retirement_stays_one_way_and_keeps_deleted_audit_immutable(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    let original = audit(&pool, &f).await;
    for state in ["pending", "finalized"] {
        guard(
            sqlx::query!(
                "UPDATE artifacts SET lifecycle=$2 WHERE artifact_ref=$1",
                f.reference.as_str(),
                state
            )
            .execute(&pool)
            .await
            .unwrap_err(),
            "snapshot_registry_immutable",
        );
    }
    guard(
        sqlx::query!(
            "UPDATE artifacts SET title='Changed retired title' WHERE artifact_ref=$1",
            f.reference.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "snapshot_registry_immutable",
    );
    guard(
        sqlx::query!(
            "UPDATE artifacts SET deleted_at=deleted_at+INTERVAL '1 second' WHERE artifact_ref=$1",
            f.reference.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "snapshot_registry_immutable",
    );
    guard(sqlx::query!("UPDATE artifact_json_payloads SET payload='{}'::jsonb WHERE artifact_id=(SELECT id FROM artifacts WHERE artifact_ref=$1)",f.reference.as_str()).execute(&pool).await.unwrap_err(),"snapshot_payload_immutable");
    assert_eq!(original, audit(&pool, &f).await);
}
#[sqlx::test(migrations = "../../migrations")]
async fn prepopulated_retirement_receipt_insert_is_refused_without_changing_original_audit(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    let original = audit(&pool, &f).await;
    guard(sqlx::query!(r#"INSERT INTO artifact_links(artifact_id,target_kind,target_id,role,metadata,created_by_user_id,
        retention_released_at,retention_released_by_user_id,retention_release_reason,retirement_fingerprint)
        SELECT id,'cabinet_snapshot','injected-receipt','citation','{}'::jsonb,$2,NOW(),$2,'Forged retirement',repeat('a',64)
        FROM artifacts WHERE artifact_ref=$1"#,f.reference.as_str(),f.owner.get()).execute(&pool).await.unwrap_err(),"snapshot_receipt_insert");
    assert_eq!(original, audit(&pool, &f).await);
}
#[sqlx::test(migrations = "../../migrations")]
async fn deferred_insert_guard_cannot_commit_released_receipt_on_finalized_artifact(pool: PgPool) {
    let f = fixture(&pool).await;
    let original = audit(&pool, &f).await;
    let mut tx = pool.begin().await.unwrap();
    // Isolated transaction only: bypass the immediate guard to exercise the independent
    // deferred INSERT backstop. The failed COMMIT rolls this DDL back too.
    sqlx::query!("ALTER TABLE artifact_links DISABLE TRIGGER artifact_links_snapshot_citation")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query!(r#"INSERT INTO artifact_links(artifact_id,target_kind,target_id,role,metadata,created_by_user_id,
        retention_released_at,retention_released_by_user_id,retention_release_reason,retirement_fingerprint)
        SELECT id,'cabinet_snapshot','deferred-injected-receipt','citation','{}'::jsonb,$2,NOW(),$2,'Forged retirement',repeat('a',64)
        FROM artifacts WHERE artifact_ref=$1"#,f.reference.as_str(),f.owner.get()).execute(&mut *tx).await.unwrap();
    guard(tx.commit().await.unwrap_err(), "snapshot_retirement_atomic");
    assert_eq!(original, audit(&pool, &f).await);
    let enabled = sqlx::query_scalar!(
        r#"SELECT tgenabled::text AS "enabled!" FROM pg_trigger
        WHERE tgrelid='artifact_links'::regclass AND tgname='artifact_links_snapshot_citation'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(enabled, "O", "transactional test bypass must not persist");
}
#[sqlx::test(migrations = "../../migrations")]
async fn deferred_artifact_insert_guard_refuses_deleted_snapshot_without_receipt(pool: PgPool) {
    let f = fixture(&pool).await;
    let reference = format!("artifact_{}", Uuid::new_v4().simple());
    let mut tx = pool.begin().await.unwrap();
    sqlx::query!("INSERT INTO artifacts(artifact_ref,bear_id,created_by_user_id,owner_profile,kind,storage_kind,visibility,lifecycle,deleted_at) VALUES($1,$2,$3,'chat','cabinet_document_snapshot','db_text','same_user','deleted',NOW())",reference,f.bear.as_uuid(),f.owner.get()).execute(&mut *tx).await.unwrap();
    guard(tx.commit().await.unwrap_err(), "snapshot_retirement_atomic");
    assert!(
        sqlx::query_scalar!("SELECT id FROM artifacts WHERE artifact_ref=$1", reference)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .is_none()
    );
}
