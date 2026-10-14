use super::*;
use den_cabinet::{HistoryRequest, ReadRequest, ReviewState, UpdateItemRequest};

async fn publication_audit(pool: &PgPool, reference: &CabinetItemRef) -> serde_json::Value {
    sqlx::query_scalar!(
        r#"SELECT jsonb_build_object('page', to_jsonb(i), 'versions',
             (SELECT jsonb_agg(to_jsonb(v) ORDER BY v.revision) FROM cabinet_item_versions v WHERE v.item_id=i.id)) AS "audit!"
           FROM cabinet_items i WHERE i.cabinet_ref=$1"#,
        reference.as_str(),
    ).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn posts_do_not_change_revisions_or_publish_pending_bear_proposals(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let original = publication_audit(&pool, &reference).await;
    post(&pool, &owner, &reference, "Discussion, not publication")
        .await
        .unwrap();
    assert_eq!(publication_audit(&pool, &reference).await, original);

    let bear = bear(&pool, true).await;
    pages::configure(
        &pool,
        &owner,
        &reference,
        CabinetPolicy {
            review_required: true,
            ..Default::default()
        },
        &[],
        &[],
        &[],
    )
    .await
    .unwrap();
    let current = cabinet::read(
        &pool,
        ReadRequest {
            scope: owner.clone(),
            cabinet_ref: reference.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    let proposal = cabinet::update_item(
        &pool,
        UpdateItemRequest {
            scope: bear,
            cabinet_ref: reference.clone(),
            content: "Pending Bear proposal".into(),
            base_version: current.item.current_version.unwrap(),
            title: Some("Pending title".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(proposal.version.review(), ReviewState::Pending);
    let pending = publication_audit(&pool, &reference).await;
    post(&pool, &owner, &reference, "Please revise the proposal")
        .await
        .unwrap();
    assert_eq!(publication_audit(&pool, &reference).await, pending);
}

#[sqlx::test(migrations = "../../migrations")]
async fn revision_history_and_discussion_are_independent_facade_views(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let history_request = HistoryRequest {
        scope: owner.clone(),
        cabinet_ref: reference.clone(),
    };
    let before = cabinet::history(&pool, history_request.clone())
        .await
        .unwrap();
    let mut provenance = owner.clone();
    provenance.conversation_id = Some(den_core::ids::ConversationId("private-origin".into()));
    provenance.run_id = Some("origin-run".into());
    provenance.task_id = Some("origin-task".into());
    let entry = post(&pool, &provenance, &reference, "Discuss the next revision")
        .await
        .unwrap();
    assert_eq!(entry.authored_by, provenance);
    assert_eq!(
        cabinet::history(&pool, history_request.clone())
            .await
            .unwrap(),
        before
    );
    let current = cabinet::read(
        &pool,
        ReadRequest {
            scope: owner.clone(),
            cabinet_ref: reference.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    cabinet::update_item(
        &pool,
        UpdateItemRequest {
            scope: owner.clone(),
            cabinet_ref: reference.clone(),
            content: "A new published revision".into(),
            base_version: current.item.current_version.unwrap(),
            title: None,
        },
    )
    .await
    .unwrap();
    let after = cabinet::history(&pool, history_request).await.unwrap();
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after[1..], before);
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

fn database_error(error: sqlx::Error, code: &str, constraint: Option<&str>) {
    let sqlx::Error::Database(cause) = error else {
        panic!("expected database enforcement")
    };
    assert_eq!(cause.code().as_deref(), Some(code));
    if let Some(constraint) = constraint {
        assert_eq!(cause.constraint(), Some(constraint));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn entries_and_author_provenance_are_immutable_with_restrictive_retention(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let other = user(&pool).await;
    let entry = post(&pool, &owner, &reference, "Original audit")
        .await
        .unwrap();
    database_error(sqlx::query!(
        "UPDATE cabinet_discussion_entries SET content='rewritten', authored_by=$2, authored_by_user_id=$3, authored_at=now() WHERE discussion_ref=$1",
        entry.discussion_ref.as_str(), Json(&other) as _, user_id(&other),
    ).execute(&pool).await.unwrap_err(), "23514", None);
    database_error(
        sqlx::query!(
            "DELETE FROM cabinet_discussion_entries WHERE discussion_ref=$1",
            entry.discussion_ref.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "23514",
        None,
    );
    database_error(
        sqlx::query!("DELETE FROM users WHERE id=$1", user_id(&owner))
            .execute(&pool)
            .await
            .unwrap_err(),
        "23503",
        Some("cabinet_discussion_entries_authored_by_user_id_fkey"),
    );
    database_error(
        sqlx::query!(
            "DELETE FROM cabinet_items WHERE cabinet_ref=$1",
            reference.as_str()
        )
        .execute(&pool)
        .await
        .unwrap_err(),
        "23503",
        Some("cabinet_discussion_entries_item_id_fkey"),
    );
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn storage_rejects_misattributed_or_bear_authors(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let other = user(&pool).await;
    let bear = bear(&pool, true).await;
    for scope in [&other, &bear] {
        let discussion_ref = CabinetDiscussionRef::mint();
        database_error(sqlx::query!(
            "INSERT INTO cabinet_discussion_entries(discussion_ref,item_id,content,authored_by,authored_by_user_id)
             SELECT $1,id,'Misattributed',$3,$4 FROM cabinet_items WHERE cabinet_ref=$2",
            discussion_ref.as_str(), reference.as_str(), Json(scope) as _, user_id(&owner),
        ).execute(&pool).await.unwrap_err(), "23514", Some("cabinet_discussion_human_author"));
    }
    assert_eq!(entry_count(&pool).await, 0);
}
