// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
pub mod admin;
pub mod filters;
pub mod home;
pub mod public;
pub mod user;

use indexmap::IndexMap;
use std::sync::OnceLock;

use crate::errors::CustomError;
use crate::{auth_backend::Backend, config::Config};

use axum::{
    Router,
    extract::{MatchedPath, State},
    http::{Request, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};

use memory_serve::{CacheControl, MemoryServe, load_assets};

use axum_login::{
    AuthManagerLayerBuilder, AuthSession, permission_required,
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
}

impl AppState {
    pub fn sqlx_pool(&self) -> &PgPool {
        &self.sqlx_pool
    }
}

async fn web_readiness(State(state): State<AppState>) -> Result<&'static str, StatusCode> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.sqlx_pool())
        .await
        .map_err(|e| {
            tracing::warn!("database readiness check failed: {e}");
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

    server(
        AppState {
            sqlx_pool: sqlx_pool.clone(),
            template_env: env.clone(),
            asset_router: Arc::new(memory_serve.into_router()),
            config: config.clone(),
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

    let router = Router::new()
        .nest("/admin", admin::router())
        .route_layer(permission_required!(Backend, login_url = "/login", "admin"))
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
        .route("/healthcheck", get(|| async { "OK" }))
        .route("/health/ready", get(web_readiness))
        .with_state(state);

    Ok(router)
}

#[allow(deprecated)] // minijinja: eval_to_state; migrate to render_captured when upgrading templates
pub async fn render_template(
    template_env: Environment<'static>,
    template_id: &str,
    auth_session: AuthSession<Backend>,
    ctx: minijinja::Value,
) -> Result<Response, CustomError> {
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
