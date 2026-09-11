//! Library surface for integration tests and embedding. The binary entrypoint is [`run`].
//!
//! Clippy: broad suppressions live on the largest legacy modules (for example `den_oauth::oauth`);
//! prefer fixing warnings locally and shrinking those module allows over time.
// The JSON/REST API edge moved to the `den-api` crate (v1.5 split). Re-exported as
// `crate::api` so the remaining binary call sites (run/web/seeds) resolve unchanged.
pub use den_api as api;
pub use den_core::config;
pub use den_http::auth_backend;
pub use den_http::build_info;
pub mod core;
pub mod internal_tools;
pub use den_http::errors;
pub mod import_legacy_memory;
pub mod reindex;
pub mod seeds;
pub mod startup;
// The web edge (server-rendered UI + /v1) moved to the `den-web` crate (v1.5
// split). Re-exported as `crate::web` so the binary's `run()` resolves unchanged.
pub use den_web as web;

use crate::config::Config;
use crate::startup::{
    ensure_database_schema_supported, run_sqlx_migrations, validate_runtime_config,
    validate_upstream_connections, StartupError,
};
use tokio::{signal, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::core::email;
use axum_login::tower_sessions::session_store::ExpiredDeletion;
use sqlx::postgres::PgPoolOptions;
use tower_sessions_sqlx_store::PostgresStore;

use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Debug)]
struct NativeWebChatRuntime {
    /// Clone of the process-wide [`den_memory::MemoryStoreManager`] (one instance
    /// per process; cloning shares the per-Bear pool map).
    stores: den_memory::MemoryStoreManager,
}

