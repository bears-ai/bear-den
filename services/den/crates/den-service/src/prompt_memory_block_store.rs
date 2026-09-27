#![allow(dead_code)]

use den_core::{BearProfile, DenError};
use sqlx::{PgPool, Row};

use crate::prompt_memory_blocks::{
    PromptMemoryBlock, PromptMemoryBlockScope, PromptMemoryBlockState, PromptMemoryBlockType,
};

// Write/patch DTOs now live alongside the prompt-memory tool executors in
// `den-tools`; re-exported here so the Postgres store keeps a stable path.
pub use den_core::tools::prompt_memory::{
    PromptMemoryBlockPatch, PromptMemoryBlockWrite, PromptMemoryVisibility,
};

#[derive(Debug, Clone)]
pub struct PromptMemoryBlockQuery<'a> {
    pub bear_id: Option<uuid::Uuid>,
    pub profile_slug: &'a str,
    pub session_id: &'a str,
    pub work_surfaces: &'a [String],
    pub visibility: PromptMemoryVisibility,
}

#[derive(Debug, Clone)]
pub struct PromptMemoryRuntimeSelection {
    pub blocks: Vec<PromptMemoryBlock>,
    pub diagnostic: serde_json::Value,
}

pub async fn upsert_prompt_memory_block(
    pool: &PgPool,
    write: &PromptMemoryBlockWrite,
) -> Result<(), DenError> {
    let result = sqlx::query!(
        r"
        INSERT INTO prompt_memory_blocks (
            block_id,
            bear_id,
            profile_slug,
            scope,
            block_type,
            state,
            work_surface,
            session_id,
            title,
            body,
            priority,
            created_by_user_id,
            supersedes_block_id,
            metadata
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        ON CONFLICT (block_id)
        DO UPDATE SET
            bear_id = EXCLUDED.bear_id,
            profile_slug = EXCLUDED.profile_slug,
            scope = EXCLUDED.scope,
            block_type = EXCLUDED.block_type,
            state = EXCLUDED.state,
            work_surface = EXCLUDED.work_surface,
            session_id = EXCLUDED.session_id,
            title = EXCLUDED.title,
            body = EXCLUDED.body,
            priority = EXCLUDED.priority,
            created_by_user_id = EXCLUDED.created_by_user_id,
            supersedes_block_id = EXCLUDED.supersedes_block_id,
            metadata = EXCLUDED.metadata,
            updated_at = now()
        WHERE prompt_memory_blocks.bear_id IS NOT DISTINCT FROM EXCLUDED.bear_id
          AND prompt_memory_blocks.profile_slug IS NOT DISTINCT FROM EXCLUDED.profile_slug
          AND prompt_memory_blocks.scope = EXCLUDED.scope
          AND prompt_memory_blocks.session_id IS NOT DISTINCT FROM EXCLUDED.session_id
          AND prompt_memory_blocks.work_surface IS NOT DISTINCT FROM EXCLUDED.work_surface
        ",
        &write.block_id,
        write.bear_id,
        write.profile_slug.as_deref(),
        scope_to_db(write.scope),
        block_type_to_db(write.block_type),
        state_to_db(write.state),
        write.work_surface.as_deref(),
        write.session_id.as_deref(),
        &write.title,
        &write.body,
        write.priority,
        write.created_by_user_id,
        write.supersedes_block_id.as_deref(),
        &write.metadata,
    )
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("upsert prompt_memory_blocks: {err}")))?;
    if result.rows_affected() == 0 {
        return Err(DenError::Authorization(
            "prompt memory block ID belongs to a different Bear or scope".into(),
        ));
    }
    Ok(())
}

pub async fn patch_prompt_memory_block(
    pool: &PgPool,
    bear_id: uuid::Uuid,
    profile: BearProfile,
    block_id: &str,
    patch: &PromptMemoryBlockPatch,
) -> Result<(), DenError> {
    let result = sqlx::query!(
        r"
        UPDATE prompt_memory_blocks
        SET state = $2,
            title = $3,
            body = $4,
            priority = $5,
            supersedes_block_id = $6,
            metadata = $7,
            updated_at = now()
        WHERE block_id = $1 AND bear_id = $8 AND profile_slug = $9
        ",
        block_id,
        state_to_db(patch.state),
        &patch.title,
        &patch.body,
        patch.priority,
        patch.supersedes_block_id.as_deref(),
        &patch.metadata,
        bear_id,
        profile.as_str(),
    )
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("patch prompt_memory_blocks: {err}")))?;
    if result.rows_affected() == 0 {
        return Err(DenError::NotFound(format!(
            "prompt memory block not found: {block_id}"
        )));
    }
    Ok(())
}

