//! The den web edge: server-rendered UI (Askama/MiniJinja over axum) plus the
//! `/v1/*` JSON + SSE surface for the in-app chat. Depends on den-http (identity/
//! errors), den-api (OAuth client types + the injected tool invoker), den-runtime,
//! den-docket, and den-core.
//!
// ROUTES: When modifying routes in this file, update ROUTES.md if present.

// Self-alias so the edge keeps resolving its historical `crate::web::*` paths
// (this crate *is* the old `den::web` module tree). Avoids rewriting the many
// `web::AppState` / `web::*` references across submodules.
extern crate self as web;

// Foundation re-export shims (v1.5 den-web extraction): keep migrated call sites
// resolving `crate::config`, `crate::errors`, `crate::auth_backend`, and
// `crate::build_info` unchanged. These live in den-core / den-http now.
pub use den_core::config;
pub use den_http::{auth_backend, build_info, errors};

pub mod connections;
pub mod core;
pub mod observability;

pub mod admin;
pub mod bear;
pub mod cabinet;
pub mod design;
pub mod filters;
pub mod home;
pub mod management_hub;
pub mod onboarding;
pub mod public;
pub mod stack_health;
pub mod status;
pub mod user;
pub mod v1;
pub mod web_chat_runtime;
pub mod work;

#[cfg(test)]
mod ownership_ui_tests;
#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use std::sync::OnceLock;

use crate::errors::CustomError;
use crate::{auth_backend::Backend, config::Config};

use axum::{
    extract::{MatchedPath, State},
    http::{header, Request, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use axum_extra::routing::RouterExt;

use memory_serve::{load_assets, CacheControl, MemoryServe};

use axum_login::{
    login_required, permission_required,
    tower_sessions::{
        cookie::SameSite,
        {Expiry, SessionManagerLayer},
    },
    AuthManagerLayerBuilder, AuthSession,
};
use sqlx::postgres::PgPool;
use tower_sessions_sqlx_store::PostgresStore;

use tower_http::trace::TraceLayer;
use tracing::info_span;

use time::Duration;

use minijinja::Environment;
use std::sync::Arc;

/// Week start day options (0 = Sunday … 6 = Saturday), for settings UI.
pub fn day_of_week_names() -> IndexMap<i32, &'static str> {
    [
        (0, "Sunday"),
        (1, "Monday"),
        (2, "Tuesday"),
        (3, "Wednesday"),
        (4, "Thursday"),
        (5, "Friday"),
        (6, "Saturday"),
    ]
    .into_iter()
    .collect()
}

pub fn theme_descriptions() -> &'static IndexMap<&'static str, &'static str> {
    static THEME_DESCRIPTIONS: OnceLock<IndexMap<&str, &str>> = OnceLock::new();
    THEME_DESCRIPTIONS.get_or_init(|| {
        let mut m = IndexMap::new();
        m.insert("system", "Use system setting");
        m.insert("dark", "Always dark mode");
        m.insert("light", "Always light mode");
        m
    })
}

#[derive(Clone)]
pub struct AppState {
    sqlx_pool: PgPool,
    template_env: Environment<'static>,
    asset_router: Arc<Router<AppState>>,
    pub config: Arc<Config>,
    pub bifrost: std::sync::Arc<den_service::bifrost::BifrostClient>,
    pub bifrost_catalog: den_service::bifrost::BifrostCatalogStore,
    pub web_chat_runtime: Arc<dyn crate::web_chat_runtime::WebChatRuntime>,
    pub media: Option<crate::core::s3::MediaStore>,
    /// Clone of the process-wide per-Bear memory store manager (ADR-0031:
    /// one `MemoryStoreManager` instance per process; clones share pools).
    pub memory_stores: den_memory::MemoryStoreManager,
}

impl AppState {
    pub fn sqlx_pool(&self) -> &PgPool {
        &self.sqlx_pool
    }

    #[cfg(test)]
    pub(crate) fn test_with_template_env(
        sqlx_pool: PgPool,
        template_env: Environment<'static>,
        config: Arc<Config>,
    ) -> Self {
        Self::test_with_template_env_and_chat_runtime(
            sqlx_pool,
            template_env,
            config,
            crate::web_chat_runtime::unavailable_web_chat_runtime(),
        )
    }

    #[cfg(test)]
    pub(crate) fn test_with_template_env_and_chat_runtime(
        sqlx_pool: PgPool,
        template_env: Environment<'static>,
        config: Arc<Config>,
        web_chat_runtime: Arc<dyn crate::web_chat_runtime::WebChatRuntime>,
    ) -> Self {
        let bifrost =
            std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref()));
        let memory_stores = den_memory::MemoryStoreManager::new(config.as_ref());
        Self {
            sqlx_pool,
            template_env,
            asset_router: Arc::new(Router::new()),
            config,
            bifrost,
            bifrost_catalog: den_service::bifrost::new_catalog_store(),
            web_chat_runtime,
            media: None,
            memory_stores,
        }
    }
}

