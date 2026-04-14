// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! End-user JSON + SSE under `/v1/*` (session cookie, same origin as Deep Chat).

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};
use axum_login::login_required;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth_backend::{AuthSession, Backend},
    core::bears::db as bears_db,
    errors::CustomError,
    web::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bears", get(list_my_bears))
        .route("/chat/send", post(chat_send))
        .route_layer(login_required!(Backend, login_url = "/login"))
}

/// Membership-filtered bears for the chat UI (no Letta agent id exposed).
#[derive(Serialize)]
pub struct BearPublic {
    pub bear_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
}

async fn list_my_bears(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Json<Vec<BearPublic>>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let rows = bears_db::list_bears_for_user(state.sqlx_pool(), user_id).await?;
    let out: Vec<BearPublic> = rows
        .into_iter()
        .map(|b| BearPublic {
            bear_id: b.id,
            slug: b.slug,
            name: b.name,
            description: b.description,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ChatSendRequest {
    pub bear_id: Uuid,
    pub message: String,
    /// Reserved for Letta threading / OTID pass-through (optional).
    #[serde(default)]
    pub conversation_id: Option<String>,
}

async fn chat_send(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(body): Json<ChatSendRequest>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    if body.message.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "message must not be empty".to_string(),
        ));
    }

    if !state.letta.is_enabled() {
        return Err(CustomError::System(
            "Chat is unavailable: LETTA_BASE_URL is not set".to_string(),
        ));
    }

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, body.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let agent_id = bear
        .letta_agent_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::System(
                "This bear is not provisioned in Letta yet (missing letta_agent_id).".to_string(),
            )
        })?;

    if body.conversation_id.is_some() {
        tracing::debug!("conversation_id accepted but not yet forwarded to Letta in this build");
    }

    let upstream = state
        .letta
        .post_messages_streaming(agent_id, body.message.trim())
        .await?;

    let stream = upstream.bytes_stream().map(|res| {
        res.map_err(|e| std::io::Error::other(e.to_string()))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .map_err(|e| CustomError::System(format!("response build: {e}")))
}
