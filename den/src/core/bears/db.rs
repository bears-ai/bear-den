//! SQL for bears, templates, and `user_bear` (runtime `query_as` — see `model.rs`).

use sqlx::{PgPool, types::Json};
use uuid::Uuid;

use crate::errors::CustomError;

use super::model::{Bear, BearTemplate};

pub async fn list_templates(pool: &PgPool) -> Result<Vec<BearTemplate>, CustomError> {
    sqlx::query_as::<_, BearTemplate>(
        r#"
        SELECT id, slug, name, description, system_prompt, default_model, tools_enabled,
               created_at, updated_at
        FROM bear_templates
        ORDER BY slug
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn get_template(pool: &PgPool, id: Uuid) -> Result<Option<BearTemplate>, CustomError> {
    sqlx::query_as::<_, BearTemplate>(
        r#"
        SELECT id, slug, name, description, system_prompt, default_model, tools_enabled,
               created_at, updated_at
        FROM bear_templates
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn create_template(
    pool: &PgPool,
    slug: &str,
    name: &str,
    description: &str,
    system_prompt: &str,
    default_model: Option<&str>,
    tools_enabled: Option<Json<serde_json::Value>>,
) -> Result<Uuid, CustomError> {
    let row: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO bear_templates (slug, name, description, system_prompt, default_model, tools_enabled)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(slug)
    .bind(name)
    .bind(description)
    .bind(system_prompt)
    .bind(default_model)
    .bind(tools_enabled)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn list_bears(pool: &PgPool) -> Result<Vec<Bear>, CustomError> {
    sqlx::query_as::<_, Bear>(
        r#"
        SELECT id, slug, name, description, letta_agent_id, default_model, tools_enabled,
               system_prompt, source_template_id, created_at, updated_at
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
               system_prompt, source_template_id, created_at, updated_at
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

pub async fn template_slug_exists(pool: &PgPool, slug: &str) -> Result<bool, CustomError> {
    let n: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM bear_templates WHERE slug = $1",
    )
    .bind(slug)
    .fetch_one(pool)
    .await?;
    Ok(n.0 > 0)
}

/// Copies prompt/model/tools from the template; `letta_agent_id` stays unset until Letta provisions.
pub async fn create_bear_from_template(
    pool: &PgPool,
    template_id: Uuid,
    slug: &str,
    name: &str,
    description: &str,
) -> Result<Uuid, CustomError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO bears (slug, name, description, system_prompt, default_model, tools_enabled,
                          letta_agent_id, source_template_id)
        SELECT $1, $2, $3, bt.system_prompt, bt.default_model, bt.tools_enabled, NULL, bt.id
        FROM bear_templates bt
        WHERE bt.id = $4
        RETURNING bears.id
        "#,
    )
    .bind(slug)
    .bind(name)
    .bind(description)
    .bind(template_id)
    .fetch_optional(pool)
    .await?;

    row.map(|r| r.0)
        .ok_or_else(|| CustomError::NotFound("template not found".to_string()))
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
        ON CONFLICT (user_id, bear_id) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(bear_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
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
