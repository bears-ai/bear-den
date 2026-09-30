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

mod core_review;
mod legacy_instructions;
mod review;
mod work_review;

#[cfg(test)]
mod tests;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(review::router())
        .merge(core_review::router())
        .merge(work_review::router())
        .route_with_tsr("/bear/{slug}/hats", get(index).post(create))
        .route_with_tsr("/bear/{slug}/hats/{hat_id}", get(detail).post(update))
        .route_with_tsr(
            "/bear/{slug}/hats/{hat_id}/ide-default",
            post(set_ide_default),
        )
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
struct HatUpdateForm {
    name: String,
    purpose: String,
    identity_prompt: String,
    #[serde(default)]
    confirm_work_audience: bool,
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
    #[serde(default)]
    expected_identity_sha256: String,
    #[serde(default)]
    confirm_identity_audience: bool,
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
    let bear_id = BearId::new(bear.id);
    let hats = hats::list_hats(state.sqlx_pool(), bear_id).await?;
    let ide_default_hat_id = hats::ide_default_hat(state.sqlx_pool(), bear_id)
        .await?
        .map(|id| id.to_string());
    web::render_template(
        &state,
        "bear/manage/hats.jinja",
        auth,
        context! {
            hats, ide_default_hat_id, can_manage_bear => true, native_runtime => true,
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
    let identity_preview = hats::identity::render_hat_identity_component(&bear, &hat)?;
    let identity_sha256 = hats::identity::identity_fingerprint(&hat.name, &hat.identity_prompt);
    let previous_instructions = legacy_instructions::for_bear(state.sqlx_pool(), &bear).await?;
    let ide_default_hat_id = hats::ide_default_hat(state.sqlx_pool(), bear_id).await?;
    let is_ide_default = ide_default_hat_id == Some(hat_id);
    let ide_default_hat_name = hats::list_hats(state.sqlx_pool(), bear_id)
        .await?
        .into_iter()
        .find(|candidate| Some(candidate.id) == ide_default_hat_id)
        .map(|candidate| candidate.name);
    let granted = manage::allowed_surfaces(state.sqlx_pool(), bear_id, hat_id).await?;
    let memory = state.memory_stores.store_for_bear(bear.id).await?;
    let historical_hat_records = den_memory::hat_review::snapshot_for_hat(&memory, hat_id)
        .await?
        .total_records;
    let work_reviews = hats::work_review::list_receipts(
        state.sqlx_pool(),
        bear_id,
        hat_id,
        UserId::new(session_user(&auth).await?.id),
    )
    .await?;
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
            hat, identity_preview, identity_sha256, previous_instructions, is_ide_default, ide_default_hat_name, choices, grant_count => granted.len(), historical_hat_records, work_reviews, message => query.message,
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
    Form(form): Form<HatUpdateForm>,
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
        &form.identity_prompt,
        form.confirm_work_audience,
    )
    .await?;
    Ok(Redirect::to(&hat_url(&bear.slug, id)).into_response())
}

async fn set_ide_default(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let id = HatId::new(hat_id);
    hats::set_ide_default_hat(state.sqlx_pool(), BearId::new(bear.id), id).await?;
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
            if form.confirmation.trim() != "enable work" || !form.confirm_identity_audience {
                return Err(CustomError::ValidationError(
                    "confirm the hat identity and autonomous Work audience".into(),
                ));
            }
            manage::enable_work_if_empty(
                state.sqlx_pool(),
                &state.memory_stores,
                bear_id,
                id,
                &form.expected_identity_sha256,
            )
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
