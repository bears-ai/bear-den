//! Durable page discussion. Posts never create or publish Cabinet revisions.

use super::{actor_denormalized, db_error, parse_lifecycle, violation, write_fence};
use den_cabinet::{
    Actor, ActorScope, CabinetDiscussionRef, CabinetError, CabinetItemRef, ContractViolation,
    Lifecycle,
};
pub use den_cabinet::{DiscussionEntry, MAX_DISCUSSION_CHARS};
use sqlx::{types::Json, PgPool};

/// The newest entries returned by [`list`], ordered oldest-first within that window.
pub const DISCUSSION_LIST_LIMIT: i64 = 100;

/// Read discussion with the page's current inherited read authority.
/// Archived pages remain readable; deleted and unreadable refs are indistinguishable.
/// Returns only the newest [`DISCUSSION_LIST_LIMIT`] entries, oldest-first within
/// that window, breaking timestamp ties by discussion ref. No pagination or total
/// count is provided; this is not the complete discussion archive.
pub async fn list(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
) -> Result<Vec<DiscussionEntry>, CabinetError> {
    let (human, bear) = actor_denormalized(scope);
    // Authorization and content share one statement snapshot. The outer page row
    // distinguishes an authorized empty discussion from an unreadable/missing page.
    let rows = sqlx::query!(
        r#"SELECT i.cabinet_ref, d.discussion_ref AS "discussion_ref?",
                  d.content AS "content?", d.authored_by AS "authored_by?: Json<ActorScope>",
                  d.authored_at AS "authored_at?"
           FROM cabinet_items i
           LEFT JOIN LATERAL (
               SELECT discussion_ref, content, authored_by, authored_at
               FROM cabinet_discussion_entries WHERE item_id = i.id
               ORDER BY authored_at DESC, discussion_ref DESC LIMIT $4
           ) d ON true
           WHERE i.cabinet_ref = $1 AND cabinet_can_access(i.id, $2, $3, false)
           ORDER BY d.authored_at, d.discussion_ref"#,
        reference.as_str(),
        human,
        bear,
        DISCUSSION_LIST_LIMIT,
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    if rows.is_empty() {
        return Err(CabinetError::NotFound);
    }
    rows.into_iter()
        .filter_map(|row| {
            row.discussion_ref.map(|discussion_ref| {
                let missing =
                    || CabinetError::Storage("incomplete discussion entry in storage".into());
                Ok(DiscussionEntry {
                    discussion_ref: CabinetDiscussionRef::parse(&discussion_ref)
                        .map_err(violation)?,
                    cabinet_ref: CabinetItemRef::parse(&row.cabinet_ref).map_err(violation)?,
                    content: row.content.ok_or_else(missing)?,
                    authored_by: row.authored_by.ok_or_else(missing)?.0,
                    authored_at: row.authored_at.ok_or_else(missing)?,
                })
            })
        })
        .collect()
}

/// Append a human entry to an active page under its current write authority.
/// Whitespace is trimmed; content must contain 1..=16,000 Unicode scalar values.
/// Bear posts are not supported in this release, even when the Bear can edit the page.
pub async fn post(
    pool: &PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    content: &str,
) -> Result<DiscussionEntry, CabinetError> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    write_fence(&mut tx).await?;
    let (human, bear) = actor_denormalized(scope);
    // Recheck on the fenced transaction, after any queued policy/topology/lifecycle
    // mutation has committed. Callers' earlier page metadata is not authoritative.
    let page = sqlx::query!(
        r#"SELECT id, lifecycle, cabinet_can_access(id, $2, $3, false) AS "can_read!",
                  cabinet_can_access(id, $2, $3, true) AS "can_write!"
           FROM cabinet_items WHERE cabinet_ref = $1"#,
        reference.as_str(),
        human,
        bear,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_error)?
    .ok_or(CabinetError::NotFound)?;
    if !page.can_read {
        return Err(CabinetError::NotFound);
    }
    let Actor::User { user_id } = scope.actor else {
        return Err(CabinetError::NotAuthorized);
    };
    if !page.can_write {
        return Err(CabinetError::NotAuthorized);
    }
    if parse_lifecycle(&page.lifecycle)? != Lifecycle::Active {
        return Err(CabinetError::Policy(
            "discussion posts require an active Cabinet page".into(),
        ));
    }
    let content = content.trim();
    if content.is_empty() {
        return Err(violation(ContractViolation::EmptyField {
            record: "discussion entry",
            field: "content",
        }));
    }
    if content.chars().take(MAX_DISCUSSION_CHARS + 1).count() > MAX_DISCUSSION_CHARS {
        return Err(CabinetError::Policy(format!(
            "discussion content is limited to {MAX_DISCUSSION_CHARS} Unicode scalar values"
        )));
    }
    if content.contains('\0') {
        return Err(CabinetError::Policy(
            "discussion content must not contain NUL characters".into(),
        ));
    }
    let discussion_ref = CabinetDiscussionRef::mint();
    let authored_at = sqlx::query_scalar!(
        r#"INSERT INTO cabinet_discussion_entries
           (discussion_ref, item_id, content, authored_by, authored_by_user_id)
           VALUES ($1, $2, $3, $4, $5) RETURNING authored_at"#,
        discussion_ref.as_str(),
        page.id,
        content,
        Json(scope) as _,
        user_id.0,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(DiscussionEntry {
        discussion_ref,
        cabinet_ref: reference.clone(),
        content: content.to_owned(),
        authored_by: scope.clone(),
        authored_at,
    })
}

#[cfg(test)]
mod tests;