async fn web_manifest(State(state): State<AppState>) -> impl IntoResponse {
    let body = serde_json::json!({
        "lang": "en",
        "id": state.config.app_slug,
        "name": state.config.app_display_name,
        "short_name": state.config.app_slug,
        "icons": [{
            "src": "/assets/icons/jaunty.png",
            "sizes": "96x96",
            "type": "image/png"
        }],
        "start_url": "/",
        "display": "fullscreen",
        "theme_color": "black",
        "background_color": "white"
    })
    .to_string();
    ([(header::CONTENT_TYPE, "application/manifest+json")], body)
}

async fn web_readiness(State(state): State<AppState>) -> Result<&'static str, StatusCode> {
    sqlx::query_scalar!("SELECT 1")
        .fetch_one(state.sqlx_pool())
        .await
        .map_err(|e| {
            let pool = state.sqlx_pool();
            tracing::warn!(
                error = %e,
                pool_size = pool.size(),
                pool_idle = pool.num_idle(),
                "database readiness check failed (web)",
            );
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    Ok("OK")
}

pub async fn server_with_state(
    sqlx_pool: PgPool,
    session_store: PostgresStore,
    config: Arc<Config>,
    memory_stores: den_memory::MemoryStoreManager,
) -> Result<Router, Box<dyn std::error::Error>> {
    server_with_state_and_runtime(
        sqlx_pool,
        session_store,
        config,
        memory_stores,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await
}

pub fn template_environment(config: &Config) -> Environment<'static> {
    #[cfg(feature = "production")]
    let _ = config;

    let mut env = Environment::new();
    env.add_filter("hexadecimal", filters::hexadecimal);
    env.add_filter("urlencode", filters::urlencode);
    env.add_filter("markdown", filters::markdown);
    env.add_filter("timeago", filters::time::timeago);
    env.add_filter("humanize_time", filters::time::timeago);
    env.add_filter("is_future", filters::time::is_future);
    minijinja_contrib::add_to_environment(&mut env);

    #[cfg(feature = "production")]
    {
        minijinja_embed::load_templates!(&mut env);
    }
    #[cfg(not(feature = "production"))]
    {
        let template_path = &config.templates_dir;
        env.set_loader(minijinja::path_loader(template_path));
    }

    env
}

pub async fn server_with_state_and_runtime(
    sqlx_pool: PgPool,
    session_store: PostgresStore,
    config: Arc<Config>,
    memory_stores: den_memory::MemoryStoreManager,
    web_chat_runtime: Arc<dyn crate::web_chat_runtime::WebChatRuntime>,
) -> Result<Router, Box<dyn std::error::Error>> {
    let env = template_environment(config.as_ref());

    let memory_serve =
        MemoryServe::new(load_assets!("src/assets")).cache_control(CacheControl::Short);

    let bifrost = std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref()));
    let bifrost_catalog = den_service::bifrost::new_catalog_store();
    den_service::bifrost::spawn_managed_catalog_refresh(
        bifrost.clone(),
        bifrost_catalog.clone(),
        config.bifrost_catalog_refresh_secs,
        config.clone(),
    );

    let media = crate::core::s3::MediaStore::new(config.as_ref());
    server(
        AppState {
            sqlx_pool: sqlx_pool.clone(),
            template_env: env.clone(),
            asset_router: Arc::new(memory_serve.into_router()),
            config: config.clone(),
            bifrost,
            bifrost_catalog,
            web_chat_runtime,
            media,
            memory_stores,
        },
        session_store,
    )
    .await
}

struct ImportStagingCleanup(tokio::task::AbortHandle);

