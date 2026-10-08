//! Read-only audience summaries of the same ancestor membership intersections as Cabinet.

use den_cabinet::{ActorScope, CabinetItemRef};
use den_core::ids::{BearId, UserId};
use serde::Serialize;

use super::cabinet_error;
use crate::errors::CustomError;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Audience {
    OpenWiki,
    Restricted {
        people: Vec<String>,
        bears: Vec<String>,
    },
}

#[derive(Default)]
struct MembershipIntersection {
    people: Option<std::collections::HashSet<UserId>>,
    bears: Option<std::collections::HashSet<BearId>>,
}

impl MembershipIntersection {
    fn include(&mut self, people: &[i32], bears: &[uuid::Uuid]) {
        // Both empty is an unrestricted level, not a deny-all list.
        if people.is_empty() && bears.is_empty() {
            return;
        }
        let people: std::collections::HashSet<_> =
            people.iter().copied().map(UserId::new).collect();
        let bears: std::collections::HashSet<_> = bears.iter().copied().map(BearId::new).collect();
        if let Some(current) = &mut self.people {
            current.retain(|id| people.contains(id));
        } else {
            self.people = Some(people);
        }
        if let Some(current) = &mut self.bears {
            current.retain(|id| bears.contains(id));
        } else {
            self.bears = Some(bears);
        }
    }
}

pub(super) async fn for_page(
    pool: &sqlx::PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
) -> Result<Audience, CustomError> {
    // Do not disclose membership of an inaccessible target.
    den_service::cabinet::pages::metadata(pool, scope, reference)
        .await
        .map_err(cabinet_error)?;
    let mut members = MembershipIntersection::default();
    include_ancestry(pool, reference, &mut members).await?;
    summarize(pool, members).await
}

pub(super) async fn after_move(
    pool: &sqlx::PgPool,
    scope: &ActorScope,
    reference: &CabinetItemRef,
    parent: Option<&CabinetItemRef>,
) -> Result<Audience, CustomError> {
    let page = den_service::cabinet::pages::metadata(pool, scope, reference)
        .await
        .map_err(cabinet_error)?;
    let mut members = MembershipIntersection::default();
    members.include(&page.users, &page.bears);
    if let Some(parent) = parent {
        den_service::cabinet::pages::metadata(pool, scope, parent)
            .await
            .map_err(cabinet_error)?;
        include_ancestry(pool, parent, &mut members).await?;
    }
    summarize(pool, members).await
}

async fn include_ancestry(
    pool: &sqlx::PgPool,
    reference: &CabinetItemRef,
    members: &mut MembershipIntersection,
) -> Result<(), CustomError> {
    let rows = sqlx::query!(
        r#"SELECT a.user_members AS "people!", a.bear_members AS "bears!"
        FROM cabinet_items i CROSS JOIN LATERAL cabinet_ancestors(i.id) a
        WHERE i.cabinet_ref = $1"#,
        reference.as_str(),
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    for row in rows {
        members.include(&row.people, &row.bears);
    }
    Ok(())
}

async fn summarize(
    pool: &sqlx::PgPool,
    members: MembershipIntersection,
) -> Result<Audience, CustomError> {
    let Some(people) = members.people else {
        return Ok(Audience::OpenWiki);
    };
    let people: Vec<_> = people.into_iter().map(|id| id.get()).collect();
    let bears: Vec<_> = members
        .bears
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.as_uuid())
        .collect();
    let people = sqlx::query_scalar!(
        "SELECT username FROM users WHERE id = ANY($1) ORDER BY username",
        &people
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    let bears = sqlx::query_scalar!(
        "SELECT slug FROM bears WHERE id = ANY($1) AND cabinet_enabled ORDER BY slug",
        &bears
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    Ok(Audience::Restricted { people, bears })
}

#[cfg(test)]
mod tests;
