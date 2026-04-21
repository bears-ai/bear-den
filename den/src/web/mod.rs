// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod admin;
pub mod bears_health;
pub mod status;
pub mod bear_chat;
pub mod bear_create_support;
pub mod bear_management;
pub mod filters;
pub mod home;
pub mod public;
pub mod user;
pub mod v1;

use indexmap::IndexMap;
use std::sync::OnceLock;

use crate::build_info;
use crate::errors::CustomError;
use crate::{auth_backend::Backend, config::Config};

use axum::{
    Router,
    extract::{MatchedPath, State},
    http::{header, Request, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use axum_extra::routing::RouterExt;

use memory_serve::{CacheControl, MemoryServe, load_assets};

use axum_login::{
    AuthManagerLayerBuilder, AuthSession, login_required, permission_required,
    tower_sessions::{
        cookie::SameSite,
        {Expiry, SessionManagerLayer},
    },
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
    pub letta: std::sync::Arc<crate::core::letta::LettaClient>,
    pub codepool: std::sync::Arc<crate::core::codepool::CodePoolClient>,
    pub media: Option<crate::core::s3::MediaStore>,
}

impl AppState {
    pub fn sqlx_pool(&self) -> &PgPool {
        &self.sqlx_pool
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
    sqlx::query_scalar::<_, i32>("SELECT 1")
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
) -> Result<Router, Box<dyn std::error::Error>> {
    let mut env = Environment::new();
    env.add_filter("hexadecimal", filters::hexadecimal);
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

    let memory_serve =
        MemoryServe::new(load_assets!("src/web/assets")).cache_control(CacheControl::Short);

    let letta = std::sync::Arc::new(crate::core::letta::LettaClient::new(config.as_ref()));
    let codepool = std::sync::Arc::new(crate::core::codepool::CodePoolClient::new(
        config.as_ref(),
    ));
    let media = crate::core::s3::MediaStore::new(config.as_ref());
    server(
        AppState {
            sqlx_pool: sqlx_pool.clone(),
            template_env: env.clone(),
            asset_router: Arc::new(memory_serve.into_router()),
            config: config.clone(),
            letta,
            codepool,
            media,
        },
        session_store,
    )
    .await
}

pub async fn server(
    state: AppState,
    session_store: PostgresStore,
) -> Result<Router, Box<dyn std::error::Error>> {
    let mut session_layer = SessionManagerLayer::new(session_store)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)));

    #[cfg(feature = "production")]
    {
        session_layer = session_layer.with_secure(true);
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
        session_layer = session_layer.with_secure(false);
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
                .route("/status", get(status::page))
                .route("/status.json", get(status::json_endpoint))
                .route("/health/bears", get(bears_health::page))
                .route("/health/bears.json", get(bears_health::json_endpoint)),
        )
        .merge(
            Router::new()
                .nest("/admin", admin::router())
                .route_layer(permission_required!(Backend, login_url = "/login", "admin")),
        )
        .merge(
            Router::new()
                .merge(bear_management::router())
                // TSR: conversation links use `/bear/{slug}/?conversation_id=…`; plain `/bear/{slug}` is the canonical chat URL.
                .route_with_tsr("/bear/{slug}", get(bear_chat::bear_page))
                .route_layer(login_required!(Backend, login_url = "/login")),
        )
        .nest("/v1", v1::router())
        .merge(user::router())
        .merge(home::router())
        .merge(public::router())
        .nest("/assets", asset_router)
        .layer(auth_layer)
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
        ..ctx
    };
    let template_env = state.template_env.clone();
    if let Some((template_tag, _)) = template_id.replace("/", "-").clone().split_once('.') {
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

        if template_id.contains('#') {
            let segments = template_id.split_once('#');
            let template_name = segments.map(|seg| seg.0);
            let block_name = segments.map(|seg| seg.1);

            if let (Some(template_name), Some(block_name)) = (template_name, block_name) {
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
                        CustomError::Render(format!(
                            "Unable to render block '{template_id}': {e:?}"
                        ))
                    })?;
                Ok(Html(rendered).into_response())
            } else {
                Err(CustomError::Render(
                    "Template id must be 'template_name#block_name'".to_string(),
                ))
            }
        } else if let Ok(template) = template_env.get_template(template_id) {
            match template.render(merged_ctx) {
                Ok(rendered) => Ok(Html(rendered).into_response()),
                Err(e) => {
                    tracing::error!("Error rendering template: {:#}", e);
                    Err(CustomError::Render(format!(
                        "Error rendering template '{template_id}'"
                    )))
                }
            }
        } else {
            tracing::error!("Template `{}` not found", template_id);
            Err(CustomError::Render(format!(
                "Template `{template_id}` not found"
            )))
        }
    } else {
        Err(CustomError::Render(format!(
            "Template id '{template_id}' is not a valid template name"
        )))
    }
}
