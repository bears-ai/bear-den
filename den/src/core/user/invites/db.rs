use serde::Serialize;
use sqlx::{PgPool, Row};

use crate::errors::CustomError;

#[derive(Serialize, sqlx::FromRow)]
pub struct Invite {
    pub id: i32,
    pub code: String,
    pub new_user_id: Option<i32>,
    pub new_username: Option<String>,
    pub new_display_name: Option<String>,
}

pub struct InvitingUser {
    pub inviting_username: Option<String>,
    pub inviting_display_name: Option<String>,
}

pub async fn by_user_id(db_pool: &PgPool, user_id: i32) -> Result<Vec<Invite>, CustomError> {
    let invites = sqlx::query_as::<_, Invite>(
        r#"
        SELECT
            invites.id,
            invites.code,
            invites.new_user_id,
            users.username AS new_username,
            users.display_name AS new_display_name
        FROM invites
        LEFT JOIN users ON invites.new_user_id = users.id
        WHERE invites.user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_all(db_pool)
    .await?;
    Ok(invites)
}

pub async fn create(db_pool: &PgPool, user_id: i32, code: &str) -> Result<i32, CustomError> {
    let row = sqlx::query(
        "INSERT INTO invites (user_id, code, created_at, updated_at) VALUES ($1, $2, NOW(), NOW()) RETURNING id",
    )
    .bind(user_id)
    .bind(code)
    .fetch_one(db_pool)
    .await?;
    let id: i32 = row.try_get("id")?;
    Ok(id)
}

pub async fn check(db_pool: &PgPool, code: &str) -> Result<Option<InvitingUser>, CustomError> {
    let row = sqlx::query(
        r#"
        SELECT
            users.username AS inviting_username,
            users.display_name AS inviting_display_name
        FROM invites
        LEFT JOIN users ON invites.user_id = users.id
        WHERE invites.code = $1 AND invites.new_user_id IS NULL
        "#,
    )
    .bind(code)
    .fetch_optional(db_pool)
    .await?;

    Ok(row.map(|r| InvitingUser {
        inviting_username: r.try_get("inviting_username").ok(),
        inviting_display_name: r.try_get("inviting_display_name").ok(),
    }))
}

pub async fn consume(db_pool: &PgPool, code: &str, new_user_id: i32) -> Result<(), CustomError> {
    sqlx::query("UPDATE invites SET new_user_id = $1, updated_at = NOW() WHERE code = $2")
        .bind(new_user_id)
        .bind(code)
        .execute(db_pool)
        .await?;
    Ok(())
}
