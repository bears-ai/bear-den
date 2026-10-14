//! Explicit Cabinet HTTP composition, independent of Den's native runtime.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get, post},
    Router,
};
use axum_extra::extract::{Form, Query};
use axum_login::{
    login_required,
    tower_sessions::{cookie::SameSite, Expiry, SessionManagerLayer, SessionStore},
    AuthManagerLayerBuilder,
};
use den_core::config::{cabinet::validate_origin, CabinetOriginError, WebSurface};
use memory_serve::{load_assets, CacheControl, MemoryServe};
use serde::Deserialize;
use sqlx::PgPool;
use time::Duration;
use tower_http::trace::TraceLayer;
use tower_sessions_sqlx_store::PostgresStore;
use url::Url;
use validator::Validate;

use crate::{
    auth_backend::{AuthSession, Backend, Credentials},
    config::Config,
    errors::CustomError,
    AppState,
};

pub const CABINET_SESSION_COOKIE: &str = "cabinet_session";

/// Keep the authenticated local rollout fallback. With a dedicated destination,
/// mount only a legacy-link handler: no Cabinet writers remain on Den, including
/// future or unknown paths. The caller mounts this outside Den authentication.
pub fn den_cabinet_router(config: &Config) -> Router<AppState> {
    match config.cabinet_destination() {
        den_core::config::CabinetDestination::Local => {
            crate::cabinet::router().route_layer(login_required!(Backend, login_url = "/login"))
        }
        destination => Router::new()
            .route("/cabinet", any(legacy_cabinet_destination))
            .route("/cabinet/", any(legacy_cabinet_destination))
            .route("/cabinet/{*path}", any(legacy_cabinet_destination))
            .with_state(destination),
    }
}

async fn legacy_cabinet_destination(
    State(destination): State<den_core::config::CabinetDestination>,
    request: Request,
) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        if let Some(path) = request.uri().path_and_query() {
            if let Ok(target) = destination.resolve_path(path.as_str()) {
                return Redirect::temporary(&target).into_response();
            }
        }
        return StatusCode::BAD_REQUEST.into_response();
    }
    (
        StatusCode::GONE,
        "Cabinet mutations on Den are retired; submit a new request on the Cabinet origin",
    )
        .into_response()
}

/// No memory manager construction, SQLite access, catalog refresh, or native
/// runtime initialization. The inert Bifrost client is retained only for legacy
/// AppState compatibility; no Cabinet routes can access that capability.
pub fn cabinet_state(pool: PgPool, config: Arc<Config>) -> Result<AppState, CabinetOriginError> {
    let origin = config
        .cabinet_web_origin
        .as_ref()
        .ok_or(CabinetOriginError)?;
    validate_origin(origin)?;
    Ok(AppState {
        sqlx_pool: pool,
        template_env: crate::template_environment(&config),
        asset_router: Arc::new(
            MemoryServe::new(load_assets!("src/assets"))
                .cache_control(CacheControl::Short)
                .into_router(),
        ),
        bifrost: Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        bifrost_catalog: den_service::bifrost::new_catalog_store(),
        web_chat_runtime: crate::web_chat_runtime::unavailable_web_chat_runtime(),
        media: crate::core::s3::MediaStore::new(&config),
        config,
        memory_stores: None,
        surface: WebSurface::Cabinet,
    })
}

pub async fn cabinet_server(
    pool: PgPool,
    session_store: PostgresStore,
    config: Arc<Config>,
) -> Result<Router, Box<dyn std::error::Error>> {
    let state = cabinet_state(pool, config)?;
    if state.config.owns_cabinet_recovery(state.surface) {
        crate::cabinet::cleanup::spawn(state.clone());
    }
    Ok(cabinet_router(state, session_store))
}

fn cabinet_session_layer<S: SessionStore>(state: &AppState, store: S) -> SessionManagerLayer<S> {
    // Deliberately never inherit SESSION_COOKIE_DOMAIN or Den's session name.
    // Production Cabinet cookies remain Secure even if Den's local override is off.
    let secure = cfg!(feature = "production")
        || state
            .config
            .cabinet_web_origin
            .as_ref()
            .is_some_and(|origin| origin.scheme() == "https");
    SessionManagerLayer::new(store)
        .with_name(CABINET_SESSION_COOKIE)
        .with_path("/")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
}