pub async fn list_prompt_memory_blocks_for_runtime(
    pool: &PgPool,
    query: PromptMemoryBlockQuery<'_>,
) -> Result<Vec<PromptMemoryBlock>, DenError> {
    let rows = sqlx::query(
        r"
        SELECT block_id, scope, block_type, state, profile_slug, work_surface, session_id, title, body, priority
        FROM prompt_memory_blocks
        WHERE state = 'active'
          AND (bear_id = $1 OR bear_id IS NULL)
          AND (
            scope = 'bear_wide'
            OR (scope = 'profile_local' AND profile_slug = $2)
            OR (scope = 'session' AND session_id = $3)
            OR (scope = 'work_surface' AND work_surface = ANY($4))
          )
        ORDER BY priority DESC, updated_at DESC
        ",
    )
    .bind(query.bear_id)
    .bind(query.profile_slug)
    .bind(query.session_id)
    .bind(query.work_surfaces)
    .fetch_all(pool)
    .await
    .map_err(|err| DenError::Database(format!("select prompt_memory_blocks: {err}")))?;

    rows.into_iter()
        .map(row_to_block)
        .filter_map(|result| match result {
            Ok(block) if query.visibility.allows(&block, query.session_id) => Some(Ok(block)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub async fn list_prompt_memory_blocks_for_bear_profile(
    pool: &PgPool,
    bear_id: uuid::Uuid,
    profile_slug: &str,
) -> Result<Vec<PromptMemoryBlock>, DenError> {
    let rows = sqlx::query(
        r"
        SELECT block_id, scope, block_type, state, profile_slug, work_surface, session_id, title, body, priority
        FROM prompt_memory_blocks
        WHERE bear_id = $1
          AND profile_slug = $2
        ORDER BY updated_at DESC, priority DESC
        ",
    )
    .bind(bear_id)
    .bind(profile_slug)
    .fetch_all(pool)
    .await
    .map_err(|err| DenError::Database(format!("list prompt_memory_blocks for bear role: {err}")))?;

    rows.into_iter().map(row_to_block).collect()
}

pub async fn archive_prompt_memory_blocks_superseded_by(
    pool: &PgPool,
    bear_id: uuid::Uuid,
    profile_slug: &str,
    supersedes_block_id: &str,
) -> Result<u64, DenError> {
    let result = sqlx::query(
        r"
        UPDATE prompt_memory_blocks
        SET state = 'archived', updated_at = now()
        WHERE bear_id = $1
          AND profile_slug = $2
          AND block_id = $3
          AND state <> 'archived'
        ",
    )
    .bind(bear_id)
    .bind(profile_slug)
    .bind(supersedes_block_id)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("archive superseded prompt_memory_blocks: {err}")))?;
    Ok(result.rows_affected())
}

pub async fn archive_conflicting_prompt_memory_blocks(
    pool: &PgPool,
    write: &PromptMemoryBlockWrite,
) -> Result<u64, DenError> {
    let result = sqlx::query(
        r"
        UPDATE prompt_memory_blocks
        SET state = 'archived', updated_at = now()
        WHERE bear_id = $1
          AND profile_slug = $2
          AND block_id <> $3
          AND state = 'active'
          AND scope = $4
          AND block_type = $5
          AND COALESCE(work_surface, '') = COALESCE($6, '')
          AND COALESCE(session_id, '') = COALESCE($7, '')
        ",
    )
    .bind(write.bear_id)
    .bind(write.profile_slug.as_deref())
    .bind(&write.block_id)
    .bind(scope_to_db(write.scope))
    .bind(block_type_to_db(write.block_type))
    .bind(write.work_surface.as_deref())
    .bind(write.session_id.as_deref())
    .execute(pool)
    .await
    .map_err(|err| {
        DenError::Database(format!("archive conflicting prompt_memory_blocks: {err}"))
    })?;
    Ok(result.rows_affected())
}

pub async fn select_prompt_memory_blocks_for_runtime(
    pool: &PgPool,
    query: PromptMemoryBlockQuery<'_>,
) -> Result<PromptMemoryRuntimeSelection, DenError> {
    let blocks = list_prompt_memory_blocks_for_runtime(pool, query.clone()).await?;
    let diagnostic = serde_json::json!({
        "source": "prompt_memory_blocks",
        "persisted": true,
        "bear_id": query.bear_id.map(|id| id.to_string()),
        "profile_slug": query.profile_slug,
        "session_id": query.session_id,
        "work_surfaces": query.work_surfaces,
        "visibility": format!("{:?}", query.visibility),
        "matched_block_ids": blocks.iter().map(|block| block.id.clone()).collect::<Vec<_>>(),
        "matched_count": blocks.len(),
    });
    Ok(PromptMemoryRuntimeSelection { blocks, diagnostic })
}

fn row_to_block(row: sqlx::postgres::PgRow) -> Result<PromptMemoryBlock, DenError> {
    Ok(PromptMemoryBlock {
        id: row.try_get("block_id").map_err(db_decode("block_id"))?,
        block_type: block_type_from_db(
            &row.try_get::<String, _>("block_type")
                .map_err(db_decode("block_type"))?,
        )?,
        scope: scope_from_db(
            &row.try_get::<String, _>("scope")
                .map_err(db_decode("scope"))?,
        )?,
        state: state_from_db(
            &row.try_get::<String, _>("state")
                .map_err(db_decode("state"))?,
        )?,
        role: row
            .try_get("profile_slug")
            .map_err(db_decode("profile_slug"))?,
        work_surface: row
            .try_get("work_surface")
            .map_err(db_decode("work_surface"))?,
        session_id: row.try_get("session_id").map_err(db_decode("session_id"))?,
        title: row.try_get("title").map_err(db_decode("title"))?,
        body: row.try_get("body").map_err(db_decode("body"))?,
        priority: row.try_get("priority").map_err(db_decode("priority"))?,
    })
}

fn db_decode(field: &'static str) -> impl Fn(sqlx::Error) -> DenError {
    move |err| DenError::Database(format!("decode prompt_memory_blocks {field}: {err}"))
}

fn scope_to_db(scope: PromptMemoryBlockScope) -> &'static str {
    match scope {
        PromptMemoryBlockScope::BearWide => "bear_wide",
        PromptMemoryBlockScope::RoleLocal => "profile_local",
        PromptMemoryBlockScope::WorkSurface => "work_surface",
        PromptMemoryBlockScope::Session => "session",
    }
}

fn block_type_to_db(block_type: PromptMemoryBlockType) -> &'static str {
    match block_type {
        PromptMemoryBlockType::RoleGuidance => "profile_guidance",
        PromptMemoryBlockType::WorkSurfaceContext => "work_surface_context",
        PromptMemoryBlockType::SessionFocus => "session_focus",
        PromptMemoryBlockType::UserInstruction => "user_instruction",
    }
}

