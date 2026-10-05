//! Cabinet page-tree facade: scoped reads, immutable versions and conditional
//! Bear-write review over Den Postgres storage.
//!
//! Contract: `docs/architecture/cabinet-contract.md` (types in `den-cabinet`).
//! Every operation takes an explicit [`ActorScope`]; authorization runs here,
//! not in callers, and nothing outside this module touches the cabinet tables.
//! Direct publication is the default; ancestor policy can require human review.
//! Legacy collection/Mission binding fields remain rejected compatibility inputs;
//! the page-only topology and policy helpers own all new structural state.

use den_cabinet::Authority;
use den_cabinet::{
    validate_source_locator, Actor, ActorScope, CabinetError, CabinetItem, CabinetItemRef,
    CabinetSourceRef, CabinetVersionRef, ContractViolation, CreateItemRequest, HistoryRequest,
    ItemKind, ItemSummary, ItemVersion, Lifecycle, LinkSourceRequest, ReadRequest, ReviewState,
    SearchRequest, SourceKind, SourceLink, SourceRole, UnlinkSourceRequest, UpdateItemRequest,
    VersionSummary,
};
pub mod attachments;
pub mod pages;
pub mod snapshots;

/// Serialize cross-domain edits against Cabinet policy/topology changes.
pub async fn write_fence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), CabinetError> {
    pages::lock(tx).await
}

use serde::Serialize;
use sqlx::types::Json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// A read result: the item, one version (current unless requested otherwise),
/// and the item's source links.
#[derive(Debug, Clone, Serialize)]
pub struct ItemView {
    pub item: CabinetItem,
    pub version: ItemVersion,
    pub sources: Vec<SourceLink>,
    pub proposed_title: Option<String>,
}

const SEARCH_LIMIT: i64 = 50;

fn violation(violation: ContractViolation) -> CabinetError {
    CabinetError::Validation(violation)
}

/// Blanket capability check for the acting identity. Item-level authorization
/// (Mission/collection policy) arrives in Phase 2; Phase 1 items are open to
/// every authenticated Den member and every Cabinet-enabled Bear.
async fn authorize(pool: &PgPool, scope: &ActorScope) -> Result<(), CabinetError> {
    match &scope.actor {
        Actor::User { .. } => Ok(()),
        Actor::Bear { bear_id, .. } => {
            let enabled = sqlx::query_scalar!(
                "SELECT cabinet_enabled FROM bears WHERE id = $1",
                bear_id.as_uuid()
            )
            .fetch_optional(pool)
            .await
            .map_err(db_error)?;
            if enabled == Some(true) {
                Ok(())
            } else {
                Err(CabinetError::NotAuthorized)
            }
        }
    }
}

fn db_error(error: sqlx::Error) -> CabinetError {
    CabinetError::Storage(error.to_string())
}

fn scope_json(scope: &ActorScope) -> Result<serde_json::Value, CabinetError> {
    serde_json::to_value(scope).map_err(|error| CabinetError::Storage(error.to_string()))
}

fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

struct ItemRow {
    id: Uuid,
    cabinet_ref: String,
    kind: String,
    title: String,
    lifecycle: String,
    collection_ref: Option<String>,
    mission_ref: Option<String>,
    current_version_id: Option<Uuid>,
    created_by: Json<ActorScope>,
    created_at: OffsetDateTime,
}

struct VersionRow {
    version_ref: String,
    item_cabinet_ref: String,
    revision: i32,
    content: String,
    content_sha256: String,
    base_version_ref: Option<String>,
    review: String,
    authored_by: Json<ActorScope>,
    authored_at: OffsetDateTime,
}

struct SourceRow {
    source_ref: String,
    item_cabinet_ref: String,
    source_kind: String,
    locator: String,
    role: String,
    created_by: Json<ActorScope>,
    created_at: OffsetDateTime,
}

fn parse_kind(kind: &str) -> Result<ItemKind, CabinetError> {
    match kind {
        "document" => Ok(ItemKind::Document),
        other => Err(CabinetError::Policy(format!(
            "unknown cabinet item kind in storage: {other}"
        ))),
    }
}

fn parse_lifecycle(lifecycle: &str) -> Result<Lifecycle, CabinetError> {
    match lifecycle {
        "active" => Ok(Lifecycle::Active),
        "archived" => Ok(Lifecycle::Archived),
        "deleted" => Ok(Lifecycle::Deleted),
        other => Err(CabinetError::Policy(format!(
            "unknown cabinet lifecycle in storage: {other}"
        ))),
    }
}

fn parse_review(review: &str) -> Result<ReviewState, CabinetError> {
    match review {
        "none" => Ok(ReviewState::None),
        "pending" => Ok(ReviewState::Pending),
        "approved" => Ok(ReviewState::Approved),
        "rejected" => Ok(ReviewState::Rejected),
        other => Err(CabinetError::Policy(format!(
            "unknown cabinet review state in storage: {other}"
        ))),
    }
}

fn parse_source_kind(kind: &str) -> Result<SourceKind, CabinetError> {
    match kind {
        "url" => Ok(SourceKind::Url),
        "offline" => Ok(SourceKind::Offline),
        "artifact" => Ok(SourceKind::Artifact),
        "conversation" => Ok(SourceKind::Conversation),
        "external_record" => Ok(SourceKind::ExternalRecord),
        other => Err(CabinetError::Policy(format!(
            "unknown cabinet source kind in storage: {other}"
        ))),
    }
}

fn parse_source_role(role: &str) -> Result<SourceRole, CabinetError> {
    match role {
        "origin" => Ok(SourceRole::Origin),
        "citation" => Ok(SourceRole::Citation),
        "related" => Ok(SourceRole::Related),
        other => Err(CabinetError::Policy(format!(
            "unknown cabinet source role in storage: {other}"
        ))),
    }
}

