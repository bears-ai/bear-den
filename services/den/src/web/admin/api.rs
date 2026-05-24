// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! JSON admin API (same operator session as HTML console; not for browser JS with API keys).

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::{
    core::{
        bears::{
            db::{self as bears_db, BearParams, MembershipRow},
            model::Bear,
            provision,
        },
        user::db as user_db,
    },
    errors::CustomError,
    web::AppState,
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
    /// Deprecated: prefer `letta_tool_ids`. When set, stored as `bears.tools_enabled` for backward compatibility.
    tools_enabled: Option<serde_json::Value>,
    letta_agent_type: Option<String>,
    #[serde(default)]
    letta_tool_ids: Vec<String>,
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
    let tools = body.tools_enabled.map(SqlxJson);
    let letta_tool_ids: Vec<String> = body
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let letta_agent_type = body
        .letta_agent_type
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let id = bears_db::create_bear(
        state.sqlx_pool(),
        BearParams {
            slug,
            name: body.name.trim(),
            description: body.description.trim(),
            system_prompt: body.system_prompt.trim(),
            default_model: body
                .default_model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            tools_enabled: tools,
            letta_agent_type: letta_agent_type.as_deref(),
            letta_tool_ids: SqlxJson(letta_tool_ids),
            context_profile: None,
        },
    )
    .await?;

    if let Err(e) = provision::provision_bear_if_configured(
        state.sqlx_pool(),
        state.letta.as_ref(),
        state.bifrost.as_ref(),
        id,
    )
    .await
    {
        tracing::warn!(%id, "Letta provision failed after admin API create: {e}");
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
