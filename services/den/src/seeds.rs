//! Idempotent development/test data seeding.
//!
//! These fixtures are deliberately outside SQLx migrations: migrations own schema and
//! environment-agnostic bootstrap, while this module owns disposable dev/smoke data.

use anyhow::{anyhow, Context, Result};
use password_auth::generate_hash;
use sqlx::{postgres::PgPoolOptions, types::Json, PgPool};

use crate::{
    config::Config,
    core::{
        acp_tokens,
        bears::{
            db as bears_db, db::BearParams, db::BEAR_ROLE_ADMIN,
            provision::provision_missing_bear_roles, runtime_plan::default_runtime_plan,
        },
        bifrost::BifrostClient,
        letta::LettaClient,
        user::{self, db as user_db, email_settings},
    },
    startup::run_sqlx_migrations,
};

pub const SMOKE_USERNAME: &str = "alice";
pub const SMOKE_PASSWORD: &str = "Never deploy seed passwords.";
pub const SMOKE_BEAR_SLUG: &str = "test-bear";
pub const SMOKE_ACP_TOKEN: &str = "bears_acp_smoke_known_token_for_dev_and_ci_only_000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedProfile {
    Minimal,
    Smoke,
}

impl SeedProfile {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(Self::Minimal),
            "smoke" => Ok(Self::Smoke),
            other => Err(anyhow!(
                "unknown seed profile {other:?}; expected 'minimal' or 'smoke'"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Smoke => "smoke",
        }
    }
}

#[derive(Debug)]
pub struct SeedReport {
    pub profile: SeedProfile,
    pub user_id: i32,
    pub username: String,
    pub bear_id: uuid::Uuid,
    pub bear_slug: String,
}

pub async fn seed_database_url(database_url: &str, profile: SeedProfile) -> Result<SeedReport> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(database_url)
        .await
        .context("connect to DATABASE_URL for seeding")?;

    run_sqlx_migrations(&pool)
        .await
        .context("run SQLx migrations before seeding")?;

    seed_pool(&pool, profile).await
}

pub async fn seed_pool(pool: &PgPool, profile: SeedProfile) -> Result<SeedReport> {
    // `minimal` intentionally aliases `smoke` until there is a need for a richer split.
    match profile {
        SeedProfile::Minimal | SeedProfile::Smoke => seed_smoke(pool, profile).await,
    }
}

async fn seed_smoke(pool: &PgPool, profile: SeedProfile) -> Result<SeedReport> {
    let user_id = ensure_user(pool, SMOKE_USERNAME, SMOKE_PASSWORD)
        .await
        .context("ensure smoke user")?;
    email_settings::set_admin_email_verified(pool, user_id, true)
        .await
        .context("verify smoke user email")?;

    let bear_id = ensure_bear(pool, SMOKE_BEAR_SLUG)
        .await
        .context("ensure smoke bear")?;
    ensure_smoke_bear_model(pool, bear_id)
        .await
        .context("ensure smoke bear model")?;
    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan())
        .await
        .context("ensure smoke bear runtime_plan")?;
    bears_db::grant_membership(pool, user_id, bear_id, Some(BEAR_ROLE_ADMIN))
        .await
        .context("ensure smoke membership")?;
    ensure_smoke_acp_token(pool, user_id, bear_id)
        .await
        .context("ensure smoke ACP token")?;
    if let Err(err) = ensure_smoke_role_runtimes(pool, bear_id).await {
        tracing::warn!(error = %err, "smoke seed could not provision role runtimes; continuing with database fixtures only");
    }

    Ok(SeedReport {
        profile,
        user_id,
        username: SMOKE_USERNAME.to_string(),
        bear_id,
        bear_slug: SMOKE_BEAR_SLUG.to_string(),
    })
}

async fn ensure_user(pool: &PgPool, username: &str, password: &str) -> Result<i32> {
    let passhash = generate_hash(password);
    if let Some(existing) = user::user_by_username_opt(pool, username.to_string()).await? {
        user_db::set_user_passhash_by_id(pool, existing.id, &passhash).await?;
        return Ok(existing.id);
    }

    let email = format!("{username}@localhost");
    let display_name = "Alice Dev";
    user_db::create_user(pool, &email, username, display_name, &passhash)
        .await
        .map_err(Into::into)
}

async fn ensure_bear(pool: &PgPool, slug: &str) -> Result<uuid::Uuid> {
    if let Some(id) = bear_id_by_slug(pool, slug).await? {
        return Ok(id);
    }

    bears_db::create_bear(
        pool,
        BearParams {
            slug,
            name: "Test Bear",
            description: "Seeded bear for devcontainer smoke tests and manual UI checks.",
            system_prompt: "You are Test Bear, a concise assistant for local BEARS development and smoke testing.",
            default_model: None,
            tools_enabled: None::<Json<serde_json::Value>>,
            letta_agent_type: Some("letta_v1_agent"),
            letta_tool_ids: Json(Vec::new()),
            context_profile: None,
        },
    )
    .await
    .map_err(Into::into)
}

async fn ensure_smoke_bear_model(pool: &PgPool, bear_id: uuid::Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE bears
        SET default_model = COALESCE(NULLIF(default_model, ''), 'letta/letta-free')
        WHERE id = $1
        "#,
    )
    .bind(bear_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_smoke_role_runtimes(pool: &PgPool, bear_id: uuid::Uuid) -> Result<()> {
    let config = Config::load();
    let letta = LettaClient::new(&config);
    if !letta.is_enabled() {
        return Ok(());
    }
    let bifrost = BifrostClient::new(&config);
    provision_missing_bear_roles(pool, &letta, &bifrost, bear_id)
        .await
        .context("provision missing smoke bear role runtimes")?;
    Ok(())
}

async fn ensure_smoke_acp_token(pool: &PgPool, user_id: i32, bear_id: uuid::Uuid) -> Result<()> {
    let token_hash = acp_tokens::hash_raw_token_for_seed(SMOKE_ACP_TOKEN);
    let mut tx = pool.begin().await?;
    let row: (uuid::Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO acp_tokens (user_id, name, token_hash, scopes, revoked_at, expires_at)
        VALUES ($1, 'Smoke ACP token', $2, $3, NULL, NULL)
        ON CONFLICT (token_hash) DO UPDATE
        SET user_id = EXCLUDED.user_id,
            name = EXCLUDED.name,
            scopes = EXCLUDED.scopes,
            revoked_at = NULL,
            expires_at = NULL
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(token_hash)
    .bind(serde_json::json!([
        acp_tokens::acp_chat_scope(),
        acp_tokens::acp_tools_scope()
    ]))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO acp_token_bears (token_id, bear_id)
        VALUES ($1, $2)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(row.0)
    .bind(bear_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

async fn bear_id_by_slug(pool: &PgPool, slug: &str) -> Result<Option<uuid::Uuid>> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM bears WHERE slug = $1")
        .bind(slug)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_profiles() {
        assert_eq!(SeedProfile::parse("smoke").unwrap(), SeedProfile::Smoke);
        assert_eq!(SeedProfile::parse("minimal").unwrap(), SeedProfile::Minimal);
        assert!(SeedProfile::parse("demo").is_err());
    }
}