fn item_from_row(
    row: &ItemRow,
    current_version: Option<CabinetVersionRef>,
) -> Result<CabinetItem, CabinetError> {
    Ok(CabinetItem {
        cabinet_ref: CabinetItemRef::parse(&row.cabinet_ref).map_err(violation)?,
        kind: parse_kind(&row.kind)?,
        title: row.title.clone(),
        current_version,
        collection_ref: row
            .collection_ref
            .as_deref()
            .map(den_cabinet::CabinetCollectionRef::parse)
            .transpose()
            .map_err(violation)?,
        mission_ref: row
            .mission_ref
            .as_deref()
            .map(den_cabinet::MissionRef::parse)
            .transpose()
            .map_err(violation)?,
        created_by: row.created_by.0.clone(),
        created_at: row.created_at,
        lifecycle: parse_lifecycle(&row.lifecycle)?,
    })
}

fn version_from_row(row: VersionRow) -> Result<ItemVersion, CabinetError> {
    let declared_sha256 = row.content_sha256;
    let base_version = row
        .base_version_ref
        .as_deref()
        .map(CabinetVersionRef::parse)
        .transpose()
        .map_err(violation)?;
    let revision = u32::try_from(row.revision)
        .map_err(|_| CabinetError::Storage("negative revision in storage".to_string()))?;
    let version = ItemVersion::with_review_state(
        CabinetVersionRef::parse(&row.version_ref).map_err(violation)?,
        CabinetItemRef::parse(&row.item_cabinet_ref).map_err(violation)?,
        revision,
        row.content,
        row.authored_by.0,
        row.authored_at,
        base_version,
        parse_review(&row.review)?,
    )
    .map_err(violation)?;
    if version.content_sha256() != declared_sha256 {
        return Err(violation(ContractViolation::ContentHashMismatch {
            declared: declared_sha256,
            computed: version.content_sha256().to_string(),
        }));
    }
    Ok(version)
}

fn source_from_row(row: SourceRow) -> Result<SourceLink, CabinetError> {
    Ok(SourceLink {
        source_ref: CabinetSourceRef::parse(&row.source_ref).map_err(violation)?,
        cabinet_ref: CabinetItemRef::parse(&row.item_cabinet_ref).map_err(violation)?,
        source_kind: parse_source_kind(&row.source_kind)?,
        locator: row.locator,
        role: parse_source_role(&row.role)?,
        created_by: row.created_by.0,
        created_at: row.created_at,
    })
}

fn same_actor_identity(left: &Actor, right: &Actor) -> bool {
    match (left, right) {
        (Actor::User { user_id: left }, Actor::User { user_id: right }) => left == right,
        (Actor::Bear { bear_id: left, .. }, Actor::Bear { bear_id: right, .. }) => left == right,
        _ => false,
    }
}

fn actor_denormalized(scope: &ActorScope) -> (Option<i32>, Option<Uuid>) {
    match &scope.actor {
        Actor::User { user_id } => (Some(user_id.0), None),
        Actor::Bear { bear_id, .. } => (None, Some(bear_id.as_uuid())),
    }
}

async fn load_item_row(
    pool: &PgPool,
    scope: &ActorScope,
    cabinet_ref: &CabinetItemRef,
) -> Result<Option<ItemRow>, CabinetError> {
    pages::authorize(pool, scope, cabinet_ref, Authority::Read).await?;
    sqlx::query_as!(
        ItemRow,
        r#"SELECT id, cabinet_ref, kind, title, lifecycle, collection_ref, mission_ref,
                  current_version_id,
                  created_by AS "created_by: Json<ActorScope>",
                  created_at
           FROM cabinet_items
           WHERE cabinet_ref = $1"#,
        cabinet_ref.as_str()
    )
    .fetch_optional(pool)
    .await
    .map_err(db_error)
}

