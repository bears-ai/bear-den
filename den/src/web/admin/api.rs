// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! JSON admin API (same operator session as HTML console; not for browser JS with API keys).

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::{
    core::{
        bears::{
            db::{self as bears_db, MembershipRow},
            model::{Bear, BearTemplate},
        },
        user::db as user_db,
    },
    errors::CustomError,
    web::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bear-templates", get(list_templates).post(create_template))
        .route("/bear-templates/{id}", get(get_template))
        .route("/bears", get(list_bears).post(create_bear))
        .route("/bears/{id}", get(get_bear))
        .route("/membership", get(list_membership).post(grant_membership))
}

async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<BearTemplate>>, CustomError> {
    let v = bears_db::list_templates(state.sqlx_pool()).await?;
    Ok(Json(v))
}

async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<BearTemplate>, CustomError> {
    let t = bears_db::get_template(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("template not found".to_string()))?;
    Ok(Json(t))
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateRequest {
    slug: String,
    name: String,
    description: String,
    system_prompt: String,
    default_model: Option<String>,
    tools_enabled: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct IdResponse {
    id: Uuid,
}

async fn create_template(
    State(state): State<AppState>,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<(axum::http::StatusCode, Json<IdResponse>), CustomError> {
    let slug = body.slug.trim();
    if slug.is_empty() {
        return Err(CustomError::ValidationError("slug is required".to_string()));
    }
    if bears_db::template_slug_exists(state.sqlx_pool(), slug).await? {
        return Err(CustomError::ValidationError(
            "template slug already exists".to_string(),
        ));
    }
    let tools = body.tools_enabled.map(SqlxJson);
    let id = bears_db::create_template(
        state.sqlx_pool(),
        slug,
        body.name.trim(),
        body.description.trim(),
        body.system_prompt.trim(),
        body.default_model.as_deref().filter(|s| !s.trim().is_empty()),
        tools,
    )
    .await?;
    Ok((axum::http::StatusCode::CREATED, Json(IdResponse { id })))
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
    template_id: Uuid,
    slug: String,
    name: String,
    description: String,
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
    let id = bears_db::create_bear_from_template(
        state.sqlx_pool(),
        body.template_id,
        slug,
        body.name.trim(),
        body.description.trim(),
    )
    .await?;
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
    bears_db::grant_membership(
        state.sqlx_pool(),
        body.user_id,
        body.bear_id,
        role,
    )
    .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
