//! Page topology, inherited authorization and human-owned policy decisions.

use super::{actor_denormalized, db_error, violation};
use den_cabinet::{Actor, ActorScope, Authority, CabinetError, CabinetItemRef, CabinetPolicy};
use serde::Serialize;
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Page {
    #[serde(skip)]
    pub(crate) id: Uuid,
    pub parent: Option<CabinetItemRef>,
    pub position: i32,
    pub policy: Option<CabinetPolicy>,
    pub users: Vec<i32>,
    pub bears: Vec<Uuid>,
    pub reviewers: Vec<i32>,
    pub can_write: bool,
    pub can_review: bool,
    pub can_manage: bool,
    pub review_required: bool,
}
#[derive(Debug, Serialize)]
pub struct Child {
    pub reference: CabinetItemRef,
    pub title: String,
    pub position: i32,
}

pub(crate) async fn lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), CabinetError> {
    sqlx::query!("SELECT pg_advisory_xact_lock(791004812)::text AS \"locked!\"")
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(())
}

pub async fn metadata(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
) -> Result<Page, CabinetError> {
    let (human, bear) = actor_denormalized(scope);
    let row=sqlx::query!(r#"SELECT i.id,p.cabinet_ref AS "parent_ref?", i.position,i.page_policy AS "page_policy:Json<CabinetPolicy>",i.user_members,i.bear_members,i.reviewer_user_ids,cabinet_can_access(i.id,$2,$3,false) AS "can_read!",cabinet_can_access(i.id,$2,$3,true) AS "can_write!",cabinet_can_review(i.id,$2) AS "can_review!",(EXISTS(SELECT 1 FROM users WHERE id=$2 AND is_admin) OR EXISTS(SELECT 1 FROM cabinet_ancestors(i.id) a WHERE a.created_by_user_id=$2)) AS "can_manage!", cabinet_review_required(i.id) AS "review_required!" FROM cabinet_items i LEFT JOIN cabinet_items p ON p.id=i.parent_item_id WHERE i.cabinet_ref=$1"#,reference.as_str(),human,bear).fetch_optional(pool).await.map_err(db_error)?.ok_or(CabinetError::NotFound)?;
    if !row.can_read {
        return Err(CabinetError::NotFound);
    }
    Ok(Page {
        id: row.id,
        parent: row
            .parent_ref
            .as_deref()
            .map(CabinetItemRef::parse)
            .transpose()
            .map_err(violation)?,
        position: row.position,
        policy: row.page_policy.map(|p| p.0),
        users: row.user_members,
        bears: row.bear_members,
        reviewers: row.reviewer_user_ids,
        can_write: row.can_write,
        can_review: row.can_review,
        can_manage: row.can_manage,
        review_required: row.review_required,
    })
}

pub(crate) async fn authorize(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    authority: Authority,
) -> Result<Page, CabinetError> {
    let page = metadata(pool, scope, reference).await?;
    match authority {
        Authority::Read => {}
        Authority::Write if page.can_write => {}
        Authority::Review if page.can_review => {}
        _ => return Err(CabinetError::NotAuthorized),
    }
    Ok(page)
}

pub async fn children(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
) -> Result<Vec<Child>, CabinetError> {
    let page = metadata(pool, scope, reference).await?;
    let (human, bear) = actor_denormalized(scope);
    let rows=sqlx::query!("SELECT cabinet_ref,title,position FROM cabinet_items WHERE parent_item_id=$1 AND lifecycle <> 'deleted' AND current_version_id IS NOT NULL AND cabinet_can_access(id,$2,$3,false) ORDER BY position,id LIMIT 100",page.id,human,bear).fetch_all(pool).await.map_err(db_error)?;
    rows.into_iter()
        .map(|r| {
            Ok(Child {
                reference: CabinetItemRef::parse(&r.cabinet_ref).map_err(violation)?,
                title: r.title,
                position: r.position,
            })
        })
        .collect()
}

pub async fn configure(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    policy: CabinetPolicy,
    users: &[i32],
    bears: &[Uuid],
    reviewers: &[i32],
) -> Result<(), CabinetError> {
    let Actor::User { user_id } = scope.actor else {
        return Err(CabinetError::NotAuthorized);
    };
    if users.len() > 100 || bears.len() > 100 || reviewers.len() > 100 {
        return Err(CabinetError::Policy(
            "page membership lists are limited to 100 entries".into(),
        ));
    }
    if (!users.is_empty() || !bears.is_empty()) && !users.contains(&user_id.0) {
        return Err(CabinetError::Policy(
            "the human managing this page must remain a member".into(),
        ));
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    lock(&mut tx).await?;
    let page = authorize(pool, scope, reference, Authority::Read).await?;
    if !page.can_manage {
        return Err(CabinetError::NotAuthorized);
    }
    let user_count = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM users WHERE id=ANY($1)"#,
        users
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let bear_count = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM bears WHERE id=ANY($1)"#,
        bears
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let reviewer_count = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM users WHERE id=ANY($1)"#,
        reviewers
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    if user_count as usize != users.len()
        || bear_count as usize != bears.len()
        || reviewer_count as usize != reviewers.len()
    {
        return Err(CabinetError::Policy(
            "members must name existing, distinct principals".into(),
        ));
    }
    let row=sqlx::query!("UPDATE cabinet_items SET page_policy=$2,user_members=$3,bear_members=$4,reviewer_user_ids=$5,updated_at=now() WHERE id=$1",page.id,Json(policy) as _,users,bears,reviewers).execute(&mut *tx).await.map_err(db_error)?;
    if row.rows_affected() != 1 {
        return Err(CabinetError::NotFound);
    }
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

pub async fn organize(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    parent: Option<&CabinetItemRef>,
    position: i32,
    confirm_audience: bool,
) -> Result<(), CabinetError> {
    if !confirm_audience || position < 0 {
        return Err(CabinetError::Policy(
            "confirm the destination audience and choose a nonnegative position".into(),
        ));
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    lock(&mut tx).await?;
    let page = authorize(pool, scope, reference, Authority::Write).await?;
    let parent_id = if let Some(parent) = parent {
        Some(authorize(pool, scope, parent, Authority::Write).await?.id)
    } else {
        None
    };
    if let Some(parent) = parent_id {
        let rows = sqlx::query!("SELECT id,depth FROM cabinet_ancestors($1)", parent)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_error)?;
        let height=sqlx::query_scalar!(r#"WITH RECURSIVE children AS(SELECT id,1 AS depth FROM cabinet_items WHERE id=$1 UNION ALL SELECT i.id,c.depth+1 FROM cabinet_items i JOIN children c ON i.parent_item_id=c.id WHERE c.depth<33) SELECT COALESCE(max(depth),1) AS "height!" FROM children"#,page.id).fetch_one(&mut *tx).await.map_err(db_error)?;
        if rows.iter().any(|r| r.id == Some(page.id)) || rows.len() + height as usize > 32 {
            return Err(CabinetError::Policy(
                "move would create a cycle or exceed the page depth limit".into(),
            ));
        }
    }
    sqlx::query!(
        "UPDATE cabinet_items SET parent_item_id=$2,position=$3,updated_at=now() WHERE id=$1",
        page.id,
        parent_id,
        position
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PendingReview {
    pub cabinet_ref: CabinetItemRef,
    pub version_ref: den_cabinet::CabinetVersionRef,
    pub title: String,
    pub revision: i32,
}

pub async fn pending_reviews(
    pool: &PgPool,
    scope: &ActorScope,
) -> Result<Vec<PendingReview>, CabinetError> {
    let Actor::User { user_id } = scope.actor else {
        return Ok(Vec::new());
    };
    let rows=sqlx::query!("SELECT i.cabinet_ref,i.title,v.version_ref,v.revision FROM cabinet_item_versions v JOIN cabinet_items i ON i.id=v.item_id WHERE v.review='pending' AND cabinet_can_review(i.id,$1) ORDER BY v.authored_at LIMIT 100",user_id.0).fetch_all(pool).await.map_err(db_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(PendingReview {
                cabinet_ref: CabinetItemRef::parse(&row.cabinet_ref).map_err(violation)?,
                version_ref: den_cabinet::CabinetVersionRef::parse(&row.version_ref)
                    .map_err(violation)?,
                title: row.title,
                revision: row.revision,
            })
        })
        .collect()
}

pub async fn review(
    pool: &PgPool,
    request: den_cabinet::ReviewRequest,
) -> Result<(), CabinetError> {
    let Actor::User { user_id } = request.scope.actor else {
        return Err(CabinetError::NotAuthorized);
    };
    if request.rationale.trim().is_empty() || request.rationale.len() > 4000 {
        return Err(CabinetError::Policy(
            "a bounded review rationale is required".into(),
        ));
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    lock(&mut tx).await?;
    let page = authorize(
        pool,
        &request.scope,
        &request.cabinet_ref,
        Authority::Review,
    )
    .await?;
    let row=sqlx::query!(r#"SELECT v.id,v.review,v.base_version_ref,v.proposed_title,c.version_ref AS "current_ref?" FROM cabinet_item_versions v JOIN cabinet_items i ON i.id=v.item_id LEFT JOIN cabinet_item_versions c ON c.id=i.current_version_id WHERE v.item_id=$1 AND v.version_ref=$2 FOR UPDATE OF v,i"#,page.id,request.version_ref.as_str()).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or(CabinetError::NotFound)?;
    if super::parse_review(&row.review)? != den_cabinet::ReviewState::Pending {
        return Err(CabinetError::Policy(
            "this version has already left review".into(),
        ));
    }
    let approved = request.decision == den_cabinet::ReviewDecision::Approved;
    if approved && row.base_version_ref != row.current_ref {
        if let Some(current) = row.current_ref {
            return Err(CabinetError::Conflict {
                current_version: den_cabinet::CabinetVersionRef::parse(&current)
                    .map_err(violation)?,
            });
        }
        return Err(CabinetError::Policy(
            "review base is no longer published".into(),
        ));
    }
    let decision = if approved { "approved" } else { "rejected" };
    sqlx::query!(
        "UPDATE cabinet_item_versions SET review=$2 WHERE id=$1",
        row.id,
        decision
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    if approved {
        sqlx::query!("UPDATE cabinet_items SET current_version_id=$2,title=COALESCE($3,title),updated_at=now() WHERE id=$1",page.id,row.id,row.proposed_title).execute(&mut *tx).await.map_err(db_error)?;
    }
    sqlx::query!("INSERT INTO cabinet_reviews(item_id,version_id,reviewer_user_id,decision,rationale) VALUES($1,$2,$3,$4,$5)",page.id,row.id,user_id.0,decision,request.rationale.trim()).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

pub async fn configure_named(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    policy: CabinetPolicy,
    people: &[String],
    bears: &[String],
    reviewers: &[String],
) -> Result<(), CabinetError> {
    if !metadata(pool, scope, reference).await?.can_manage {
        return Err(CabinetError::NotAuthorized);
    }
    let users = sqlx::query!(
        "SELECT id,username FROM users WHERE username=ANY($1)",
        people
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let bear_rows = sqlx::query!("SELECT id,slug FROM bears WHERE slug=ANY($1)", bears)
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
    let review_rows = sqlx::query!(
        "SELECT id,username FROM users WHERE username=ANY($1)",
        reviewers
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    if users.len() != people.len()
        || bear_rows.len() != bears.len()
        || review_rows.len() != reviewers.len()
    {
        return Err(CabinetError::Policy(
            "one or more named members are missing or duplicated".into(),
        ));
    }
    configure(
        pool,
        scope,
        reference,
        policy,
        &users.iter().map(|r| r.id).collect::<Vec<_>>(),
        &bear_rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        &review_rows.iter().map(|r| r.id).collect::<Vec<_>>(),
    )
    .await
}

pub async fn member_names(
    pool: &PgPool,
    page: &Page,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), CabinetError> {
    let people = sqlx::query_scalar!(
        "SELECT username FROM users WHERE id=ANY($1) ORDER BY username",
        &page.users
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let bears = sqlx::query_scalar!(
        "SELECT slug FROM bears WHERE id=ANY($1) ORDER BY slug",
        &page.bears
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let reviewers = sqlx::query_scalar!(
        "SELECT username FROM users WHERE id=ANY($1) ORDER BY username",
        &page.reviewers
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    Ok((people, bears, reviewers))
}
