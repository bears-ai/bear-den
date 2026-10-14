//! Cabinet-only process composition. This path deliberately never enters Den's
//! runtime startup: Postgres, shared identity, assets, and Cabinet HTTP only.

use std::{io, net::SocketAddr, sync::Arc};

use axum_login::tower_sessions::{session::Id, SessionStore};
use sqlx::migrate::{AppliedMigration, Migrate, Migrator};
use tower_sessions_sqlx_store::PostgresStore;

use crate::{config::Config, startup::StartupError};

pub fn validate_cabinet_config(config: &Config) -> Result<(), StartupError> {
    if config.database_url.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "DATABASE_URL is required for cabinet-serve",
        )
        .into());
    }
    let origin = config.cabinet_web_origin.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CABINET_URL is required for cabinet-serve; WEB_SERVER_URL must remain the Den origin",
        )
    })?;
    crate::config::cabinet::validate_origin(origin).map_err(io::Error::other)?;
    Ok(())
}

/// Read-only schema admission. The existing `den migrate` job owns all DDL.
/// Reuse Den's forward-version guard, then reject pending/checksum-drifted or
/// dirty migrations instead of silently accepting an unprepared shared database.
pub async fn ensure_cabinet_schema_ready(pool: &sqlx::PgPool) -> Result<(), StartupError> {
    crate::startup::ensure_database_schema_supported(pool).await?;
    let mut connection = pool.acquire().await?;
    if let Some(version) = connection.dirty_version().await? {
        return Err(sqlx::migrate::MigrateError::Dirty(version).into());
    }
    let applied = connection.list_applied_migrations().await?;
    validate_cabinet_migrations(&sqlx::migrate!(), &applied)?;
    drop(connection);
    verify_shared_session_schema(pool).await?;
    Ok(())
}

fn validate_cabinet_migrations(
    migrator: &Migrator,
    applied: &[AppliedMigration],
) -> Result<(), StartupError> {
    for applied in applied {
        if !migrator.iter().any(|migration| {
            migration.migration_type.is_up_migration() && migration.version == applied.version
        }) {
            return Err(sqlx::migrate::MigrateError::VersionMissing(applied.version).into());
        }
    }
    for migration in migrator
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
    {
        let Some(applied) = applied
            .iter()
            .find(|applied| applied.version == migration.version)
        else {
            return Err(io::Error::other(format!("Cabinet requires unapplied migration {}; run the existing den migrate job before cabinet-serve", migration.version)).into());
        };
        if applied.checksum != migration.checksum {
            return Err(sqlx::migrate::MigrateError::VersionMismatch(migration.version).into());
        }
    }
    Ok(())
}

pub(crate) async fn verify_shared_session_schema(
    pool: &sqlx::PgPool,
) -> Result<PostgresStore, StartupError> {
    let session_store = PostgresStore::new(pool.clone());
    // A read verifies shared session schema exists, without calling migrate().
    session_store.load(&Id::default()).await.map_err(|error| {
        StartupError::SessionStore(format!(
            "Shared browser sessions require the session schema from den migrate: {error}"
        ))
    })?;
    Ok(session_store)
}

pub async fn cabinet_serve() -> Result<(), StartupError> {
    crate::init_tracing()?;
    let config = Arc::new(Config::try_load().map_err(io::Error::other)?);
    validate_cabinet_config(&config)?;
    let pool = crate::connect_database(&config).await?;
    ensure_cabinet_schema_ready(&pool).await?;

    let session_store = PostgresStore::new(pool.clone());
    let app = crate::web::surface::cabinet_server(pool, session_store, config.clone())
        .await
        .map_err(|error| io::Error::other(error.to_string()))?;
    let address = SocketAddr::from(([0, 0, 0, 0], config.web_port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, origin = %config.surface_public_origin(crate::config::WebSurface::Cabinet), "Cabinet-only service started (no native runtime)");
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(crate::shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "cabinet_startup/tests.rs"]
mod tests;
