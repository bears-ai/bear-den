//! Startup validation, SQLx migration runner, and structured errors for [`crate::run`].

use crate::config::Config;
use crate::core::codepool::CodePoolClient;
use crate::core::letta::LettaClient;
use crate::core::runtime_provider::{
    acp_requires_runtime, RuntimeHealthCheck, RuntimeStartupCapabilities,
};
use sqlx::PgPool;
use thiserror::Error;

/// Failures while initializing the process (config, database, migrations, tracing).
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// Tower-sessions Postgres schema migration (error type varies by store version).
    #[error("tower-sessions store migration: {0}")]
    SessionStore(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("tracing subscriber: {0}")]
    Tracing(String),
    /// Database connection failed with operator-actionable context.
    #[error("database error: {message} (url={db_url})\n  hint: {hint}")]
    Database {
        message: String,
        db_url: String,
        hint: String,
    },
}

/// `true` when SQLx should ignore migration files present in `_sqlx_migrations` but missing
/// from the embedded migrator (recovery / legacy DBs only). Default **false** — set
/// `SQLX_MIGRATE_IGNORE_MISSING=true` only when you understand the risk.
pub fn sqlx_migrate_ignore_missing_from_env() -> bool {
    std::env::var("SQLX_MIGRATE_IGNORE_MISSING")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Run embedded SQLx migrations from `migrations/` against `pool`.
pub async fn run_sqlx_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    if sqlx_migrate_ignore_missing_from_env() {
        sqlx::migrate!().set_ignore_missing(true).run(pool).await
    } else {
        sqlx::migrate!().run(pool).await
    }
}

/// Whether [`validate_runtime_config`] requires a non-empty `JWT_SECRET` (production builds or `RUN_API`).
pub fn requires_jwt_secret(config: &Config) -> bool {
    #[cfg(feature = "production")]
    {
        let _ = config;
        true
    }
    #[cfg(not(feature = "production"))]
    {
        config.run_api
    }
}

