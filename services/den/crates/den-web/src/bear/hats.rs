//! Bear-admin hat setup. Hat grants narrow Bear-assigned surfaces; binding is
//! immutable for an existing conversation or draft Job.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId, UserId};
use den_service::{
    bears::hats::{self, bindings, manage},
    conversation::persistence,
    work_surfaces,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::settings::{bear_nav_context, load_session_bear_manage, session_user};
use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

#[cfg(test)]
mod tests;

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/hats", get(index).post(create))
        .route_with_tsr("/bear/{slug}/hats/{hat_id}", get(detail).post(update))
        .route_with_tsr("/bear/{slug}/hats/{hat_id}/surfaces", post(set_surfaces))
        .route_with_tsr("/bear/{slug}/hats/{hat_id}/work", post(set_work))
        .route_with_tsr(
            "/bear/{slug}/hats/{hat_id}/conversations",
            post(create_conversation),
        )
        .route_with_tsr(
            "/bear/{slug}/conversations/{conversation_id}/hat",
            post(bind_conversation),
        )
}

#[derive(Debug, Deserialize)]
struct HatForm {
    name: String,
    purpose: String,
}

#[derive(Debug, Deserialize)]
struct SurfaceForm {
    #[serde(default)]
    surface_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct WorkForm {
    action: WorkAction,
    #[serde(default)]
    confirmation: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkAction {
    Enable,
    Disable,
}

#[derive(Debug, Deserialize)]
struct DetailQuery {
    #[serde(default)]
    message: Option<String>,
}

#[derive(Serialize)]
struct SurfaceChoice {
    id: Uuid,
    name: String,
    description: Option<String>,
    selected: bool,
}

fn hat_url(slug: &str, id: HatId) -> String {
    format!("/bear/{slug}/hats/{id}")
}

async fn index(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let hats = hats::list_hats(state.sqlx_pool(), BearId::new(bear.id)).await?;
    web::render_template(
        &state,
        "bear/manage/hats.jinja",
        auth,
        context! {
            hats, can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await
}

async fn create(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<HatForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let user_id = session_user(&auth).await?.id;
    let hat = hats::create_hat(
        state.sqlx_pool(),
        BearId::new(bear.id),
        UserId::new(user_id),
        &form.name,
        &form.purpose,
    )
    .await?;
    Ok(Redirect::to(&hat_url(&bear.slug, hat.id)).into_response())
}

async fn detail(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    Query(query): Query<DetailQuery>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let hat_id = HatId::new(hat_id);
    let hat = manage::get_hat(state.sqlx_pool(), bear_id, hat_id).await?;
    let granted = manage::allowed_surfaces(state.sqlx_pool(), bear_id, hat_id).await?;
    let choices: Vec<SurfaceChoice> =
        work_surfaces::list_surfaces_for_bears(state.sqlx_pool(), &[bear.id])
            .await?
            .into_iter()
            .map(|surface| SurfaceChoice {
                selected: granted.contains(&surface.id),
                id: surface.id,
                name: surface.name,
                description: surface.description,
            })
            .collect();
    web::render_template(
        &state,
        "bear/manage/hat.jinja",
        auth,
        context! {
            hat, choices, grant_count => granted.len(), message => query.message,
            can_manage_bear => true, native_runtime => true,
            ..bear_nav_context(&bear, "hats"),
        },
    )
    .await
}

async fn update(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<HatForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let id = HatId::new(hat_id);
    manage::update_hat(
        state.sqlx_pool(),
        BearId::new(bear.id),
        id,
        &form.name,
        &form.purpose,
    )
    .await?;
    Ok(Redirect::to(&hat_url(&bear.slug, id)).into_response())
}

async fn set_surfaces(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<SurfaceForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let id = HatId::new(hat_id);
    manage::replace_surfaces(
        state.sqlx_pool(),
        BearId::new(bear.id),
        id,
        &form.surface_ids,
    )
    .await?;
    Ok(Redirect::to(&hat_url(&bear.slug, id)).into_response())
}

async fn set_work(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<WorkForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let id = HatId::new(hat_id);
    match form.action {
        WorkAction::Enable => {
            if form.confirmation.trim() != "enable work" {
                return Err(CustomError::ValidationError(
                    "type enable work to confirm this wider audience".into(),
                ));
            }
            manage::enable_work_if_empty(state.sqlx_pool(), &state.memory_stores, bear_id, id)
                .await?;
        }
        WorkAction::Disable => manage::disable_work(state.sqlx_pool(), bear_id, id).await?,
    }
    Ok(Redirect::to(&hat_url(&bear.slug, id)).into_response())
}

async fn create_conversation(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let bear_id = BearId::new(bear.id);
    let hat_id = HatId::new(hat_id);
    manage::get_hat(state.sqlx_pool(), bear_id, hat_id).await?;
    let user_id = session_user(&auth).await?.id;
    let external_id = format!("conv-{}", Uuid::new_v4().simple());
    let conversation = persistence::ensure_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        Some(user_id),
        &external_id,
        None,
        None,
    )
    .await?;
    bindings::bind_conversation_hat(state.sqlx_pool(), bear_id, conversation.id, hat_id).await?;
    Ok(Redirect::to(&format!(
        "/bear/{}?conversation_id={external_id}",
        bear.slug
    ))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct BindForm {
    hat_id: Uuid,
}

async fn bind_conversation(
    Path((slug, conversation_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<BindForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    bindings::bind_conversation_hat(
        state.sqlx_pool(),
        BearId::new(bear.id),
        conversation_id,
        HatId::new(form.hat_id),
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/conversations/{conversation_id}",
        bear.slug
    ))
    .into_response())
}