impl web::web_chat_runtime::WebChatRuntime for NativeWebChatRuntime {
    fn stream_chat(
        &self,
        state: &web::AppState,
        request: web::web_chat_runtime::WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<
        'static,
        Result<web::web_chat_runtime::WebChatRuntimeStream, web::errors::CustomError>,
    > {
        let pool = state.sqlx_pool().clone();
        let config = state.config.clone();
        let stores = self.stores.clone();
        Box::pin(async move {
            let deps = den_runtime::native_runtime::NativeRuntimeDeps {
                pool: &pool,
                config: config.as_ref(),
                stores: &stores,
            };
            let tool_invoker = den_runtime::native_runtime::tool_invoker().ok_or_else(|| {
                web::errors::CustomError::System(
                    "builtin Den tool runtime is not initialized".to_string(),
                )
            })?;
            let runtime_stream =
                den_runtime::native_runtime::start_native_web_chat_turn_event_stream(
                    den_runtime::native_runtime::NativeWebChatTurnParams {
                        deps: &deps,
                        bear_id: request.bear_id,
                        bear_slug: &request.bear_slug,
                        chat_binding_id: &request.chat_binding_id,
                        user_id: request.user_id,
                        username: request.username.as_deref(),
                        membership_role: request.membership_role.as_deref(),
                        conversation_id: &request.conversation_id,
                        session_id: &request.session_id,
                        prompt: &request.prompt,
                        request_id: request.request_id,
                        tool_invoker,
                    },
                )
                .await?;

            let stream = futures::StreamExt::map(runtime_stream, |item| {
                item.map_err(web::errors::CustomError::from)
            });
            Ok(Box::pin(stream) as web::web_chat_runtime::WebChatRuntimeStream)
        })
    }
}

fn native_web_chat_runtime(
    stores: den_memory::MemoryStoreManager,
) -> Arc<dyn web::web_chat_runtime::WebChatRuntime> {
    Arc::new(NativeWebChatRuntime { stores })
}

/// Run all enabled services until a shutdown signal (Ctrl+C, or SIGTERM on Unix).
pub async fn run() -> Result<(), StartupError> {
    run_server(false).await
}

pub async fn serve_without_migrations() -> Result<(), StartupError> {
    run_server(true).await
}

pub async fn migrate_only() -> Result<(), StartupError> {
    init_tracing()?;

    let build = crate::build_info::snapshot();
    tracing::info!(
        service = build.service,
        version = build.version,
        git_sha = build.git_sha,
        built_at_utc = build.built_at_utc,
        "Starting migration job",
    );

    let config = Arc::new(Config::load());
    validate_runtime_config(config.as_ref())?;
    let sqlx_pool = connect_database(config.as_ref()).await?;
    ensure_database_schema_supported(&sqlx_pool).await?;
    run_sqlx_migrations(&sqlx_pool).await?;
    tracing::info!(
        schema_version = crate::startup::embedded_schema_version(),
        "SQLx migrations completed successfully"
    );
    Ok(())
}

async fn run_server(skip_migrations: bool) -> Result<(), StartupError> {
    let mut task_set = JoinSet::new();

    init_tracing()?;

    let build = crate::build_info::snapshot();
    tracing::info!(
        service = build.service,
        version = build.version,
        git_sha = build.git_sha,
        built_at_utc = build.built_at_utc,
        "Starting build",
    );

    let config = Arc::new(Config::load());
    validate_runtime_config(config.as_ref())?;
    let needs_db = crate::startup::needs_database(config.as_ref());
    if needs_db {
        validate_upstream_connections(config.as_ref()).await?;
        email::init_mailgun(config.as_ref());
    }
    tracing::info!(
        app = %config.app_display_name,
        slug = %config.app_slug,
        web_url = %config.web_server_url,
        api_url = %config.api_server_url,
        "Loaded configuration",
    );

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
    if config.run_sandbox {
        services.push("sandbox");
        tracing::info!(
            "Sandbox provider will start on port {}",
            config.sandbox_port
        );
    }
    if services.is_empty() {
        tracing::warn!(
            "No services enabled! Set RUN_WEB, RUN_API, RUN_WORKERS, or RUN_SANDBOX to true."
        );
    } else {
        tracing::info!(
            "Starting application (`den`) with services: {}",
            services.join(", ")
        );
    }

    // Everything Postgres-backed (web, API, session store, workers) lives in
    // this branch. A standalone sandbox server (RUN_SANDBOX only) skips it all
    // — it must be able to run on a host with no database.
    let mut worker_token_opt: Option<CancellationToken> = None;
    let mut bearwire_expiry_token_opt: Option<CancellationToken> = None;
    let mut deletion_task_abort_handle = None;
    if needs_db {
        let sqlx_pool = connect_database(config.as_ref()).await?;
        // The one production `MemoryStoreManager` for this process (ADR-0031
        // write-topology amendment): every subsystem below receives a clone,
        // which shares the per-Bear pool map — one pool (and one SQLite write
        // connection) per Bear per process.
        let memory_stores = den_memory::MemoryStoreManager::new(config.as_ref());
        ensure_database_schema_supported(&sqlx_pool).await?;
        if skip_migrations {
            tracing::info!(
                schema_version = crate::startup::embedded_schema_version(),
                "Skipping startup SQLx migrations; assuming deploy-time migration job already succeeded"
            );
        } else {
            run_sqlx_migrations(&sqlx_pool).await?;
        }
        ensure_database_schema_supported(&sqlx_pool).await?;

        let den_state = den_service::DenState::new(
            sqlx_pool.clone(),
            config.clone(),
            Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
            memory_stores.clone(),
        );
        den_service::bifrost::spawn_managed_catalog_refresh(
            den_state.bifrost.clone(),
            den_state.bifrost_catalog.clone(),
            config.bifrost_catalog_refresh_secs,
            config.clone(),
        );
        den_runtime::native_runtime::set_tool_invoker(Arc::new(
            crate::core::tools::runtime_invoker::DenRuntimeToolInvoker::new(den_state.clone()),
        ));

        let session_store = PostgresStore::new(sqlx_pool.clone());
        session_store
            .migrate()
            .await
            .map_err(|e| StartupError::SessionStore(format!("{e:?}")))?;

        let deletion_task = tokio::task::spawn(
            session_store
                .clone()
                .continuously_delete_expired(tokio::time::Duration::from_mins(1)),
        );
        deletion_task_abort_handle = Some(deletion_task.abort_handle());

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
            let web_app = web::server_with_state_and_runtime(
                sqlx_pool.clone(),
                session_store.clone(),
                config_web,
                memory_stores.clone(),
                native_web_chat_runtime(memory_stores.clone()),
            )
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

            // Composition root: wire peer HTTP edges together. den-api owns the
            // JSON/REST + OAuth app; BearWire is injected here as a peer router so
            // neither edge depends on the other (ADR-0043).
            let peer_routers: Vec<(&'static str, axum::Router<den_service::DenState>)> = vec![
                ("/internal", crate::internal_tools::router()),
                ("/bearwire", den_bearwire::router()),
            ];
            let api_app =
                api::create_api_app(den_state.clone(), session_store.clone(), peer_routers)
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

            let expiry_token = CancellationToken::new();
            bearwire_expiry_token_opt = Some(expiry_token.clone());
            let expiry_state = den_state.clone();
            task_set.spawn(async move {
                den_bearwire::run_client_obligation_expiry_loop(
                    expiry_state,
                    expiry_token,
                    std::time::Duration::from_secs(1),
                )
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

        worker_token_opt = if config.run_workers {
            Some(CancellationToken::new())
        } else {
            None
        };

        if let Some(token) = worker_token_opt.clone() {
            let t = token;
            let worker_pool = sqlx_pool.clone();
            let worker_config = config.clone();
            let worker_stores = memory_stores.clone();
            task_set.spawn(async move {
                tracing::info!("Workers: memory_curate runner loop enabled");
                den_runtime::reflection_conductor::run_memory_curate_worker_loop(
                    worker_pool,
                    worker_config,
                    worker_stores,
                    t,
                    std::time::Duration::from_secs(5),
                )
                .await
                .map_err(std::io::Error::other)
            });
        } else {
            tracing::info!("Workers disabled (RUN_WORKERS=false or not set)");
        }

        if let Some(token) = worker_token_opt.clone() {
            let t = token;
            let worker_state = den_state.clone();
            task_set.spawn(async move {
                tracing::info!("Workers: open-session pair reflection loop enabled");
                den_bearwire::run_open_session_reflection_loop(
                    worker_state,
                    t,
                    std::time::Duration::from_mins(5),
                )
                .await
                .map_err(std::io::Error::other)
            });
        }

        if let Some(token) = worker_token_opt.clone() {
            if config.qdrant_url.is_some() {
                let t = token;
                let worker_pool = sqlx_pool.clone();
                let worker_config = config.clone();
                let worker_stores = memory_stores.clone();
                task_set.spawn(async move {
                    tracing::info!("Workers: recall_index runner loop enabled");
                    den_runtime::reflection_conductor::run_recall_index_worker_loop(
                        worker_pool,
                        worker_config,
                        worker_stores,
                        t,
                        std::time::Duration::from_secs(5),
                    )
                    .await
                    .map_err(std::io::Error::other)
                });
            } else {
                tracing::info!("Workers: recall_index loop disabled (QDRANT_URL unset)");
            }
        }

        if let Some(token) = worker_token_opt.clone() {
            let t = token;
            let worker_pool = sqlx_pool.clone();
            let worker_config = config.clone();
            task_set.spawn(async move {
                tracing::info!("Workers: context_compact runner loop enabled");
                den_runtime::reflection_conductor::run_context_compact_worker_loop(
                    worker_pool,
                    worker_config,
                    t,
                    std::time::Duration::from_secs(5),
                )
                .await
                .map_err(std::io::Error::other)
            });
        }

        if let Some(token) = worker_token_opt.clone() {
            let t = token;
            let worker_pool = sqlx_pool.clone();
            let worker_config = config.clone();
            let worker_stores = memory_stores.clone();
            task_set.spawn(async move {
                tracing::info!("Workers: archive_harvest runner loop enabled");
                den_runtime::reflection_conductor::run_archive_harvest_worker_loop(
                    worker_pool,
                    worker_config,
                    worker_stores,
                    t,
                    std::time::Duration::from_secs(30),
                )
                .await
                .map_err(std::io::Error::other)
            });
        }

        if let Some(token) = worker_token_opt.clone() {
            if config.sandbox_server_url.is_some() {
                let t = token;
                let worker_pool = sqlx_pool.clone();
                let worker_config = config.clone();
                task_set.spawn(async move {
                    tracing::info!("Workers: work_dispatch runner loop enabled");
                    den_runtime::work_dispatch::run_work_dispatch_worker_loop(
                        worker_pool,
                        worker_config,
                        t,
                        std::time::Duration::from_secs(2),
                    )
                    .await
                    .map_err(std::io::Error::other)
                });
            } else {
                tracing::info!("Workers: work_dispatch loop disabled (SANDBOX_SERVER_URL unset)");
            }
        }
    }

    if config.run_sandbox {
        let sandbox_addr = SocketAddr::from(([0, 0, 0, 0], config.sandbox_port));
        tracing::info!("Starting sandbox provider on http://{}", sandbox_addr);

        let sandbox_listener = tokio::net::TcpListener::bind(sandbox_addr)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to bind sandbox provider to port {}: {}",
                    config.sandbox_port,
                    e
                );
                e
            })?;

        let sandbox_app = den_sandbox::create_sandbox_app(
            den_sandbox::SandboxServerConfig::from_config(config.as_ref()),
        )
        .map_err(|e| {
            tracing::error!("Failed to create sandbox provider: {}", e);
            StartupError::SandboxProvider(e.to_string())
        })?;

        task_set.spawn(async move {
            tracing::info!("Sandbox provider started successfully");
            axum::serve(sandbox_listener, sandbox_app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(std::io::Error::other)
        });
    }

    tracing::info!("All services started successfully. Waiting for shutdown signal...");

    shutdown_signal().await;

    tracing::info!("Shutdown signal received. Stopping services...");

    if let Some(handle) = deletion_task_abort_handle {
        handle.abort();
    }

    if let Some(token) = worker_token_opt {
        token.cancel();
    }

    if let Some(token) = bearwire_expiry_token_opt {
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

fn init_tracing() -> Result<(), StartupError> {
    #[cfg(feature = "production")]
    let default_tracing_filter = "den=info,\
        den::web=info,\
        den::api=info,\
        tower_sessions=info,\
        tower_http=info,\
        axum=info,\
        axum_login=info";
    #[cfg(not(feature = "production"))]
    let default_tracing_filter = "den=info,\
        den::core=debug,\
        den::web=debug,\
        den::api=debug,\
        tower_sessions=info,\
        tower_http=info,\
        axum=info,\
        axum_login=info";
    let tracing_filter =
        std::env::var("RUST_LOG").unwrap_or_else(|_| default_tracing_filter.to_string());

    tracing_subscriber::registry()
        .with(EnvFilter::new(tracing_filter))
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .map_err(|e| StartupError::Tracing(e.to_string()))
}

async fn connect_database(config: &Config) -> Result<sqlx::PgPool, StartupError> {
    let db_redacted = redact_database_url(&config.database_url);
    tracing::info!(
        db_url = %db_redacted,
        max_connections = config.db_max_connections,
        acquire_timeout_secs = config.db_acquire_timeout_secs,
        idle_timeout_secs = config.db_idle_timeout_secs,
        "Connecting to database",
    );

    let idle_timeout = if config.db_idle_timeout_secs == 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(config.db_idle_timeout_secs))
    };

    let sqlx_pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            config.db_acquire_timeout_secs,
        ))
        .idle_timeout(idle_timeout)
        .connect(&config.database_url)
        .await
        .map_err(|e| {
            tracing::error!(
                db_url = %db_redacted,
                error = %e,
                "Failed to connect to database — is Postgres running and accepting connections?",
            );
            StartupError::Database {
                message: format!("{e}"),
                db_url: db_redacted.clone(),
                hint: "Check that Postgres is running, DATABASE_URL is correct, \
                       and the host/port are reachable from this container."
                    .into(),
            }
        })?;

    tracing::info!("Database connected successfully");
    Ok(sqlx_pool)
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
            () = ctrl_c => {
                tracing::info!("Initiating graceful shutdown due to Ctrl+C");
            },
            () = terminate => {
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

/// Return a copy of the DATABASE_URL with the password replaced by `***`.
fn redact_database_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        }
        Err(_) => "<unparseable DATABASE_URL>".into(),
    }
}
