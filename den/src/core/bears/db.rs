//! SQL for bears and `user_bear` (runtime `query_as` — see `model.rs`).

use sqlx::{PgPool, types::Json};
use uuid::Uuid;

use crate::errors::CustomError;

use super::model::{Bear, BearWithMembership};

pub async fn list_bears(pool: &PgPool) -> Result<Vec<Bear>, CustomError> {
    sqlx::query_as::<_, Bear>(
        r#"
        SELECT id, slug, name, description, letta_agent_id, default_model, tools_enabled,
               letta_agent_type, letta_tool_ids, system_prompt, created_at, updated_at
        FROM bears
        ORDER BY slug
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn get_bear(pool: &PgPool, id: Uuid) -> Result<Option<Bear>, CustomError> {
    sqlx::query_as::<_, Bear>(
        r#"
        SELECT id, slug, name, description, letta_agent_id, default_model, tools_enabled,
               letta_agent_type, letta_tool_ids, system_prompt, created_at, updated_at
        FROM bears
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn bear_slug_exists(pool: &PgPool, slug: &str) -> Result<bool, CustomError> {
    let n: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM bears WHERE slug = $1",
    )
    .bind(slug)
    .fetch_one(pool)
    .await?;
    Ok(n.0 > 0)
}

pub async fn bear_slug_exists_excluding(
    pool: &PgPool,
    slug: &str,
    exclude_id: Uuid,
) -> Result<bool, CustomError> {
    let n: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM bears WHERE slug = $1 AND id <> $2",
    )
    .bind(slug)
    .bind(exclude_id)
    .fetch_one(pool)
    .await?;
    Ok(n.0 > 0)
}

pub async fn update_bear(
    pool: &PgPool,
    id: Uuid,
    slug: &str,
    name: &str,
    description: &str,
    system_prompt: &str,
    default_model: Option<&str>,
    tools_enabled: Option<Json<serde_json::Value>>,
    letta_agent_type: Option<&str>,
    letta_tool_ids: Json<Vec<String>>,
) -> Result<(), CustomError> {
    let r = sqlx::query(
        r#"
        UPDATE bears
        SET slug = $1,
            name = $2,
            description = $3,
            system_prompt = $4,
            default_model = $5,
            tools_enabled = $6,
            letta_agent_type = $7,
            letta_tool_ids = $8,
            updated_at = NOW()
        WHERE id = $9
        "#,
    )
    .bind(slug)
    .bind(name)
    .bind(description)
    .bind(system_prompt)
    .bind(default_model)
    .bind(tools_enabled)
    .bind(letta_agent_type)
    .bind(letta_tool_ids)
    .bind(id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    Ok(())
}

/// `letta_agent_id` stays unset until Letta provisions the agent.
pub async fn create_bear(
    pool: &PgPool,
    slug: &str,
    name: &str,
    description: &str,
    system_prompt: &str,
    default_model: Option<&str>,
    tools_enabled: Option<Json<serde_json::Value>>,
    letta_agent_type: Option<&str>,
    letta_tool_ids: Json<Vec<String>>,
) -> Result<Uuid, CustomError> {
    let row: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO bears (
            slug, name, description, system_prompt, default_model, tools_enabled,
            letta_agent_type, letta_tool_ids, letta_agent_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL)
        RETURNING id
        "#,
    )
    .bind(slug)
    .bind(name)
    .bind(description)
    .bind(system_prompt)
    .bind(default_model)
    .bind(tools_enabled)
    .bind(letta_agent_type)
    .bind(letta_tool_ids)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Canonical role for users who manage membership and bear settings (not site `users.is_admin`).
pub const BEAR_ROLE_ADMIN: &str = "admin";
pub const BEAR_ROLE_MEMBER: &str = "member";

#[inline]
pub fn role_is_bear_admin(role: Option<&str>) -> bool {
    matches!(
        role.map(|s| s.trim().eq_ignore_ascii_case(BEAR_ROLE_ADMIN)),
        Some(true)
    )
}

pub async fn grant_membership(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    role: Option<&str>,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO user_bear (user_id, bear_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, bear_id) DO UPDATE SET role = EXCLUDED.role
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn revoke_membership(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
) -> Result<(), CustomError> {
    let r = sqlx::query("DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2")
        .bind(user_id)
        .bind(bear_id)
        .execute(pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(CustomError::NotFound("membership not found".to_string()));
    }
    Ok(())
}

pub async fn delete_bear(pool: &PgPool, bear_id: Uuid) -> Result<(), CustomError> {
    let r = sqlx::query("DELETE FROM bears WHERE id = $1")
        .bind(bear_id)
        .execute(pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct BearMemberRow {
    pub user_id: i32,
    pub username: String,
    pub role: Option<String>,
}

pub async fn list_members_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearMemberRow>, CustomError> {
    sqlx::query_as::<_, BearMemberRow>(
        r#"
        SELECT ub.user_id, u.username, ub.role
        FROM user_bear ub
        INNER JOIN users u ON u.id = ub.user_id
        WHERE ub.bear_id = $1
        ORDER BY
            CASE WHEN lower(btrim(coalesce(ub.role, ''))) = 'admin' THEN 0 ELSE 1 END,
            u.username
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn count_bear_admins(pool: &PgPool, bear_id: Uuid) -> Result<i64, CustomError> {
    let n: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::bigint
        FROM user_bear
        WHERE bear_id = $1
          AND lower(btrim(coalesce(role, ''))) = 'admin'
        "#,
    )
    .bind(bear_id)
    .fetch_one(pool)
    .await?;
    Ok(n.0)
}

pub async fn membership_role_for_user(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
) -> Result<Option<Option<String>>, CustomError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT role FROM user_bear WHERE user_id = $1 AND bear_id = $2",
    )
    .bind(user_id)
    .bind(bear_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct BearChatActivityRow {
    pub id: i64,
    pub username: String,
    pub channel: String,
    pub message_preview: String,
    pub created_at: time::OffsetDateTime,
}

pub async fn record_chat_activity(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    channel: &str,
    message_preview: &str,
) -> Result<(), CustomError> {
    let preview: String = message_preview.chars().take(500).collect();
    sqlx::query(
        r#"
        INSERT INTO bear_chat_activity (bear_id, user_id, channel, message_preview)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(bear_id)
    .bind(user_id)
    .bind(channel)
    .bind(preview)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_chat_activity_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<BearChatActivityRow>, CustomError> {
    sqlx::query_as::<_, BearChatActivityRow>(
        r#"
        SELECT a.id, u.username, a.channel, a.message_preview, a.created_at
        FROM bear_chat_activity a
        INNER JOIN users u ON u.id = a.user_id
        WHERE a.bear_id = $1
        ORDER BY a.created_at DESC
        LIMIT $2
        "#,
    )
    .bind(bear_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct MembershipRow {
    pub user_id: i32,
    pub username: String,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub bear_name: String,
    pub role: Option<String>,
}

pub async fn list_memberships(pool: &PgPool) -> Result<Vec<MembershipRow>, CustomError> {
    sqlx::query_as::<_, MembershipRow>(
        r#"
        SELECT ub.user_id, u.username, ub.bear_id, b.slug AS bear_slug, b.name AS bear_name, ub.role
        FROM user_bear ub
        INNER JOIN users u ON u.id = ub.user_id
        INNER JOIN bears b ON b.id = ub.bear_id
        ORDER BY u.username, b.slug
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_bears_for_user(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<BearWithMembership>, CustomError> {
    sqlx::query_as::<_, BearWithMembership>(
        r#"
        SELECT b.id, b.slug, b.name, b.description, b.letta_agent_id, b.default_model, b.tools_enabled,
               b.letta_agent_type, b.letta_tool_ids, b.system_prompt, b.created_at, b.updated_at,
               ub.role AS membership_role
        FROM bears b
        INNER JOIN user_bear ub ON ub.bear_id = b.id
        WHERE ub.user_id = $1
        ORDER BY b.slug
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Bear visible to the user via `user_bear`, keyed by slug (for `/bear/{slug}`).
pub async fn bear_for_user_by_slug(
    pool: &PgPool,
    user_id: i32,
    slug: &str,
) -> Result<Option<Bear>, CustomError> {
    sqlx::query_as::<_, Bear>(
        r#"
        SELECT b.id, b.slug, b.name, b.description, b.letta_agent_id, b.default_model, b.tools_enabled,
               b.letta_agent_type, b.letta_tool_ids, b.system_prompt, b.created_at, b.updated_at
        FROM bears b
        INNER JOIN user_bear ub ON ub.bear_id = b.id
        WHERE ub.user_id = $1 AND b.slug = $2
        "#,
    )
    .bind(user_id)
    .bind(slug)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn count_bear_members(pool: &PgPool, bear_id: Uuid) -> Result<i64, CustomError> {
    let n: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM user_bear WHERE bear_id = $1",
    )
    .bind(bear_id)
    .fetch_one(pool)
    .await?;
    Ok(n.0)
}

pub async fn user_may_use_bear(pool: &PgPool, user_id: i32, bear_id: Uuid) -> Result<bool, CustomError> {
    let n: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::bigint FROM user_bear WHERE user_id = $1 AND bear_id = $2
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .fetch_one(pool)
    .await?;
    Ok(n.0 > 0)
}

/// Non-empty `letta_agent_id` values already assigned to some bear (for orphan-agent UI).
pub async fn list_letta_agent_ids_in_use(pool: &PgPool) -> Result<Vec<String>, CustomError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT letta_agent_id
        FROM bears
        WHERE letta_agent_id IS NOT NULL AND btrim(letta_agent_id) <> ''
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn bear_exists_for_letta_agent_id(
    pool: &PgPool,
    agent_id: &str,
) -> Result<bool, CustomError> {
    let n: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM bears WHERE letta_agent_id = $1",
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await?;
    Ok(n.0 > 0)
}

pub async fn set_letta_agent_id(
    pool: &PgPool,
    bear_id: Uuid,
    agent_id: &str,
) -> Result<(), CustomError> {
    sqlx::query(
        "UPDATE bears SET letta_agent_id = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(agent_id)
    .bind(bear_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// One row per `user_bear` for LettaBot YAML (`username` + `bear_slug` + optional Letta id).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LettaBotRow {
    pub username: String,
    pub bear_slug: String,
    pub letta_agent_id: Option<String>,
}

pub async fn list_lettabot_rows(pool: &PgPool) -> Result<Vec<LettaBotRow>, CustomError> {
    sqlx::query_as::<_, LettaBotRow>(
        r#"
        SELECT u.username, b.slug AS bear_slug, b.letta_agent_id
        FROM user_bear ub
        INNER JOIN users u ON u.id = ub.user_id
        INNER JOIN bears b ON b.id = ub.bear_id
        ORDER BY u.username, b.slug
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}