async fn load_sources(
    pool: &PgPool,
    item_id: Uuid,
    item_ref: &str,
) -> Result<Vec<SourceLink>, CabinetError> {
    let rows = sqlx::query!(
        r#"SELECT source_ref, source_kind, locator, role,
                  created_by AS "created_by: Json<ActorScope>",
                  created_at
           FROM cabinet_source_links
           WHERE item_id = $1
           ORDER BY created_at ASC"#,
        item_id
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    rows.into_iter()
        .map(|row| {
            source_from_row(SourceRow {
                source_ref: row.source_ref,
                item_cabinet_ref: item_ref.to_string(),
                source_kind: row.source_kind,
                locator: row.locator,
                role: row.role,
                created_by: row.created_by,
                created_at: row.created_at,
            })
        })
        .collect()
}

async fn load_version(
    pool: &PgPool,
    item_id: Uuid,
    item_ref: &str,
    version_ref: Option<&CabinetVersionRef>,
    current_version_id: Option<Uuid>,
) -> Result<Option<ItemVersion>, CabinetError> {
    let row = match version_ref {
        Some(version_ref) => sqlx::query!(
            r#"SELECT version_ref, revision, content, content_sha256, base_version_ref, review,
                      authored_by AS "authored_by: Json<ActorScope>",
                      authored_at
               FROM cabinet_item_versions
               WHERE item_id = $1 AND version_ref = $2"#,
            item_id,
            version_ref.as_str()
        )
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .map(|row| VersionRow {
            version_ref: row.version_ref,
            item_cabinet_ref: item_ref.to_string(),
            revision: row.revision,
            content: row.content,
            content_sha256: row.content_sha256,
            base_version_ref: row.base_version_ref,
            review: row.review,
            authored_by: row.authored_by,
            authored_at: row.authored_at,
        }),
        None => {
            let Some(current_version_id) = current_version_id else {
                return Ok(None);
            };
            sqlx::query!(
                r#"SELECT version_ref, revision, content, content_sha256, base_version_ref, review,
                          authored_by AS "authored_by: Json<ActorScope>",
                          authored_at
                   FROM cabinet_item_versions
                   WHERE id = $1"#,
                current_version_id
            )
            .fetch_optional(pool)
            .await
            .map_err(db_error)?
            .map(|row| VersionRow {
                version_ref: row.version_ref,
                item_cabinet_ref: item_ref.to_string(),
                revision: row.revision,
                content: row.content,
                content_sha256: row.content_sha256,
                base_version_ref: row.base_version_ref,
                review: row.review,
                authored_by: row.authored_by,
                authored_at: row.authored_at,
            })
        }
    };
    row.map(version_from_row).transpose()
}

/// `cabinet_search`: metadata/text search over items the actor may read.
pub async fn search(
    pool: &PgPool,
    request: SearchRequest,
) -> Result<Vec<ItemSummary>, CabinetError> {
    search_impl(pool, request, false).await
}

pub async fn search_roots(
    pool: &PgPool,
    request: SearchRequest,
) -> Result<Vec<ItemSummary>, CabinetError> {
    search_impl(pool, request, true).await
}

async fn search_impl(
    pool: &PgPool,
    request: SearchRequest,
    roots_only: bool,
) -> Result<Vec<ItemSummary>, CabinetError> {
    authorize(pool, &request.scope).await?;
    if request.filters.collection_ref.is_some() || request.filters.mission_ref.is_some() {
        return Err(CabinetError::Policy(
            "collection and Mission filters are not available yet (Cabinet Phase 2)".to_string(),
        ));
    }
    let lifecycle = request
        .filters
        .lifecycle
        .map_or("active", Lifecycle::as_str);
    let kind = request.filters.kind.map(ItemKind::as_str);
    let query = request.query.trim();
    let pattern = if query.is_empty() {
        None
    } else {
        Some(format!("%{}%", escape_like(query)))
    };
    let (human_actor, bear_actor) = actor_denormalized(&request.scope);
    let rows = sqlx::query!(
        r#"SELECT i.cabinet_ref, i.kind, i.title, i.lifecycle, i.collection_ref, i.mission_ref,
                  i.updated_at, v.version_ref AS current_version_ref
           FROM cabinet_items i
           JOIN cabinet_item_versions v ON v.id = i.current_version_id
           WHERE i.lifecycle = $1
             AND ($2::text IS NULL OR i.kind = $2)
             AND ($3::text IS NULL OR i.title ILIKE $3 OR v.content ILIKE $3)
             AND cabinet_can_access(i.id, $5, $6, false)
             AND (NOT $7 OR i.parent_item_id IS NULL)
           ORDER BY i.updated_at DESC
           LIMIT $4"#,
        lifecycle,
        kind,
        pattern,
        SEARCH_LIMIT,
        human_actor,
        bear_actor,
        roots_only
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ItemSummary {
                cabinet_ref: CabinetItemRef::parse(&row.cabinet_ref).map_err(violation)?,
                current_version: CabinetVersionRef::parse(&row.current_version_ref)
                    .map_err(violation)?,
                title: row.title,
                kind: parse_kind(&row.kind)?,
                collection_ref: row
                    .collection_ref
                    .as_deref()
                    .map(den_cabinet::CabinetCollectionRef::parse)
                    .transpose()
                    .map_err(violation)?,
                mission_ref: row
                    .mission_ref
                    .as_deref()
                    .map(den_cabinet::MissionRef::parse)
                    .transpose()
                    .map_err(violation)?,
                lifecycle: parse_lifecycle(&row.lifecycle)?,
                updated_at: row.updated_at,
            })
        })
        .collect()
}

/// `cabinet_read`: the item plus the requested version (current by default)
/// and its source links. Tombstoned items read as `NotFound`.
pub async fn read(pool: &PgPool, request: ReadRequest) -> Result<ItemView, CabinetError> {
    authorize(pool, &request.scope).await?;
    let row = load_item_row(pool, &request.scope, &request.cabinet_ref)
        .await?
        .ok_or(CabinetError::NotFound)?;
    if row.lifecycle == "deleted" {
        return Err(CabinetError::NotFound);
    }
    let version = load_version(
        pool,
        row.id,
        &row.cabinet_ref,
        request.version_ref.as_ref(),
        row.current_version_id,
    )
    .await?
    .ok_or(CabinetError::NotFound)?;
    let sources = load_sources(pool, row.id, &row.cabinet_ref).await?;
    if matches!(
        version.review(),
        ReviewState::Pending | ReviewState::Rejected
    ) && !same_actor_identity(&version.authored_by().actor, &request.scope.actor)
    {
        pages::authorize(
            pool,
            &request.scope,
            &request.cabinet_ref,
            Authority::Review,
        )
        .await?;
    }
    let current =
        load_version(pool, row.id, &row.cabinet_ref, None, row.current_version_id).await?;
    let item = item_from_row(&row, current.map(|value| value.version_ref().clone()))?;
    pages::authorize(pool, &request.scope, &request.cabinet_ref, Authority::Read).await?;
    let proposed_title = if version.review() == ReviewState::Pending {
        sqlx::query_scalar!(
            "SELECT proposed_title FROM cabinet_item_versions WHERE item_id=$1 AND version_ref=$2",
            row.id,
            version.version_ref().as_str()
        )
        .fetch_one(pool)
        .await
        .map_err(db_error)?
    } else {
        None
    };
    Ok(ItemView {
        item,
        version,
        sources,
        proposed_title,
    })
}