impl Drop for ImportStagingCleanup {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn server(
    state: AppState,
    session_store: PostgresStore,
) -> Result<Router, Box<dyn std::error::Error>> {
    let import_cleanup = Arc::new(ImportStagingCleanup(
        bear::settings::start_import_staging_cleanup(state.config.clone()).abort_handle(),
    ));
    crate::cabinet::cleanup::spawn(state.clone());
    let mut session_layer = SessionManagerLayer::new(session_store)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)));

    #[cfg(feature = "production")]
    {
        session_layer =
            session_layer.with_secure(crate::config::session_cookie_secure_from_env(true));
        let session_cookie_domain: Option<String> = state
            .config
            .session_cookie_domain
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(domain) = session_cookie_domain {
            session_layer = session_layer.with_domain(domain);
        }
    }

    #[cfg(not(feature = "production"))]
    {
        session_layer =
            session_layer.with_secure(crate::config::session_cookie_secure_from_env(false));
    }

    let backend = Backend::new(state.sqlx_pool.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    let asset_router = Arc::as_ref(&state.asset_router).clone();

    // Register liveness/readiness, manifest, and BEARS `/status` **first** (before admin,
    // `/bear/*`, and `public::router()`'s fallback) so `/health/*` and `/status*` resolve before fallbacks.
    let router = Router::new()
        .merge(
            Router::new()
                .route("/manifest.json", get(web_manifest))
                .route("/health", get(|| async { "OK" }))
                .route("/version", get(build_info::json_handler))
                .route("/healthcheck", get(|| async { "OK" }))
                .route("/health/ready", get(web_readiness))
                .route(
                    "/metrics",
                    get(|| async {
                        let text = crate::observability::metrics::render_prometheus_text();
                        (
                            [(
                                header::CONTENT_TYPE,
                                "text/plain; version=0.0.4; charset=utf-8",
                            )],
                            text,
                        )
                    }),
                )
                .route("/status", get(status::page))
                .route("/status.json", get(status::json_endpoint)),
        )
        .merge(
            Router::new()
                .nest("/admin", admin::router())
                .route_layer(permission_required!(Backend, login_url = "/login", "admin")),
        )
        .merge(
            Router::new()
                .merge(bear::management::router())
                .merge(bear::memory::router())
                .merge(bear::manage::router())
                .merge(bear::skills::router())
                .merge(onboarding::router())
                .merge(management_hub::router())
                .merge(connections::router())
                .merge(cabinet::router())
                .merge(work::router())
                .nest("/bear/{bear_slug}", work::docket_router())
                // TSR: conversation links use `/bear/{slug}/?conversation_id=…`; plain `/bear/{slug}` is the canonical chat URL.
                .route_with_tsr("/bear/{slug}", get(bear::chat::bear_page))
                .route_layer(login_required!(Backend, login_url = "/login")),
        )
        .nest("/v1", v1::router())
        .merge(user::router())
        .merge(design::router())
        .merge(home::router())
        .merge(public::router())
        .nest("/assets", asset_router)
        .layer(auth_layer)
        .layer(axum::Extension(import_cleanup))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let matched_path = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map(MatchedPath::as_str);

                info_span!(
                    "http_request",
                    method = ?request.method(),
                    matched_path,
                    path = ?request.uri().path(),
                )
            }),
        )
        .with_state(state);

    Ok(router)
}

enum TemplateRenderTarget<'a> {
    Template(&'a str),
    Block {
        template_name: &'a str,
        block_name: &'a str,
    },
}

impl<'a> TemplateRenderTarget<'a> {
    fn from_template_id(template_id: &'a str) -> Self {
        match template_id.split_once('#') {
            Some((template_name, block_name)) => Self::Block {
                template_name,
                block_name,
            },
            None => Self::Template(template_id),
        }
    }
}

fn template_tag_for_template_id(template_id: &str) -> Option<String> {
    template_id
        .replace('/', "-")
        .split_once('.')
        .map(|(template_tag, _)| template_tag.to_string())
}

#[allow(deprecated)] // minijinja: eval_to_state; migrate to render_captured when upgrading templates
pub async fn render_template(
    state: &AppState,
    template_id: &str,
    auth_session: AuthSession<Backend>,
    ctx: minijinja::Value,
) -> Result<Response, CustomError> {
    let ctx = minijinja::context! {
        app_display_name => state.config.app_display_name.clone(),
        app_slug => state.config.app_slug.clone(),
        public_web_origin => state.config.web_public_origin(),
        ui_fixture_profile => state.config.ui_fixture_profile.map(|p| p.as_str().to_string()),
        native_runtime => true,
        ..ctx
    };
    let template_env = state.template_env.clone();
    let Some(template_tag) = template_tag_for_template_id(template_id) else {
        return Err(CustomError::Render(format!(
            "Template id '{template_id}' is not a valid template name"
        )));
    };
    let merged_ctx = match auth_session.user {
        Some(user) => minijinja::context! {
            template_tag => template_tag,
            session => { minijinja::context! {
                user_id => user.id,
                username => user.username,
                is_admin => user.is_admin,
                theme => user.theme,
            }},
            ..ctx
        },
        None => ctx,
    };

    match TemplateRenderTarget::from_template_id(template_id) {
        TemplateRenderTarget::Block {
            template_name,
            block_name,
        } => {
            let template = template_env.get_template(template_name).map_err(|e| {
                CustomError::Render(format!("Unable to find template '{template_name}': {e:?}"))
            })?;
            let rendered = template
                .eval_to_state(merged_ctx)
                .map_err(|e| {
                    CustomError::Render(format!(
                        "Unable to parse template '{template_name}': {e:?}"
                    ))
                })?
                .render_block(block_name)
                .map_err(|e| {
                    CustomError::Render(format!("Unable to render block '{template_id}': {e:?}"))
                })?;
            Ok(Html(rendered).into_response())
        }
        TemplateRenderTarget::Template(template_name) => {
            match template_env.get_template(template_name) {
                Ok(template) => match template.render(merged_ctx) {
                    Ok(rendered) => Ok(Html(rendered).into_response()),
                    Err(e) => {
                        tracing::error!("Error rendering template: {:#}", e);
                        Err(CustomError::Render(format!(
                            "Error rendering template '{template_id}'"
                        )))
                    }
                },
                Err(_) => {
                    tracing::error!("Template `{}` not found", template_id);
                    Err(CustomError::Render(format!(
                        "Template `{template_id}` not found"
                    )))
                }
            }
        }
    }
}
