//! Model inspection is not an execution grant or conversation creation path.

use super::{checked_chat_id, normalize_client_conversation_id, ChatApiError};
use crate::{
    auth_backend::AuthSession, bear::settings::model_configurations::selectable_model_options,
    errors::CustomError, web::AppState,
};
use axum::{
    extract::{rejection::JsonRejection, State},
    Json,
};
use axum_extra::extract::{Query, QueryRejection};
use den_core::{
    ids::{BearId, ModelConfigurationId, UserId},
    DenError, ThinkingEffort,
};
use den_llm::ModelOption;
use den_service::{
    archived_conversations,
    bears::{
        hats,
        model_configurations::{self, PrimaryModelSource, ResolvedPrimaryModel},
    },
    conversation::persistence as conversation_persistence,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ChatModelQuery {
    pub bear_id: Uuid,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatModelPatchBody {
    pub bear_id: Uuid,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub selection_mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct ChatModelResponse {
    pub selection_mode: String,
    pub requested_model: Option<String>,
    pub selected_model: Option<String>,
    pub effective_model: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub configuration_id: Option<ModelConfigurationId>,
    pub configuration_name: Option<String>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub model_options: Vec<ModelOption>,
}

impl ChatModelResponse {
    pub(super) fn from_primary(
        primary: ResolvedPrimaryModel,
        model_options: Vec<ModelOption>,
    ) -> Self {
        let pinned = primary.source == PrimaryModelSource::ConversationPin;
        Self {
            selection_mode: if pinned { "explicit" } else { "auto" }.to_string(),
            requested_model: pinned.then(|| primary.model_handle.clone()),
            selected_model: pinned.then(|| primary.model_handle.clone()),
            effective_model: Some(primary.model_handle),
            source: Some(
                match primary.source {
                    PrimaryModelSource::ConversationPin => "conversation_explicit",
                    PrimaryModelSource::HatOverride => "hat_override",
                    PrimaryModelSource::BearDefault => "bear_default",
                    PrimaryModelSource::DeploymentDefault => "deployment_default",
                }
                .to_string(),
            ),
            error: None,
            configuration_id: primary.configuration_id,
            configuration_name: primary.configuration_name,
            thinking_effort: primary.thinking_effort,
            model_options,
        }
    }

    pub(super) fn from_resolution(
        primary: Result<ResolvedPrimaryModel, DenError>,
        model_state: Option<conversation_persistence::ConversationModelState>,
        model_options: Vec<ModelOption>,
    ) -> Result<Self, ChatApiError> {
        match primary {
            Ok(primary) => Ok(Self::from_primary(primary, model_options)),
            Err(DenError::ValidationError(_)) => {
                // Persisted selectors are a legacy boundary, not execution state.
                let pin = model_state.filter(|state| state.selection_mode == "explicit");
                let pinned = pin.is_some();
                Ok(Self {
                    selection_mode: if pinned { "explicit" } else { "auto" }.to_string(),
                    requested_model: pin.as_ref().and_then(|state| state.requested_model.clone()),
                    selected_model: pin.as_ref().and_then(|state| state.selected_model.clone()),
                    effective_model: None,
                    source: pinned.then(|| "conversation_explicit".into()),
                    error: Some(
                        "The configured model is unavailable or no longer selectable.".into(),
                    ),
                    configuration_id: None,
                    configuration_name: None,
                    thinking_effort: None,
                    model_options,
                })
            }
            Err(error) => Err(error.into()),
        }
    }
}

async fn response_for(
    state: &AppState,
    user_id: i32,
    bear_id: Uuid,
    conversation_id: Option<&str>,
) -> Result<ChatModelResponse, ChatApiError> {
    let requested_id = normalize_client_conversation_id(conversation_id)?;
    let (_, conv_id) = checked_chat_id(state.sqlx_pool(), bear_id, user_id, &requested_id).await?;
    let model_options = selectable_model_options(state.sqlx_pool()).await?;
    let conversation = if conv_id.starts_with("new-") {
        None
    } else {
        conversation_persistence::get_conversation_for_external_id(
            state.sqlx_pool(),
            bear_id,
            &conv_id,
        )
        .await?
    };
    let bound = match conversation.as_ref() {
        Some(conversation) => hats::bindings::conversation_hat(
            state.sqlx_pool(),
            BearId::new(bear_id),
            conversation.id,
        )
        .await?
        .is_some(),
        None => false,
    };
    let (primary, model_state) = if let Some(conversation) = conversation.filter(|_| bound) {
        // Keep the verified hat and full pin/hat/Bear/deployment chain for bound sources.
        let model_state = conversation_persistence::get_conversation_model_state(
            state.sqlx_pool(),
            conversation.id,
        )
        .await?;
        (
            den_service::model_selection::resolve_conversation_primary_model(
                state.sqlx_pool(),
                BearId::new(bear_id),
                conversation.id,
                &state.config.default_llm_model,
            )
            .await,
            model_state,
        )
    } else {
        // Pending, missing and legacy unbound rows only preview Bear configuration.
        // Never infer a hat from available choices or honor an unbound historical pin.
        (
            model_configurations::resolve_primary(
                state.sqlx_pool(),
                BearId::new(bear_id),
                None,
                None,
                &state.config.default_llm_model,
            )
            .await,
            None,
        )
    };
    ChatModelResponse::from_resolution(primary, model_state, model_options)
}

pub(super) async fn chat_model_get(
    State(state): State<AppState>,
    auth_session: AuthSession,
    query: Result<Query<ChatModelQuery>, QueryRejection>,
) -> Result<Json<ChatModelResponse>, ChatApiError> {
    let Query(q) = query?;
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    Ok(Json(
        response_for(&state, user_id, q.bear_id, q.conversation_id.as_deref()).await?,
    ))
}

pub(super) async fn chat_model_patch(
    State(state): State<AppState>,
    auth_session: AuthSession,
    body: Result<Json<ChatModelPatchBody>, JsonRejection>,
) -> Result<Json<ChatModelResponse>, ChatApiError> {
    let Json(body) = body?;
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    let requested_id = normalize_client_conversation_id(body.conversation_id.as_deref())?;
    let (_, conv_id) =
        checked_chat_id(state.sqlx_pool(), body.bear_id, user_id, &requested_id).await?;
    if requested_id.starts_with("new-") {
        return Err(ChatApiError::ConversationReadOnly);
    }
    let conversation = conversation_persistence::get_conversation_for_external_id(
        state.sqlx_pool(),
        body.bear_id,
        &conv_id,
    )
    .await?
    .ok_or(ChatApiError::ConversationReadOnly)?;
    if archived_conversations::list_for_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .contains(&conv_id)
    {
        return Err(ChatApiError::ConversationArchived);
    }
    // Admin transcript visibility is not write authority. Check current owner,
    // membership, active status and the real hat before any model-state write.
    den_service::conversation::viewer::require_ordinary_tool_source(
        state.sqlx_pool(),
        BearId::new(body.bear_id),
        UserId::new(user_id),
        &conv_id,
    )
    .await
    .map_err(ChatApiError::ordinary_source)?;
    den_service::model_selection::apply_conversation_model_selection(
        state.sqlx_pool(),
        conversation.id,
        body.selection_mode.as_deref().unwrap_or("auto").trim(),
        body.model.as_deref(),
        "human_selected",
        "inherit_hat_bear_or_deployment_default",
    )
    .await?;
    Ok(Json(
        response_for(&state, user_id, body.bear_id, Some(&conv_id)).await?,
    ))
}
