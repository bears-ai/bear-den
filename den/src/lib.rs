//! Library surface for integration tests and embedding. The binary entrypoint is [`run`].
//!
//! Clippy: the tree still carries a large extracted domain. These allows keep `cargo clippy -- -D warnings`
//! workable in CI; remove them gradually as call sites are cleaned up.
#![allow(clippy::assertions_on_constants)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::result_large_err)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::too_many_arguments)]
pub mod api;
pub mod auth_backend;
pub mod config;
pub mod core;
pub mod errors;
pub mod web;

use crate::config::Config;
use tokio::{signal, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::core::email;
use axum_login::tower_sessions::session_store::ExpiredDeletion;
use sqlx::postgres::PgPoolOptions;
use tower_sessions_sqlx_store::PostgresStore;

use std::sync::Arc;
use std::{io::Error, net::SocketAddr};

/// Run all enabled services until a shutdown signal (Ctrl+C, or SIGTERM on Unix).
pub async fn run() -> Result<(), Error> {
    let mut task_set = JoinSet::new();

    let tracing_filter: String;
    #[cfg(feature = "production")]
    {
        tracing_filter = "newapp=info,\
            newapp::web=info,\
            newapp::api=info,\
            tower_sessions=info,\
            tower_http=info,\
            axum=info,\
            axum_login=info"
            .to_string();
    }
    #[cfg(not(feature = "production"))]
    {
        tracing_filter = "newapp=info,\
            newapp::core=debug,\
            newapp::web=debug,\
            newapp::api=debug,\
            tower_sessions=info,\
            tower_http=info,\
            axum=info,\
            axum_login=info"
            .to_string();
    }

    tracing_subscriber::registry()
        .with(EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or(tracing_filter),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .unwrap();

    let config = Arc::new(Config::load());
    email::init_mailgun(config.as_ref());

    let mut services = Vec::new();
    if config.run_web {
        services.push("web");
        tracing::info!("Web server will start on port {}", config.web_port);
    }
    if config.run_api {
        services.push("api");
        tracing::info!("API server will start on port {}", config.api_port);
    }
    if config.run_workers {
        services.push("workers");
        tracing::info!("Background workers slot enabled (no domain workers in this slim starter)");
    }
    if services.is_empty() {
        tracing::warn!("No services enabled! Set RUN_WEB, RUN_API, or RUN_WORKERS to true.");
    } else {
        tracing::info!(
            "Starting application (`newapp`) with services: {}",
            services.join(", ")
        );
    }

    let sqlx_pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(&config.database_url)
        .await
        .expect("can't connect to database");

    let session_store = PostgresStore::new(sqlx_pool.clone());
    session_store.migrate().await.unwrap();

    let deletion_task = tokio::task::spawn(
        session_store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
    );
    let deletion_task_abort_handle = deletion_task.abort_handle();

    if config.run_web {
        let web_addr = SocketAddr::from(([0, 0, 0, 0], config.web_port));
        tracing::info!("Starting web server on http://{}", web_addr);

        let web_listener = tokio::net::TcpListener::bind(web_addr).await.map_err(|e| {
            tracing::error!(
                "Failed to bind web server to port {}: {}",
                config.web_port,
                e
            );
            e
        })?;

        let config_web = config.clone();
        let web_app = web::server_with_state(sqlx_pool.clone(), session_store.clone(), config_web)
            .await
            .map_err(|e| {
                tracing::error!("Failed to create web application: {}", e);
                std::io::Error::other(e.to_string())
            })?;

        task_set.spawn(async move {
            tracing::info!("Web service started successfully");
            axum::serve(web_listener, web_app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(std::io::Error::other)
        });
    }

    if config.run_api {
        let api_addr = SocketAddr::from(([0, 0, 0, 0], config.api_port));
        tracing::info!("Starting API server on http://{}", api_addr);

        let api_listener = tokio::net::TcpListener::bind(api_addr).await.map_err(|e| {
            tracing::error!(
                "Failed to bind API server to port {}: {}",
                config.api_port,
                e
            );
            e
        })?;

        let config_api = config.clone();
        let api_app = api::create_api_app(sqlx_pool.clone(), session_store.clone(), config_api)
            .await
            .map_err(|e| {
                tracing::error!("Failed to create API application: {}", e);
                std::io::Error::other(e.to_string())
            })?;

        task_set.spawn(async move {
            tracing::info!("API service started successfully");
            axum::serve(api_listener, api_app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(std::io::Error::other)
        });
    }

    task_set.spawn(async move {
        deletion_task
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)
    });

    let worker_token_opt = if config.run_workers {
        Some(CancellationToken::new())
    } else {
        None
    };

    if let Some(token) = worker_token_opt.clone() {
        let t = token.clone();
        task_set.spawn(async move {
            tracing::info!(
                "Workers: idle until shutdown (this slim starter has no import/report jobs)"
            );
            t.cancelled().await;
            Ok(())
        });
    } else {
        tracing::info!("Workers disabled (RUN_WORKERS=false or not set)");
    }

    tracing::info!("All services started successfully. Waiting for shutdown signal...");

    shutdown_signal().await;

    tracing::info!("Shutdown signal received. Stopping services...");

    deletion_task_abort_handle.abort();

    if let Some(token) = worker_token_opt {
        token.cancel();
    }

    while let Some(result) = task_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("Task completed with error: {}", e),
            Err(e) => tracing::warn!("Task cancelled or panicked: {}", e),
        }
    }

    tracing::info!("Shutdown complete.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        tracing::info!("Ctrl+C received");
    };

    #[cfg(unix)]
    {
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
            tracing::info!("SIGTERM received");
        };

        tokio::select! {
            _ = ctrl_c => {
                tracing::info!("Initiating graceful shutdown due to Ctrl+C");
            },
            _ = terminate => {
                tracing::info!("Initiating graceful shutdown due to SIGTERM");
            },
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
        tracing::info!("Initiating graceful shutdown (Ctrl+C)");
    }
}
