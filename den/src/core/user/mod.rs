pub mod db;
pub mod email_settings;
pub mod invites;

use serde::Serialize;
use sqlx::{PgPool, query_as};
use time::PrimitiveDateTime;

use crate::errors::CustomError;

pub const RESERVED_NAMES: [&str; 18] = [
    // 3-letters not needed because minimum length is 4
    // "api",
    // "cell",
    // "map",
    "account", "admin", "data", "email", "google", "history", "invalid", "lists", "login", "logout",
    "password", "profile", "register", "root", "search", "settings", "unknown", "users",
];
pub const UNKNOWN_USERNAME: &str = "unknown";
pub const UNKNOWN_USER_ID: i32 = 0;

// #[derive(sqlx::FromRow)]
#[derive(Serialize, Debug, Clone)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub email_verified: Option<bool>,
    pub theme: String,
    pub week_start_day: i32,
    pub created: PrimitiveDateTime,
    // pub premium_until: Option<PrimitiveDateTime>,
}

// #[derive(sqlx::FromRow)]
// pub struct UserAccount {
//     pub id: i32,
//     pub username: String,
//     pub passhash: String,
// }

// #[derive(sqlx::FromRow)]
pub struct UserSettings {
    pub id: i32,
    pub display_name: String,
    pub theme: String,
    pub week_start_day: i32,
}

pub async fn user_by_id(db_pool: &PgPool, id: i32) -> Result<User, CustomError> {
    let user = query_as!(
        User,
        "
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
        ",
        id
    )
    .fetch_one(db_pool)
    .await?;

    Ok(user)
}

pub async fn user_by_username_opt(
    db_pool: &PgPool,
    username: String,
) -> Result<Option<User>, CustomError> {
    let user_opt = query_as!(
        User,
        "
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
            users.username = $1
        ",
        username
    )
    .fetch_optional(db_pool)
    .await?;

    Ok(user_opt)
}