fn state_to_db(state: PromptMemoryBlockState) -> &'static str {
    match state {
        PromptMemoryBlockState::Draft => "draft",
        PromptMemoryBlockState::Active => "active",
        PromptMemoryBlockState::Superseded => "superseded",
        PromptMemoryBlockState::Archived => "archived",
    }
}

fn scope_from_db(value: &str) -> Result<PromptMemoryBlockScope, DenError> {
    match value {
        "bear_wide" => Ok(PromptMemoryBlockScope::BearWide),
        "profile_local" => Ok(PromptMemoryBlockScope::RoleLocal),
        "work_surface" => Ok(PromptMemoryBlockScope::WorkSurface),
        "session" => Ok(PromptMemoryBlockScope::Session),
        other => Err(DenError::Database(format!(
            "unknown prompt memory scope: {other}"
        ))),
    }
}

fn block_type_from_db(value: &str) -> Result<PromptMemoryBlockType, DenError> {
    match value {
        "profile_guidance" => Ok(PromptMemoryBlockType::RoleGuidance),
        "work_surface_context" => Ok(PromptMemoryBlockType::WorkSurfaceContext),
        "session_focus" => Ok(PromptMemoryBlockType::SessionFocus),
        "user_instruction" => Ok(PromptMemoryBlockType::UserInstruction),
        other => Err(DenError::Database(format!(
            "unknown prompt memory block type: {other}"
        ))),
    }
}

fn state_from_db(value: &str) -> Result<PromptMemoryBlockState, DenError> {
    match value {
        "draft" => Ok(PromptMemoryBlockState::Draft),
        "active" => Ok(PromptMemoryBlockState::Active),
        "superseded" => Ok(PromptMemoryBlockState::Superseded),
        "archived" => Ok(PromptMemoryBlockState::Archived),
        other => Err(DenError::Database(format!(
            "unknown prompt memory block state: {other}"
        ))),
    }
}
