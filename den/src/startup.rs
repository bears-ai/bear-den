//! Startup validation, SQLx migration runner, and structured errors for [`crate::run`].

use crate::config::Config;
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

fn requires_jwt_secret(config: &Config) -> bool {
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
    Ok(())
}

#[cfg(all(test, not(feature = "production")))]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn validate_jwt_rules_when_not_production_build() {
        let prev = std::env::var("JWT_SECRET").ok();
        std::env::remove_var("JWT_SECRET");

        let base = Config::test_stub();
        validate_runtime_config(&base).expect("web-only must not require JWT_SECRET");

        let mut api_on = base.clone();
        api_on.run_api = true;
        assert!(
            validate_runtime_config(&api_on).is_err(),
            "RUN_API=true requires JWT_SECRET"
        );

        std::env::set_var(
            "JWT_SECRET",
            "test-jwt-secret-for-unit-tests-min-length-ok",
        );
        validate_runtime_config(&api_on).expect("RUN_API with JWT_SECRET should pass");

        match prev {
            Some(v) => std::env::set_var("JWT_SECRET", v),
            None => std::env::remove_var("JWT_SECRET"),
        }
    }
}