fn allow_standalone_web_from_env() -> bool {
    std::env::var("DEN_ALLOW_STANDALONE_WEB")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Validate secrets and other invariants before connecting to the database.
pub fn validate_runtime_config(config: &Config) -> Result<(), StartupError> {
    if requires_jwt_secret(config) {
        let secret = std::env::var("JWT_SECRET").unwrap_or_default();
        if secret.trim().is_empty() {
            return Err(StartupError::Message(
                "JWT_SECRET must be set to a non-empty value when the binary is built with \
                 `--features production`, or when RUN_API=true (OAuth access tokens use HS256)."
                    .into(),
            ));
        }
    }
    if config.acp_gateway_enabled && !config.run_api {
        return Err(StartupError::Message(
            "ACP_GATEWAY_ENABLED=true requires RUN_API=true because ACP is exposed only on the API listener."
                .into(),
        ));
    }
    if acp_requires_runtime(config) && config.letta_base_url.trim().is_empty() {
        return Err(StartupError::Message(
            "LETTA_BASE_URL must be set when ACP_GATEWAY_ENABLED=true. Den routes ACP prompts directly to the pair role through the Letta API."
                .into(),
        ));
    }
    if config.run_web
        && config.codepool_base_url.trim().is_empty()
        && !allow_standalone_web_from_env()
    {
        return Err(StartupError::Message(
            "CODEPOOL_BASE_URL must be set when RUN_WEB=true. Den streams bear chat through \
             Codepool (Letta Code SDK), not directly to the Letta HTTP API. Set \
             DEN_ALLOW_STANDALONE_WEB=true only for local UI/dev runs without the rest of the stack. \
             Example internal URL: http://bears-codepool:3030 — see services/codepool/COOLIFY_DEPLOY.md."
                .into(),
        ));
    }
    Ok(())
}

/// Verify configured upstream HTTP services respond before accepting traffic.
///
/// Checks **Codepool** (`GET /health`) when [`Config::codepool_base_url`] is non-empty,
/// and **Letta** (`GET /v1/health`) when [`Config::letta_base_url`] is non-empty (same auth as runtime).
pub async fn validate_upstream_connections(config: &Config) -> Result<(), StartupError> {
    if !config.codepool_base_url.trim().is_empty() {
        tracing::info!(
            url = %config.codepool_base_url,
            "Checking Codepool connectivity"
        );
        CodePoolClient::new(config)
            .check_health()
            .await
            .map_err(|e| StartupError::Message(e.to_string()))?;
        tracing::info!("Codepool health check passed");
    }

    let runtime_capabilities = RuntimeStartupCapabilities::from_config(config);
    if runtime_capabilities.runtime_required_for_acp || !config.letta_base_url.trim().is_empty() {
        tracing::info!(
            url = %config.letta_base_url,
            compatibility_backend = "letta",
            acp_gateway_enabled = runtime_capabilities.acp_gateway_enabled,
            "Checking runtime compatibility backend connectivity"
        );
        let letta = LettaClient::new(config);
        let health = letta.compatibility_health_check();
        if health.enabled() {
            RuntimeHealthCheck::check_health(&health)
                .await
                .map_err(|e| StartupError::Message(e.to_string()))?;
            tracing::info!(
                compatibility_backend = RuntimeHealthCheck::compatibility_backend_name(&health),
                "Runtime compatibility backend health check passed"
            );
        } else if runtime_capabilities.runtime_required_for_acp {
            return Err(StartupError::Message(
                "LETTA_BASE_URL must be set when ACP_GATEWAY_ENABLED=true. Den routes ACP prompts directly to the pair role through the Letta API."
                    .into(),
            ));
        }
    }

    Ok(())
}

#[cfg(all(test, not(feature = "production")))]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn validate_jwt_rules_when_not_production_build() {
        let prev = std::env::var("JWT_SECRET").ok();
        // SAFETY: single-threaded test; no concurrent env reads in this process.
        unsafe {
            std::env::remove_var("JWT_SECRET");
        }

        let base = Config::test_stub();
        validate_runtime_config(&base).expect("web-only must not require JWT_SECRET");

        let mut api_on = base.clone();
        api_on.run_api = true;
        assert!(
            validate_runtime_config(&api_on).is_err(),
            "RUN_API=true requires JWT_SECRET"
        );

        unsafe {
            std::env::set_var("JWT_SECRET", "test-jwt-secret-for-unit-tests-min-length-ok");
        }
        validate_runtime_config(&api_on).expect("RUN_API with JWT_SECRET should pass");

        match prev {
            Some(v) => unsafe { std::env::set_var("JWT_SECRET", v) },
            None => unsafe { std::env::remove_var("JWT_SECRET") },
        }
    }

    #[test]
    fn validate_requires_api_and_letta_when_acp_enabled() {
        let prev = std::env::var("JWT_SECRET").ok();
        unsafe {
            std::env::set_var("JWT_SECRET", "test-jwt-secret-for-unit-tests-min-length-ok");
        }

        let mut acp_on = Config::test_stub();
        acp_on.acp_gateway_enabled = true;
        assert!(validate_runtime_config(&acp_on).is_err());

        acp_on.run_api = true;
        assert!(validate_runtime_config(&acp_on).is_err());

        acp_on.letta_base_url = "http://bears-letta:8283".into();
        validate_runtime_config(&acp_on).expect("ACP with API and Letta should pass");

        match prev {
            Some(v) => unsafe { std::env::set_var("JWT_SECRET", v) },
            None => unsafe { std::env::remove_var("JWT_SECRET") },
        }
    }

    #[test]
    fn validate_requires_codepool_when_run_web() {
        let mut web_on = Config::test_stub();
        web_on.run_web = true;
        web_on.codepool_base_url = String::new();
        assert!(
            validate_runtime_config(&web_on).is_err(),
            "RUN_WEB=true requires CODEPOOL_BASE_URL"
        );
        web_on.codepool_base_url = "http://localhost:3030".into();
        validate_runtime_config(&web_on).expect("RUN_WEB with Codepool should pass");
    }

    #[test]
    fn standalone_web_allows_missing_codepool() {
        let prev = std::env::var("DEN_ALLOW_STANDALONE_WEB").ok();
        unsafe {
            std::env::set_var("DEN_ALLOW_STANDALONE_WEB", "true");
        }

        let mut web_on = Config::test_stub();
        web_on.run_web = true;
        web_on.codepool_base_url = String::new();
        validate_runtime_config(&web_on).expect("standalone web skips Codepool requirement");

        match prev {
            Some(v) => unsafe { std::env::set_var("DEN_ALLOW_STANDALONE_WEB", v) },
            None => unsafe { std::env::remove_var("DEN_ALLOW_STANDALONE_WEB") },
        }
    }
}
