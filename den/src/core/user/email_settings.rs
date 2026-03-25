use minijinja::context;
use sqlx::{PgPool, query, query_as};

use crate::{config::Config, core::email, errors::CustomError};

pub struct UserEmailBasics {
    pub user_id: i32,
    pub email: String,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct UserEmailSettings {
    pub user_id: i32,
    pub email: String,
    #[allow(dead_code)]
    pub active: bool,

    pub display_name: String,
    pub email_config_id: i32,
    pub verified_at: Option<time::OffsetDateTime>,
}
pub struct VerifyEmailParams {
    pub user_id: i32,
    pub email: String,
    // pub display_name: String,
    pub email_config_id: i32,
    pub verified_at: Option<time::OffsetDateTime>,
}
impl From<UserEmailSettings> for VerifyEmailParams {
    fn from(user_email_settings: UserEmailSettings) -> Self {
        VerifyEmailParams {
            user_id: user_email_settings.user_id,
            email: user_email_settings.email,
            // display_name: user_email_settings.display_name,
            email_config_id: user_email_settings.email_config_id,
            verified_at: user_email_settings.verified_at,
        }
    }
}

/*
 Why, you might wonder, is email configuration in a separete table, when everything
 here is designed to use only one record per user?

 The primary answer is to keep a record of past address usage, for correlation with
 the sending service.
*/

pub async fn settings_by_id(
    db_pool: &PgPool,
    user_id: i32,
) -> Result<UserEmailSettings, CustomError> {
    if let Some(email_settings) = query_as!(
        UserEmailSettings,
        "
        SELECT
            users.id AS user_id,
            display_name,
            users.email,
            email_configs.id AS email_config_id,
            email_configs.verified_at,
            email_configs.active
        FROM users
            INNER JOIN email_configs
            ON users.id = email_configs.user_id
            AND users.email = email_configs.email_address
        WHERE
            users.id = $1
        ",
        user_id
    )
    .fetch_optional(db_pool)
    .await?
    {
        Ok(email_settings)
    } else if let Some(user_email) = query!(
        "
        SELECT email
        FROM users
        WHERE id = $1
        ",
        user_id
    )
    .fetch_optional(db_pool)
    .await?
    {
        // this will happen upon registration, between user creation and email verification
        tracing::info!(
            "No email settings for current address found for user {} (will create)",
            user_id
        );

        let current_address = user_email.email;
        let email_settings = query_as!( UserEmailSettings,
            r#"
            INSERT INTO email_configs (user_id, email_address, active)
            VALUES ($1, $2, $3)
            RETURNING
                user_id,
                COALESCE((SELECT display_name FROM users WHERE id = $1), 'Anonymous') AS "display_name!",
                email_address AS email,
                email_configs.id AS email_config_id,
                email_configs.verified_at,
                active
            "#,
            user_id,
            current_address,
            true
        ).fetch_one(db_pool).await?;

        // send_verify_email(db_pool, VerifyEmailParams::from(email_settings.clone())).await?;

        Ok(email_settings)
    } else {
        Err(CustomError::Database(format!(
            "Cannot find a current email address for user {user_id}"
        )))
    }
}

// after this is successful, a verification should be sent to the new email address
// n.b. the verified flag will be ignored
pub async fn update_email_basics(
    db_pool: &PgPool,
    email_basics: UserEmailBasics,
) -> Result<(), CustomError> {
    let user_id = email_basics.user_id;
    let new_email = email_basics.email.to_lowercase();
    let active_flag = email_basics.active;

    let mut tx = db_pool.begin().await?;

    // check if email address is new
    let current_email = query!(
        "
        SELECT email
        FROM users
        WHERE id = $1
        ",
        user_id
    )
    .fetch_one(&mut *tx)
    .await?
    .email;
    if current_email == new_email {
        // only the active flag is relevant, then
        query!(
            "
            UPDATE email_configs
            SET
                active = $3,
                updated_at = now()
            WHERE id IN (
                SELECT id
                FROM email_configs
                WHERE
                    user_id = $1
                AND email_address = $2
            )
            ",
            user_id,
            new_email,
            active_flag
        )
        .execute(&mut *tx)
        .await?;

        return Ok(());
    }

    // check if email address is already in use
    let email_exists = query!(
        "
        SELECT id
        FROM email_configs
        WHERE
            email_address = $1
        AND user_id != $2
        ",
        new_email,
        user_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if email_exists.is_some() {
        return Err(CustomError::Database(format!(
            "Email address {new_email} is already in use by another user"
        )));
    }

    // time to change email address
    query!(
        "
        UPDATE users
        SET
            email = $1
        WHERE
            id = $2
        ",
        new_email,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    tracing::info!("Updated email address for auth user {}", user_id);

    // mark current configs as inactive
    let emails_deactivated = query!(
        "
        UPDATE email_configs
        SET active = false, updated_at = now()
        WHERE id IN (
            SELECT id
            FROM email_configs
            WHERE
                user_id = $1
            AND active = true
        )
        ",
        user_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tracing::info!(
        "Deactivated {} email configs for user {}",
        emails_deactivated,
        user_id
    );

    // create new email config
    query!(
        "
        INSERT INTO email_configs (user_id, email_address, active)
        VALUES ($1, $2, $3)
        ",
        user_id,
        new_email,
        active_flag
    )
    .execute(&mut *tx)
    .await?;
    tracing::info!("Created new email config for user {}", user_id);

    tx.commit().await?;
    Ok(())
}

pub enum VerifyAttemptStatus {
    Success,
    Redundant,
    Unknown,
    Expired,
}
pub struct VerifyOutcome {
    #[allow(dead_code)]
    pub email_config_id: Option<i32>,
    pub status: VerifyAttemptStatus,
}
pub async fn mark_email_verified(
    db_pool: &PgPool,
    user_id: i32,
    verify_code: String,
) -> Result<VerifyOutcome, CustomError> {
    if let Some(email_config) = query!(
        "
        SELECT id, verify_code_expire_at, verified_at
        FROM email_configs
        WHERE
            user_id = $1
        AND verify_code = $2
        ",
        user_id,
        verify_code
    )
    .fetch_optional(db_pool)
    .await?
    {
        if email_config.verified_at.is_some() {
            return Ok(VerifyOutcome {
                email_config_id: Some(email_config.id),
                status: VerifyAttemptStatus::Redundant,
            });
        }
        let expire_at = email_config.verify_code_expire_at;
        if expire_at < time::OffsetDateTime::now_utc() {
            return Ok(VerifyOutcome {
                email_config_id: Some(email_config.id),
                status: VerifyAttemptStatus::Expired,
            });
        }

        query!(
            "
            UPDATE email_configs
            SET
                verified_at = now(),
                active = true
            WHERE id = $1
            ",
            email_config.id
        )
        .execute(db_pool)
        .await?;
        Ok(VerifyOutcome {
            email_config_id: Some(email_config.id),
            status: VerifyAttemptStatus::Success,
        })
    } else {
        Ok(VerifyOutcome {
            email_config_id: None,
            status: VerifyAttemptStatus::Unknown,
        })
    }
}

fn generate_verify_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..32)
        .map(|_| rng.random_range(b'a'..=b'z') as char)
        .collect()
}

pub async fn send_verify_email_for_user_id(
    db_pool: &PgPool,
    user_id: i32,
    app_config: &Config,
) -> Result<String, CustomError> {
    let verify_email_params = VerifyEmailParams::from(settings_by_id(db_pool, user_id).await?);

    let email_address = verify_email_params.email;
    // let display_name = verify_email_params.display_name;

    if verify_email_params.verified_at.is_some() {
        tracing::warn!(
            "Email for user {} already verified, not sending a new request",
            verify_email_params.user_id
        );

        return Ok(email_address);
    }

    let verify_code = generate_verify_code();

    query!(
        "
        UPDATE email_configs
        SET
            verify_code = $1,
            verify_code_expire_at = now() + interval '1 hour'
        WHERE id = $2
        ",
        verify_code,
        verify_email_params.email_config_id
    )
    .execute(db_pool)
    .await?;

    let user_to_email = email::get_current_config(db_pool, verify_email_params.user_id).await?;
    let subject = "Please verify your email address".to_string();
    let verify_link = format!("{}{}", app_config.email_verify_url_prefix, verify_code);
    let context = context! {
        verify_link
    };

    // this is rare enough that we'll just init this as needed
    let mut minijinja_env = minijinja::Environment::new();
    // minijinja_contrib::add_to_environment(&mut minijinja_env); // needed for the 'datetimeformat' filter
    #[cfg(feature = "production")]
    minijinja_embed::load_templates!(&mut minijinja_env, "email");
    #[cfg(not(feature = "production"))]
    minijinja_env.set_loader(minijinja::path_loader("src/core/email/templates"));

    email::send_email_template(
        db_pool,
        app_config,
        user_to_email,
        subject,
        minijinja_env,
        "verify_email.html",
        context,
        None,
    )
    .await?;

    Ok(email_address)
}
