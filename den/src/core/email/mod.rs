use std::sync::OnceLock;

use mailgun_rs::{Attachment, EmailAddress, Mailgun, MailgunRegion, Message};
use minijinja::Environment;
use sqlx::{PgPool, query, query_as};

use crate::{config::Config, errors::CustomError};

static MAIL_FROM_NAME: &str = "Application";
static MAIL_FROM_ADDRESS: &str = "noreply@newapp.example";

pub struct EmailConfig {
    pub user_id: i32,
    pub email_config_id: i32,
    pub email_address: String,
    pub username: String,
    pub display_name: String,
    pub week_start_day: i32, // 0 = Sunday, 1 = Monday, etc.
}
pub async fn get_current_config(
    sqlx_pool: &PgPool,
    user_id: i32,
) -> Result<EmailConfig, CustomError> {
    Ok(query_as!(
        EmailConfig,
        r#"
        SELECT
            users.id AS user_id,
            email_configs.id AS email_config_id,
            email_configs.email_address AS email_address,
            users.username,
            users.display_name,
            users.week_start_day
        FROM email_configs
            JOIN users ON email_configs.user_id = users.id
                AND email_configs.active = true
        WHERE user_id = $1
        LIMIT 1
        "#,
        user_id
    )
    .fetch_one(sqlx_pool)
    .await?)
}

static MAILGUN: OnceLock<Mailgun> = OnceLock::new();

/// Initialize the process-wide Mailgun client from startup [`Config`].
///
/// Call exactly once from application entry after [`Config::load`].
pub fn init_mailgun(config: &Config) {
    let _ = MAILGUN.set(Mailgun {
        api_key: config.mailgun_api_key.clone(),
        domain: config.mailgun_domain.clone(),
    });
}

pub fn mailgun_client() -> &'static Mailgun {
    MAILGUN.get().expect(
        "Mailgun client not initialized: call core::email::init_mailgun from application startup",
    )
}

pub async fn send_email_template(
    sqlx_pool: &PgPool,
    app_config: &Config,
    config: EmailConfig,
    subject: String,
    template_env: Environment<'static>,
    template_name: &str,
    ctx: minijinja::Value,
    attachments: Option<Vec<Attachment>>,
) -> Result<(), CustomError> {
    let recipient = EmailAddress::name_address(&config.display_name, &config.email_address);
    tracing::debug!("Sending email to {} with subject '{}'", recipient, subject);

    let type_name = template_name
        .split_once('.')
        .map_or(template_name, |(name, _)| name);

    let message_record = query!(
        r#"
        INSERT INTO email_messages
            (email_config_id, message_type, parameters)
        VALUES
            ($1, $2, $3)
        RETURNING id
        "#,
        config.email_config_id,
        type_name,
        serde_json::to_value(ctx.clone())?,
    )
    .fetch_one(sqlx_pool)
    .await?;
    let message_record_id = message_record.id;

    let base_url = format!(
        "{}email/{}/",
        app_config.telemetry_url_prefix, message_record_id
    );

    let merged_ctx = minijinja::context! {
        base_url => base_url,
        username => config.username,
        display_name => config.display_name,
        ..ctx.clone()
    };

    // prepare message body
    let template = template_env.get_template(template_name).map_err(|e| {
        CustomError::Render(format!("Unable to find template '{template_name}': {e:?}"))
    })?;
    let html = match template.render(merged_ctx) {
        Ok(rendered) => rendered,
        Err(e) => {
            // log all the gory details
            tracing::error!("Error rendering template: {:#}", e);
            let mut current_err = &e as &dyn std::error::Error;
            while let Some(next_err) = current_err.source() {
                tracing::error!("Causal error: {:#}", next_err);
                current_err = next_err;
            }
            return Err(CustomError::Render(format!(
                "Error rendering template '{template_name}'"
            )));
        }
    };

    let message = Message {
        to: vec![recipient],
        subject,
        html,
        ..Default::default()
    };
    let sender = EmailAddress::name_address(MAIL_FROM_NAME, MAIL_FROM_ADDRESS);

    match mailgun_client()
        .async_send(MailgunRegion::EU, &sender, message, attachments)
        .await
    {
        Ok(response) => {
            tracing::info!(
                "Email sent successfully ({}): {}",
                response.id,
                response.message
            );
            query!(
                r#"
                UPDATE email_messages
                SET
                    message_id = $2,
                    response_message = $3,
                    sent_at = NOW()
                WHERE id = $1
                "#,
                message_record_id,
                response.id,
                response.message,
            )
            .execute(sqlx_pool)
            .await?;

            Ok(())
        }
        Err(send_err) => {
            tracing::error!("Failed to send email: {}", send_err);
            query!(
                r#"
                UPDATE email_messages
                SET
                    response_message = $2,
                    failed_at = NOW()
                WHERE id = $1
                "#,
                message_record_id,
                send_err.to_string(),
            )
            .execute(sqlx_pool)
            .await?;

            Err(CustomError::Email(send_err.to_string()))
        }
    }
}