/// `cabinet_history`: the revision list, newest first.
pub async fn history(
    pool: &PgPool,
    request: HistoryRequest,
) -> Result<Vec<VersionSummary>, CabinetError> {
    authorize(pool, &request.scope).await?;
    let row = load_item_row(pool, &request.scope, &request.cabinet_ref)
        .await?
        .ok_or(CabinetError::NotFound)?;
    if row.lifecycle == "deleted" {
        return Err(CabinetError::NotFound);
    }
    let rows = sqlx::query!(
        r#"SELECT version_ref, revision, content_sha256, review,
                  authored_by AS "authored_by: Json<ActorScope>",
                  authored_at
           FROM cabinet_item_versions
           WHERE item_id = $1
           ORDER BY revision DESC"#,
        row.id
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    rows.into_iter()
        .map(|version| {
            Ok(VersionSummary {
                version_ref: CabinetVersionRef::parse(&version.version_ref).map_err(violation)?,
                revision: u32::try_from(version.revision).map_err(|_| {
                    CabinetError::Storage("negative revision in storage".to_string())
                })?,
                authored_by: version.authored_by.0,
                authored_at: version.authored_at,
                review: parse_review(&version.review)?,
                content_sha256: version.content_sha256,
            })
        })
        .collect()
}

fn reject_phase2_bindings(
    collection_ref: Option<&den_cabinet::CabinetCollectionRef>,
    mission_ref: Option<&den_cabinet::MissionRef>,
) -> Result<(), CabinetError> {
    if collection_ref.is_some() || mission_ref.is_some() {
        return Err(CabinetError::Policy(
            "collections and Missions are not available yet (Cabinet Phase 2)".to_string(),
        ));
    }
    Ok(())
}

/// `cabinet_create_item`: creates the item and its first published version
/// atomically. Phase 1 direct-edit: the version publishes immediately.
pub async fn create_item(
    pool: &PgPool,
    request: CreateItemRequest,
) -> Result<ItemView, CabinetError> {
    create_at(pool, request, None).await
}

pub async fn create_child(
    pool: &PgPool,
    request: CreateItemRequest,
    parent: &CabinetItemRef,
) -> Result<ItemView, CabinetError> {
    create_at(pool, request, Some(parent)).await
}

async fn create_at(
    pool: &PgPool,
    request: CreateItemRequest,
    parent: Option<&CabinetItemRef>,
) -> Result<ItemView, CabinetError> {
    authorize(pool, &request.scope).await?;
    reject_phase2_bindings(
        request.collection_ref.as_ref(),
        request.mission_ref.as_ref(),
    )?;
    let title = request.title.trim();
    if title.is_empty() {
        return Err(violation(ContractViolation::EmptyField {
            record: "cabinet_item",
            field: "title",
        }));
    }
    for link in &request.source_links {
        validate_source_locator(link.source_kind, &link.locator).map_err(violation)?;
    }

    let cabinet_ref = CabinetItemRef::mint();
    let now = OffsetDateTime::now_utc();
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    let parent_page = if let Some(parent) = parent {
        Some(pages::authorize(pool, &request.scope, parent, Authority::Write).await?)
    } else {
        None
    };
    if let Some(ref page) = parent_page {
        let allowed = sqlx::query_scalar!(r#"SELECT COALESCE(bool_and((page_policy->'allowed_kinds') IS NULL OR jsonb_typeof(page_policy->'allowed_kinds')='null' OR (page_policy->'allowed_kinds') ? $2),true) AS "allowed!" FROM cabinet_ancestors($1)"#,page.id,request.kind.as_str()).fetch_one(&mut *tx).await.map_err(db_error)?;
        let depth = sqlx::query_scalar!(
            r#"SELECT COALESCE(max(depth),0) AS "depth!" FROM cabinet_ancestors($1)"#,
            page.id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if !allowed || depth >= 32 {
            return Err(CabinetError::Policy(
                "page kind or depth is disallowed by the destination".into(),
            ));
        }
    }
    let pending = matches!(request.scope.actor, Actor::Bear { .. })
        && parent_page
            .as_ref()
            .is_some_and(|page| page.review_required);
    let version = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        cabinet_ref.clone(),
        1,
        request.content,
        request.scope.clone(),
        now,
        None,
        if pending {
            ReviewState::Pending
        } else {
            ReviewState::None
        },
    )
    .map_err(violation)?;
    let (user_id, bear_id) = actor_denormalized(&request.scope);
    let scope_value = scope_json(&request.scope)?;

    let parent_id = parent_page.map(|page| page.id);
    let item_id = sqlx::query_scalar!(
        r#"INSERT INTO cabinet_items
            (cabinet_ref, kind, title, created_by, created_by_user_id, created_by_bear_id,
             created_at, updated_at, parent_item_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8)
           RETURNING id"#,
        cabinet_ref.as_str(),
        request.kind.as_str(),
        title,
        scope_value.clone(),
        user_id,
        bear_id,
        now,
        parent_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;

    let version_id = sqlx::query_scalar!(
        r#"INSERT INTO cabinet_item_versions
            (version_ref, item_id, revision, content, content_sha256, base_version_ref, review,
             authored_by, authored_by_user_id, authored_by_bear_id, authored_at)
           VALUES ($1, $2, 1, $3, $4, NULL, $9, $5, $6, $7, $8)
           RETURNING id"#,
        version.version_ref().as_str(),
        item_id,
        version.content(),
        version.content_sha256(),
        scope_value.clone(),
        user_id,
        bear_id,
        now,
        version.review().as_str()
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;

    if !pending {
        sqlx::query!(
            "UPDATE cabinet_items SET current_version_id = $2 WHERE id = $1",
            item_id,
            version_id
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    }

    let mut sources = Vec::with_capacity(request.source_links.len());
    for link in &request.source_links {
        let source_ref = CabinetSourceRef::mint();
        sqlx::query!(
            r#"INSERT INTO cabinet_source_links
                (source_ref, item_id, source_kind, locator, role, created_by, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               ON CONFLICT (item_id, source_kind, locator, role) DO NOTHING"#,
            source_ref.as_str(),
            item_id,
            link.source_kind.as_str(),
            link.locator.as_str(),
            link.role.as_str(),
            scope_value.clone(),
            now,
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sources.push(SourceLink {
            source_ref,
            cabinet_ref: cabinet_ref.clone(),
            source_kind: link.source_kind,
            locator: link.locator.clone(),
            role: link.role,
            created_by: request.scope.clone(),
            created_at: now,
        });
    }
    tx.commit().await.map_err(db_error)?;

    let item = CabinetItem {
        cabinet_ref,
        kind: request.kind,
        title: title.to_string(),
        current_version: if pending {
            None
        } else {
            Some(version.version_ref().clone())
        },
        collection_ref: None,
        mission_ref: None,
        created_by: request.scope,
        created_at: now,
        lifecycle: Lifecycle::Active,
    };
    Ok(ItemView {
        item,
        version,
        sources,
        proposed_title: None,
    })
}

/// `cabinet_update_item`: appends a new immutable version and advances the
/// current version. A stale `base_version` fails with a structured conflict
/// carrying the current version ref; the facade never merges.
pub async fn update_item(
    pool: &PgPool,
    request: UpdateItemRequest,
) -> Result<ItemView, CabinetError> {
    authorize(pool, &request.scope).await?;
    if let Some(title) = request.title.as_deref() {
        if title.trim().is_empty() {
            return Err(violation(ContractViolation::EmptyField {
                record: "cabinet_item",
                field: "title",
            }));
        }
    }

    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    let page =
        pages::authorize(pool, &request.scope, &request.cabinet_ref, Authority::Write).await?;
    let pending = matches!(request.scope.actor, Actor::Bear { .. }) && page.review_required;
    let current = sqlx::query!(
        r#"SELECT i.id AS item_id, i.title, i.kind, i.lifecycle,
                  v.version_ref AS current_version_ref, v.revision AS current_revision
           FROM cabinet_items i
           JOIN cabinet_item_versions v ON v.id = i.current_version_id
           WHERE i.cabinet_ref = $1
           FOR UPDATE OF i"#,
        request.cabinet_ref.as_str()
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_error)?
    .ok_or(CabinetError::NotFound)?;

    match current.lifecycle.as_str() {
        "deleted" => return Err(CabinetError::NotFound),
        "archived" => {
            return Err(CabinetError::Policy(
                "item is archived; restore it before editing".to_string(),
            ))
        }
        _ => {}
    }

    let current_version =
        CabinetVersionRef::parse(&current.current_version_ref).map_err(violation)?;
    if current_version != request.base_version {
        return Err(CabinetError::Conflict { current_version });
    }

    let max_revision = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(revision),0) AS "max!" FROM cabinet_item_versions WHERE item_id=$1"#,
        current.item_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let revision = u32::try_from(max_revision)
        .map_err(|_| CabinetError::Policy("revision overflow".to_string()))?
        .checked_add(1)
        .ok_or_else(|| CabinetError::Policy("revision overflow".to_string()))?;
    let now = OffsetDateTime::now_utc();
    let version = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        request.cabinet_ref.clone(),
        revision,
        request.content,
        request.scope.clone(),
        now,
        Some(request.base_version.clone()),
        if pending {
            ReviewState::Pending
        } else {
            ReviewState::None
        },
    )
    .map_err(violation)?;
    let (user_id, bear_id) = actor_denormalized(&request.scope);
    let revision_db = i32::try_from(revision)
        .map_err(|_| CabinetError::Policy("revision overflow".to_string()))?;

    let version_id = sqlx::query_scalar!(
        r#"INSERT INTO cabinet_item_versions
            (version_ref, item_id, revision, content, content_sha256, base_version_ref, review,
             authored_by, authored_by_user_id, authored_by_bear_id, authored_at, proposed_title)
           VALUES ($1, $2, $3, $4, $5, $6, $11, $7, $8, $9, $10, $12)
           RETURNING id"#,
        version.version_ref().as_str(),
        current.item_id,
        revision_db,
        version.content(),
        version.content_sha256(),
        request.base_version.as_str(),
        scope_json(&request.scope)?,
        user_id,
        bear_id,
        now,
        version.review().as_str(),
        request.title.as_deref().map(str::trim)
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;

    let new_title = request
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or(current.title.as_str());
    if !pending {
        sqlx::query!(
        "UPDATE cabinet_items SET current_version_id = $2, title = $3, updated_at = $4 WHERE id = $1",
        current.item_id,
        version_id,
        new_title,
        now,
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    }
    tx.commit().await.map_err(db_error)?;

    read(
        pool,
        ReadRequest {
            scope: request.scope,
            cabinet_ref: request.cabinet_ref,
            version_ref: Some(version.version_ref().clone()),
        },
    )
    .await
}

async fn set_lifecycle(
    pool: &PgPool,
    scope: &ActorScope,
    cabinet_ref: &CabinetItemRef,
    from: &[&str],
    to: Lifecycle,
) -> Result<(), CabinetError> {
    authorize(pool, scope).await?;
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    let page = pages::authorize(pool, scope, cabinet_ref, Authority::Write).await?;
    let row = load_item_row(pool, scope, cabinet_ref)
        .await?
        .ok_or(CabinetError::NotFound)?;
    if !from.contains(&row.lifecycle.as_str()) {
        return Err(CabinetError::Policy(
            "page lifecycle transition is not available".into(),
        ));
    }
    let (human, bear) = actor_denormalized(scope);
    if to == Lifecycle::Deleted {
        let children=sqlx::query_scalar!(r#"SELECT EXISTS(SELECT 1 FROM cabinet_items WHERE parent_item_id=$1 AND lifecycle<>'deleted') AS "children!""#,page.id).fetch_one(&mut *tx).await.map_err(db_error)?;
        if children {
            return Err(CabinetError::Policy(
                "remove child pages deliberately before deleting this page".into(),
            ));
        }
    }
    if to == Lifecycle::Archived {
        let denied=sqlx::query_scalar!(r#"WITH RECURSIVE subtree AS(SELECT id FROM cabinet_items WHERE id=$1 UNION ALL SELECT i.id FROM cabinet_items i JOIN subtree p ON i.parent_item_id=p.id) SELECT EXISTS(SELECT 1 FROM subtree s JOIN cabinet_items i ON i.id=s.id WHERE i.lifecycle <> 'deleted' AND NOT cabinet_can_access(i.id,$2,$3,true)) AS "denied!""#,page.id,human,bear).fetch_one(&mut *tx).await.map_err(db_error)?;
        if denied {
            return Err(CabinetError::NotAuthorized);
        }
        sqlx::query!("WITH RECURSIVE subtree AS(SELECT id FROM cabinet_items WHERE id=$1 UNION ALL SELECT i.id FROM cabinet_items i JOIN subtree p ON i.parent_item_id=p.id) UPDATE cabinet_items SET lifecycle='archived',updated_at=now() WHERE id IN(SELECT id FROM subtree) AND lifecycle <> 'deleted'",page.id).execute(&mut *tx).await.map_err(db_error)?;
    } else {
        sqlx::query!(
            "UPDATE cabinet_items SET lifecycle=$2,updated_at=now() WHERE id=$1",
            page.id,
            to.as_str()
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    }
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

/// `cabinet_archive_item`: `active -> archived`.
pub async fn archive_item(
    pool: &PgPool,
    scope: &ActorScope,
    cabinet_ref: &CabinetItemRef,
) -> Result<(), CabinetError> {
    set_lifecycle(pool, scope, cabinet_ref, &["active"], Lifecycle::Archived).await
}

/// `cabinet_restore_item`: `archived -> active`.
pub async fn restore_item(
    pool: &PgPool,
    scope: &ActorScope,
    cabinet_ref: &CabinetItemRef,
) -> Result<(), CabinetError> {
    set_lifecycle(pool, scope, cabinet_ref, &["archived"], Lifecycle::Active).await
}

/// Tombstone an item. Versions stay readable to the storage layer so existing
/// citations keep their meaning, but the item leaves search, read, and history.
///
/// Phase 1 exposes this to people only (the `/cabinet` UI); no model tool calls
/// it. Hard purge remains an operator/compliance action outside this facade.
pub async fn delete_item(
    pool: &PgPool,
    scope: &ActorScope,
    cabinet_ref: &CabinetItemRef,
) -> Result<(), CabinetError> {
    if matches!(scope.actor, Actor::Bear { .. }) {
        return Err(CabinetError::Policy(
            "deleting a Cabinet item is reserved to people; Bears may archive instead".to_string(),
        ));
    }
    set_lifecycle(
        pool,
        scope,
        cabinet_ref,
        &["active", "archived"],
        Lifecycle::Deleted,
    )
    .await
}

/// `cabinet_link_source`: attach origin/citation provenance to an item.
pub async fn link_source(
    pool: &PgPool,
    request: LinkSourceRequest,
) -> Result<SourceLink, CabinetError> {
    authorize(pool, &request.scope).await?;
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    pages::authorize(pool, &request.scope, &request.cabinet_ref, Authority::Write).await?;
    validate_source_locator(request.link.source_kind, &request.link.locator).map_err(violation)?;
    let row = load_item_row(pool, &request.scope, &request.cabinet_ref)
        .await?
        .ok_or(CabinetError::NotFound)?;
    if row.lifecycle == "deleted" {
        return Err(CabinetError::NotFound);
    }
    let source_ref = CabinetSourceRef::mint();
    let now = OffsetDateTime::now_utc();
    sqlx::query!(
        r#"INSERT INTO cabinet_source_links
            (source_ref, item_id, source_kind, locator, role, created_by, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (item_id, source_kind, locator, role) DO NOTHING"#,
        source_ref.as_str(),
        row.id,
        request.link.source_kind.as_str(),
        request.link.locator.as_str(),
        request.link.role.as_str(),
        scope_json(&request.scope)?,
        now,
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(SourceLink {
        source_ref,
        cabinet_ref: request.cabinet_ref,
        source_kind: request.link.source_kind,
        locator: request.link.locator,
        role: request.link.role,
        created_by: request.scope,
        created_at: now,
    })
}

/// `cabinet_unlink_source`: remove one source link. Never alters versions.
pub async fn unlink_source(
    pool: &PgPool,
    request: UnlinkSourceRequest,
) -> Result<(), CabinetError> {
    authorize(pool, &request.scope).await?;
    let mut tx = pool.begin().await.map_err(db_error)?;
    pages::lock(&mut tx).await?;
    pages::authorize(pool, &request.scope, &request.cabinet_ref, Authority::Write).await?;
    let row = load_item_row(pool, &request.scope, &request.cabinet_ref)
        .await?
        .ok_or(CabinetError::NotFound)?;
    let deleted = sqlx::query!(
        "DELETE FROM cabinet_source_links WHERE item_id = $1 AND source_ref = $2",
        row.id,
        request.source_ref.as_str()
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    if deleted.rows_affected() == 0 {
        return Err(CabinetError::NotFound);
    }
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use den_cabinet::{NewSourceLink, SearchFilters, SourceKind, SourceRole};
    use den_core::execution_context::RuntimeContextLabel;

    static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn test_pool() -> Option<PgPool> {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .ok()?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .ok()?;
        sqlx::migrate!("../../migrations").run(&pool).await.ok()?;
        Some(pool)
    }

    async fn create_user(pool: &PgPool) -> ActorScope {
        let suffix = Uuid::new_v4().simple().to_string();
        let username = format!("cabinetuser{}", &suffix[..12]);
        let user_id = sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash)
             VALUES ($1, $2, $3, 'x') RETURNING id",
            format!("{username}@example.invalid"),
            &username,
            &username,
        )
        .fetch_one(pool)
        .await
        .expect("create user");
        ActorScope::user(den_core::ids::UserId(user_id))
    }

    async fn create_bear(pool: &PgPool, cabinet_enabled: bool) -> ActorScope {
        let suffix = Uuid::new_v4().simple().to_string();
        let bear_id = sqlx::query_scalar!(
            "INSERT INTO bears (slug, name, cabinet_enabled) VALUES ($1, $2, $3) RETURNING id",
            format!("cabinet-bear-{}", &suffix[..12]),
            "Cabinet Test Bear",
            cabinet_enabled,
        )
        .fetch_one(pool)
        .await
        .expect("create bear");
        ActorScope::bear(
            den_core::ids::BearId::new(bear_id),
            RuntimeContextLabel::ChannelConversation,
        )
    }

    fn create_request(scope: ActorScope, title: &str, content: &str) -> CreateItemRequest {
        CreateItemRequest {
            scope,
            kind: ItemKind::Document,
            title: title.to_string(),
            content: content.to_string(),
            collection_ref: None,
            mission_ref: None,
            source_links: Vec::new(),
        }
    }

    #[tokio::test]
    async fn human_and_authorized_bear_share_an_item() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let human = create_user(&pool).await;
        let bear = create_bear(&pool, true).await;

        // The human creates the item.
        let created = create_item(
            &pool,
            create_request(human.clone(), "Deploy runbook", "Step one: breathe."),
        )
        .await
        .expect("create item");
        assert_eq!(created.version.revision(), 1);
        assert_eq!(created.version.review(), ReviewState::None);

        // The authorized Bear finds and reads it.
        let found = search(
            &pool,
            SearchRequest {
                scope: bear.clone(),
                query: "breathe".to_string(),
                filters: SearchFilters::default(),
            },
        )
        .await
        .expect("bear search");
        assert!(found
            .iter()
            .any(|item| item.cabinet_ref == created.item.cabinet_ref));

        let read_back = read(
            &pool,
            ReadRequest {
                scope: bear.clone(),
                cabinet_ref: created.item.cabinet_ref.clone(),
                version_ref: None,
            },
        )
        .await
        .expect("bear read");
        assert_eq!(read_back.version.content(), "Step one: breathe.");

        // The Bear publishes revision 2; the human sees it.
        let updated = update_item(
            &pool,
            UpdateItemRequest {
                scope: bear,
                cabinet_ref: created.item.cabinet_ref.clone(),
                content: "Step one: breathe. Step two: deploy.".to_string(),
                base_version: created.version.version_ref().clone(),
                title: None,
            },
        )
        .await
        .expect("bear update");
        assert_eq!(updated.version.revision(), 2);
        assert_eq!(
            updated.version.base_version(),
            Some(created.version.version_ref())
        );

        let human_view = read(
            &pool,
            ReadRequest {
                scope: human.clone(),
                cabinet_ref: created.item.cabinet_ref.clone(),
                version_ref: None,
            },
        )
        .await
        .expect("human read");
        assert_eq!(
            human_view.version.content(),
            "Step one: breathe. Step two: deploy."
        );

        let versions = history(
            &pool,
            HistoryRequest {
                scope: human,
                cabinet_ref: created.item.cabinet_ref,
            },
        )
        .await
        .expect("history");
        assert_eq!(
            versions.iter().map(|v| v.revision).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[tokio::test]
    async fn unauthorized_bear_cannot_read_or_alter() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let human = create_user(&pool).await;
        let disabled_bear = create_bear(&pool, false).await;

        let created = create_item(
            &pool,
            create_request(human, "Secret-ish runbook", "content"),
        )
        .await
        .expect("create item");

        let read_result = read(
            &pool,
            ReadRequest {
                scope: disabled_bear.clone(),
                cabinet_ref: created.item.cabinet_ref.clone(),
                version_ref: None,
            },
        )
        .await;
        assert_eq!(read_result.unwrap_err(), CabinetError::NotAuthorized);

        let search_result = search(
            &pool,
            SearchRequest {
                scope: disabled_bear.clone(),
                query: String::new(),
                filters: SearchFilters::default(),
            },
        )
        .await;
        assert_eq!(search_result.unwrap_err(), CabinetError::NotAuthorized);

        let update_result = update_item(
            &pool,
            UpdateItemRequest {
                scope: disabled_bear,
                cabinet_ref: created.item.cabinet_ref,
                content: "vandalized".to_string(),
                base_version: created.version.version_ref().clone(),
                title: None,
            },
        )
        .await;
        assert_eq!(update_result.unwrap_err(), CabinetError::NotAuthorized);
    }

    #[tokio::test]
    async fn stale_base_version_conflicts_instead_of_merging() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let human = create_user(&pool).await;
        let created = create_item(&pool, create_request(human.clone(), "Contended", "v1"))
            .await
            .expect("create item");

        let first = update_item(
            &pool,
            UpdateItemRequest {
                scope: human.clone(),
                cabinet_ref: created.item.cabinet_ref.clone(),
                content: "v2".to_string(),
                base_version: created.version.version_ref().clone(),
                title: None,
            },
        )
        .await
        .expect("first update");

        let second = update_item(
            &pool,
            UpdateItemRequest {
                scope: human,
                cabinet_ref: created.item.cabinet_ref,
                content: "v2-competing".to_string(),
                base_version: created.version.version_ref().clone(),
                title: None,
            },
        )
        .await;
        assert_eq!(
            second.unwrap_err(),
            CabinetError::Conflict {
                current_version: first.version.version_ref().clone()
            }
        );
    }

    #[tokio::test]
    async fn phase2_bindings_and_pending_review_are_rejected() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let human = create_user(&pool).await;
        let mut request = create_request(human.clone(), "Mission-bound", "content");
        request.mission_ref = Some(den_cabinet::MissionRef::mint());
        let result = create_item(&pool, request).await;
        assert!(matches!(result.unwrap_err(), CabinetError::Policy(_)));

        // Source links round-trip with kind/locator validation.
        let mut with_source = create_request(human, "Sourced", "content");
        with_source.source_links = vec![NewSourceLink {
            source_kind: SourceKind::Offline,
            locator: "book://isbn/9780262046305".to_string(),
            role: SourceRole::Origin,
        }];
        let view = create_item(&pool, with_source).await.expect("create");
        assert_eq!(view.sources.len(), 1);

        let mut bad_source = create_request(view.item.created_by.clone(), "Bad source", "x");
        bad_source.source_links = vec![NewSourceLink {
            source_kind: SourceKind::Url,
            locator: "not-a-url".to_string(),
            role: SourceRole::Citation,
        }];
        assert!(matches!(
            create_item(&pool, bad_source).await.unwrap_err(),
            CabinetError::Validation(_)
        ));
    }

    #[tokio::test]
    async fn archive_hides_from_default_search_and_blocks_edits() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let human = create_user(&pool).await;
        let created = create_item(&pool, create_request(human.clone(), "Old notes", "v1"))
            .await
            .expect("create item");

        archive_item(&pool, &human, &created.item.cabinet_ref)
            .await
            .expect("archive");

        let active = search(
            &pool,
            SearchRequest {
                scope: human.clone(),
                query: "Old notes".to_string(),
                filters: SearchFilters::default(),
            },
        )
        .await
        .expect("search active");
        assert!(!active
            .iter()
            .any(|item| item.cabinet_ref == created.item.cabinet_ref));

        let update_result = update_item(
            &pool,
            UpdateItemRequest {
                scope: human.clone(),
                cabinet_ref: created.item.cabinet_ref.clone(),
                content: "v2".to_string(),
                base_version: created.version.version_ref().clone(),
                title: None,
            },
        )
        .await;
        assert!(matches!(
            update_result.unwrap_err(),
            CabinetError::Policy(_)
        ));

        restore_item(&pool, &human, &created.item.cabinet_ref)
            .await
            .expect("restore");
        let read_back = read(
            &pool,
            ReadRequest {
                scope: human,
                cabinet_ref: created.item.cabinet_ref,
                version_ref: None,
            },
        )
        .await
        .expect("read after restore");
        assert_eq!(read_back.item.lifecycle, Lifecycle::Active);
    }

    #[tokio::test]
    async fn people_can_delete_but_bears_can_only_archive() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let person = create_user(&pool).await;
        let bear = create_bear(&pool, true).await;
        let view = create_item(&pool, create_request(person.clone(), "Deletable", "body"))
            .await
            .expect("create");
        let cabinet_ref = view.item.cabinet_ref.clone();

        // A Bear may not delete, whatever its stance.
        let refused = delete_item(&pool, &bear, &cabinet_ref).await;
        assert!(matches!(refused, Err(CabinetError::Policy(_))));
        // ... and the item is untouched.
        read(
            &pool,
            ReadRequest {
                scope: bear.clone(),
                cabinet_ref: cabinet_ref.clone(),
                version_ref: None,
            },
        )
        .await
        .expect("still readable after refused delete");

        // A person may, and the tombstone hides it from read, history, and search.
        delete_item(&pool, &person, &cabinet_ref)
            .await
            .expect("person deletes");
        assert!(matches!(
            read(
                &pool,
                ReadRequest {
                    scope: person.clone(),
                    cabinet_ref: cabinet_ref.clone(),
                    version_ref: None,
                },
            )
            .await,
            Err(CabinetError::NotFound)
        ));
        assert!(matches!(
            history(
                &pool,
                HistoryRequest {
                    scope: person.clone(),
                    cabinet_ref: cabinet_ref.clone(),
                },
            )
            .await,
            Err(CabinetError::NotFound)
        ));
        let hits = search(
            &pool,
            SearchRequest {
                scope: person,
                query: "Deletable".to_string(),
                filters: SearchFilters::default(),
            },
        )
        .await
        .expect("search");
        assert!(!hits.iter().any(|hit| hit.cabinet_ref == cabinet_ref));
    }

    #[tokio::test]
    async fn source_links_can_be_added_and_removed_after_creation() {
        let _guard = DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let bear = create_bear(&pool, true).await;
        let view = create_item(&pool, create_request(bear.clone(), "Sourced", "body"))
            .await
            .expect("create");
        assert!(view.sources.is_empty());
        let cabinet_ref = view.item.cabinet_ref.clone();

        let link = link_source(
            &pool,
            LinkSourceRequest {
                scope: bear.clone(),
                cabinet_ref: cabinet_ref.clone(),
                link: den_cabinet::NewSourceLink {
                    source_kind: SourceKind::Url,
                    locator: "https://example.com/a".to_string(),
                    role: SourceRole::Origin,
                },
            },
        )
        .await
        .expect("link source");

        // A locator that contradicts its kind is refused.
        assert!(link_source(
            &pool,
            LinkSourceRequest {
                scope: bear.clone(),
                cabinet_ref: cabinet_ref.clone(),
                link: den_cabinet::NewSourceLink {
                    source_kind: SourceKind::Url,
                    locator: "book://isbn/9780262046305".to_string(),
                    role: SourceRole::Citation,
                },
            },
        )
        .await
        .is_err());

        let after_add = read(
            &pool,
            ReadRequest {
                scope: bear.clone(),
                cabinet_ref: cabinet_ref.clone(),
                version_ref: None,
            },
        )
        .await
        .expect("read");
        assert_eq!(after_add.sources.len(), 1);
        // Linking provenance does not publish a revision.
        assert_eq!(after_add.version.revision(), 1);

        unlink_source(
            &pool,
            UnlinkSourceRequest {
                scope: bear.clone(),
                cabinet_ref: cabinet_ref.clone(),
                source_ref: link.source_ref.clone(),
            },
        )
        .await
        .expect("unlink source");
        let after_remove = read(
            &pool,
            ReadRequest {
                scope: bear.clone(),
                cabinet_ref,
                version_ref: None,
            },
        )
        .await
        .expect("read");
        assert!(after_remove.sources.is_empty());
        assert_eq!(after_remove.version.revision(), 1);
    }
}