/// Pure route composition: tests can verify isolation without running workers.
pub fn cabinet_router(state: AppState, session_store: PostgresStore) -> Router {
    let auth_layer = AuthManagerLayerBuilder::new(
        Backend::new(state.sqlx_pool().clone()),
        cabinet_session_layer(&state, session_store),
    )
    .build();
    Router::new()
        .merge(crate::cabinet::router().route_layer(login_required!(Backend, login_url = "/login")))
        .route("/", get(|| async { Redirect::to("/cabinet") }))
        // Cabinet's index contains only policy-authorized Cabinet reviews, not
        // the mixed Den memory/Work review hub (which requires native memory).
        .route("/reviews", get(|| async { Redirect::to("/cabinet") }))
        .route("/login", get(login_form))
        .route("/login/password", post(login_password))
        .route("/logout", get(logout).post(logout))
        .route("/health", get(|| async { "OK" }))
        .route("/healthcheck", get(|| async { "OK" }))
        .route("/health/ready", get(crate::web_readiness))
        .route("/version", get(crate::build_info::json_handler))
        .route("/manifest.json", get(manifest))
        .nest("/assets", Arc::as_ref(&state.asset_router).clone())
        .layer(auth_layer)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            protect_mutations,
        ))
        // Native bearer MCP retains its own transport guard and must not enter
        // browser cookie authentication or the browser Origin/Referer policy.
        .merge(crate::cabinet_mcp::router())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Cabinet-local cookies authenticate independently against the shared Den user
/// database. They do not imply cross-origin SSO or accept Den's cookie name.
#[derive(Deserialize, Validate)]
struct LoginForm {
    #[validate(length(min = 1, max = 255))]
    username: String,
    #[validate(length(min = 8))]
    password: String,
    next: Option<String>,
}

#[derive(Default, Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

fn local_next(next: Option<&str>) -> String {
    den_core::config::CabinetDestination::Local
        .resolve_path(next.unwrap_or("/cabinet"))
        .unwrap_or_else(|_| "/cabinet".into())
}

async fn login_form(
    State(state): State<AppState>,
    auth: AuthSession,
    Query(query): Query<LoginQuery>,
) -> Result<Response, CustomError> {
    let next = local_next(query.next.as_deref());
    if auth.user.is_some() {
        return Ok(Redirect::to(&next).into_response());
    }
    crate::render_template(&state, "login.html", auth, minijinja::context! { next }).await
}

async fn login_password(
    State(state): State<AppState>,
    mut auth: AuthSession,
    Form(form): Form<LoginForm>,
) -> Result<Response, CustomError> {
    let next = local_next(form.next.as_deref());
    let username = form.username.clone();
    if form.validate().is_ok() {
        let credentials = Credentials {
            username: form.username,
            password: form.password,
            next: None,
            su: false,
        };
        if let Some(user) = auth.authenticate(credentials).await? {
            auth.login(&user).await?;
            return Ok(Redirect::to(&next).into_response());
        }
    }
    crate::render_template(
        &state,
        "login.html",
        auth,
        minijinja::context! {
            next,
            message => "Username or password was not recognized. Try again.",
            form_data => minijinja::context! { username },
        },
    )
    .await
}

async fn logout(mut auth: AuthSession) -> Result<Redirect, CustomError> {
    auth.logout().await?;
    Ok(Redirect::to("/login"))
}

fn same_origin(raw: &str, expected: &Url, origin_header: bool) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    url.username().is_empty()
        && url.password().is_none()
        && url.origin() == expected.origin()
        && (!origin_header || validate_origin(&url).is_ok())
}

fn mutation_origin_allowed(headers: &HeaderMap, expected: &Url) -> bool {
    // Never fall back to Referer after an invalid or conflicting Origin.
    let mut origins = headers.get_all(header::ORIGIN).iter();
    if let Some(origin) = origins.next() {
        return origins.next().is_none()
            && origin
                .to_str()
                .is_ok_and(|raw| same_origin(raw, expected, true));
    }
    let mut referers = headers.get_all(header::REFERER).iter();
    referers.next().is_some_and(|referer| {
        referers.next().is_none()
            && referer
                .to_str()
                .is_ok_and(|raw| same_origin(raw, expected, false))
    })
}

async fn protect_mutations(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mutates = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) || request.uri().path() == "/logout";
    if mutates
        && !state
            .config
            .cabinet_web_origin
            .as_ref()
            .is_some_and(|origin| mutation_origin_allowed(request.headers(), origin))
    {
        return (
            StatusCode::FORBIDDEN,
            "Cabinet mutations require a same-origin Origin or Referer header",
        )
            .into_response();
    }
    next.run(request).await
}

async fn manifest(State(state): State<AppState>) -> impl IntoResponse {
    let body = serde_json::json!({
        "lang": "en", "id": "cabinet", "name": format!("{} Cabinet", state.config.app_display_name),
        "short_name": "Cabinet", "start_url": "/cabinet", "scope": "/", "display": "standalone",
        "icons": [{ "src": "/assets/icons/jaunty.png", "sizes": "96x96", "type": "image/png" }],
    });
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        body.to_string(),
    )
}

#[cfg(test)]
mod browser_tests;
#[cfg(test)]
mod tests;
