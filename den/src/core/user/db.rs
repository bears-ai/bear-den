use sqlx::{PgPool, query, query_as};

use crate::errors::CustomError;

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct User {
    pub id: i32,
    pub email: String,
    pub username: String,
    pub display_name: String,
    pub passhash: String,
    pub admin_flag: bool,
    pub theme: String,
}

/// For authentication purposes
pub struct UserAuth {
    pub id: i32,
    pub username: String,
    pub passhash: String,
    pub admin_flag: bool,
    pub theme: String,
}

pub async fn get_users(db_pool: &PgPool) -> Result<Vec<User>, CustomError> {
    let users = query_as!(
        User,
        r#"SELECT id, email, username, display_name, passhash, admin_flag as "admin_flag!", theme FROM users"#
    )
    .fetch_all(db_pool)
    .await?;
    Ok(users)
}

pub async fn create_user(
    db_pool: &PgPool,
    email: &str,
    username: &str,
    display_name: &str,
    passhash: &str,
) -> Result<i32, CustomError> {
    let record = sqlx::query!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
        email,
        username,
        display_name,
        passhash
    )
    .fetch_one(db_pool)
    .await?;
    Ok(record.id)
}

pub async fn get_user_by_id(db_pool: &PgPool, id: i32) -> Result<Option<User>, CustomError> {
    let user = query_as!(
        User,
        r#"SELECT id, email, username, display_name, passhash, admin_flag as "admin_flag!", theme FROM users WHERE id = $1"#,
        id
    )
    .fetch_optional(db_pool)
    .await?;
    Ok(user)
}

pub async fn get_username_by_id(db_pool: &PgPool, id: i32) -> Result<Option<String>, CustomError> {
    let result = query!("SELECT username FROM users WHERE id = $1", id)
        .fetch_optional(db_pool)
        .await?;
    Ok(result.map(|r| r.username))
}

pub async fn count_users_by_username(db_pool: &PgPool, username: &str) -> Result<i64, CustomError> {
    let record = sqlx::query!("SELECT COUNT(*) FROM users WHERE username = $1", username)
        .fetch_one(db_pool)
        .await?;
    Ok(record.count.unwrap_or(0))
}

pub async fn get_user_by_username(
    db_pool: &PgPool,
    username: &str,
) -> Result<Option<UserAuth>, CustomError> {
    let user = query_as!(
        UserAuth,
        r#"SELECT id, username, passhash, admin_flag as "admin_flag!", theme FROM users WHERE username = $1"#,
        username
    )
    .fetch_optional(db_pool)
    .await?;
    Ok(user)
}

pub async fn get_user_by_email(db_pool: &PgPool, email: &str) -> Result<Option<User>, CustomError> {
    let user = query_as!(
        User,
        r#"SELECT id, email, username, display_name, passhash, admin_flag as "admin_flag!", theme FROM users WHERE email = $1"#,
        email
    )
    .fetch_optional(db_pool)
    .await?;
    Ok(user)
}

pub async fn get_user_auth_by_email(
    db_pool: &PgPool,
    email: &str,
) -> Result<Option<UserAuth>, CustomError> {
    let user = query_as!(
        UserAuth,
        r#"SELECT id, username, passhash, admin_flag as "admin_flag!", theme FROM users WHERE email = $1"#,
        email
    )
    .fetch_optional(db_pool)
    .await?;
    Ok(user)
}

pub async fn update_user_by_id(
    db_pool: &PgPool,
    id: i32,
    email: &str,
    username: &str,
    display_name: &str,
    theme: &str,
    week_start_day: i32,
) -> Result<(), CustomError> {
    query!(
        "UPDATE users SET email = $1, username = $2, display_name = $3, theme = $4, week_start_day = $5, updated_at = NOW() WHERE id = $6",
        email,
        username,
        display_name,
        theme,
        week_start_day,
        id
    )
    .execute(db_pool)
    .await?;
    Ok(())
}

pub async fn set_user_passhash_by_id(
    db_pool: &PgPool,
    id: i32,
    passhash: &str,
) -> Result<(), CustomError> {
    query!(
        "UPDATE users SET passhash = $1, updated_at = NOW() WHERE id = $2",
        passhash,
        id
    )
    .execute(db_pool)
    .await?;
    Ok(())
}

pub async fn delete_user_by_id(db_pool: &PgPool, id: i32) -> Result<(), CustomError> {
    query!("DELETE FROM users WHERE id = $1", id)
        .execute(db_pool)
        .await?;
    Ok(())
}

pub async fn user_by_id(db_pool: &PgPool, id: i32) -> Result<Option<super::User>, CustomError> {
    let user = sqlx::query_as!(
        super::User,
        r#"
        SELECT
            users.id AS id,
            username,
            display_name,
            email,
            (email_configs.verified_at IS NOT NULL) AS email_verified,
            theme,
            week_start_day,
            users.created_at AS created
        FROM users
        LEFT JOIN email_configs
            ON users.id = email_configs.user_id
            AND email_configs.active = true
        WHERE
            users.id = $1
        "#,
        id
    )
    .fetch_optional(db_pool)
    .await?;

    Ok(user)
}

pub async fn settings_by_id(
    db_pool: &PgPool,
    id: i32,
) -> Result<Option<super::UserSettings>, CustomError> {
    let settings = sqlx::query_as!(
        super::UserSettings,
        r#"
        SELECT id, display_name, theme, week_start_day
        FROM users
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(db_pool)
    .await?;

    Ok(settings)
}

pub async fn update_settings(
    db_pool: &PgPool,
    user_settings: &super::UserSettings,
) -> Result<(), CustomError> {
    sqlx::query!(
        r#"
        UPDATE users
        SET
            display_name = $2,
            theme = $3,
            week_start_day = $4
        WHERE
            id = $1
        "#,
        user_settings.id,
        user_settings.display_name,
        user_settings.theme,
        user_settings.week_start_day,
    )
    .execute(db_pool)
    .await?;

    Ok(())
}
