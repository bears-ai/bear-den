// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! JSON admin API (same operator session as HTML console; not for browser JS with API keys).

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    core::user::db as user_db,
    errors::CustomError,
    web::{bear::create_support, AppState},
};
use den_service::bears::{
    db::{self as bears_db, BearParams, MembershipRow},
    model::Bear,
    provision,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bears", get(list_bears).post(create_bear))
        .route("/bears/{id}", get(get_bear))
        .route("/membership", get(list_membership).post(grant_membership))
}

async fn list_bears(State(state): State<AppState>) -> Result<Json<Vec<Bear>>, CustomError> {
    let v = bears_db::list_bears(state.sqlx_pool()).await?;
    Ok(Json(v))
}

async fn get_bear(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Bear>, CustomError> {
    let b = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    Ok(Json(b))
}

#[derive(Debug, Deserialize)]
pub struct CreateBearRequest {
    slug: String,
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,
    /// Deprecated legacy payload; ignored by Den-native provisioning.
    tools_enabled: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct IdResponse {
    id: Uuid,
}

async fn create_bear(
    State(state): State<AppState>,
    Json(body): Json<CreateBearRequest>,
) -> Result<(axum::http::StatusCode, Json<IdResponse>), CustomError> {
    let slug = body.slug.trim();
    if slug.is_empty() {
        return Err(CustomError::ValidationError("slug is required".to_string()));
    }
    if bears_db::bear_slug_exists(state.sqlx_pool(), slug).await? {
        return Err(CustomError::ValidationError(
            "bear slug already exists".to_string(),
        ));
    }
    let default_model = body
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(model) = default_model {
        let catalog_models =
            den_service::model_selection::list_selectable_model_options(state.sqlx_pool()).await?;
        if catalog_models.is_empty() {
            return Err(CustomError::ValidationError(
                "No Den model selection options are configured; cannot validate default_model"
                    .to_string(),
            ));
        }
        if !create_support::default_model_available_in_catalog(&catalog_models, model) {
            return Err(CustomError::ValidationError(format!(
                "default_model `{model}` is not configured as a selectable Den model"
            )));
        }
    }
    let _ = body.tools_enabled;
    let id = bears_db::create_bear(
        state.sqlx_pool(),
        BearParams {
            slug,
            name: body.name.trim(),
            description: body.description.trim(),
            system_prompt: body.system_prompt.trim(),
            default_model,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;

    if let Err(e) = create_support::provision_bifrost_virtual_key_for_bear(&state, id, slug).await {
        let _ = bears_db::delete_bear(state.sqlx_pool(), id).await;
        return Err(CustomError::System(format!(
            "Bifrost virtual key provisioning failed: {e}"
        )));
    }

    if let Err(e) =
        provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await
    {
        tracing::warn!(%id, "Bear initialization failed after admin API create: {e}");
    }

    Ok((axum::http::StatusCode::CREATED, Json(IdResponse { id })))
}

async fn list_membership(
    State(state): State<AppState>,
) -> Result<Json<Vec<MembershipRow>>, CustomError> {
    let v = bears_db::list_memberships(state.sqlx_pool()).await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct GrantMembershipRequest {
    user_id: i32,
    bear_id: Uuid,
    role: Option<String>,
}

async fn grant_membership(
    State(state): State<AppState>,
    Json(body): Json<GrantMembershipRequest>,
) -> Result<axum::http::StatusCode, CustomError> {
    if user_db::get_user_by_id(state.sqlx_pool(), body.user_id)
        .await?
        .is_none()
    {
        return Err(CustomError::NotFound("user not found".to_string()));
    }
    if bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .is_none()
    {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    bears_db::grant_membership(state.sqlx_pool(), body.user_id, body.bear_id, role).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
