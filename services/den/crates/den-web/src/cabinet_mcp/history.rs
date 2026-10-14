use den_cabinet::{
    ActorScope, CabinetError, CabinetItemRef, HistoryRequest, ReadRequest, ReviewState,
    VersionSummary,
};
use den_service::cabinet;
use serde::Serialize;
use sqlx::PgPool;

const LIMIT: usize = 100;

#[derive(Serialize)]
pub(super) struct History {
    versions: Vec<VersionSummary>,
    truncated: bool,
}

fn visible_to(version: &VersionSummary, scope: &ActorScope, can_review: bool) -> bool {
    can_review
        || !matches!(version.review, ReviewState::Pending | ReviewState::Rejected)
        || version.authored_by.actor == scope.actor
}

pub(super) async fn read(
    pool: &PgPool,
    scope: &ActorScope,
    page: &CabinetItemRef,
) -> Result<History, CabinetError> {
    let versions = cabinet::history(
        pool,
        HistoryRequest {
            scope: scope.clone(),
            cabinet_ref: page.clone(),
        },
    )
    .await?;
    let authority = cabinet::pages::metadata(pool, scope, page).await?;
    let mut visible = Vec::new();
    for version in versions
        .into_iter()
        .filter(|version| visible_to(version, scope, authority.can_review))
    {
        // Summary metadata must never bypass the selected revision's authority.
        match cabinet::read(
            pool,
            ReadRequest {
                scope: scope.clone(),
                cabinet_ref: page.clone(),
                version_ref: Some(version.version_ref.clone()),
            },
        )
        .await
        {
            Ok(_) => visible.push(version),
            Err(CabinetError::NotFound | CabinetError::NotAuthorized) => continue,
            Err(error) => return Err(error),
        }
        if visible.len() > LIMIT {
            break;
        }
    }
    let current = cabinet::pages::metadata(pool, scope, page).await?;
    visible.retain(|version| visible_to(version, scope, current.can_review));
    // The lookahead and limit describe only currently visible revisions. Hidden
    // proposals must neither displace published history nor influence this bit.
    let truncated = visible.len() > LIMIT;
    visible.truncate(LIMIT);
    Ok(History {
        versions: visible,
        truncated,
    })
}
