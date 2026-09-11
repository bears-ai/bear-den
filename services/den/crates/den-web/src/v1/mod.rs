// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
//! End-user JSON + SSE under `/v1/*` (session cookie, same origin as Deep Chat).

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use axum_extra::extract::Query;
use axum_login::login_required;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::Instrument;
use uuid::Uuid;

use crate::web::bear::create_support::model_catalog_select_context;
use crate::{
    auth_backend::{AuthSession, Backend},
    errors::CustomError,
    observability::{
        chat_proxy_stream::{deep_chat_sse_body_for_assistant_text, BearChannelSseProxyStream},
        native_web_chat_stream::NativeWebChatUpstreamStream,
    },
    web::AppState,
    web_chat_runtime::WebChatRuntimeRequest,
};
use den_docket::{
    DocketEffortHint, DocketService, DocketTaskCreate, DocketTaskDifficulty, DocketTaskKind,
    DocketTaskListFilter, DocketTaskScope, PgDocketService, RoutingStrategy,
};
use den_llm::ModelOption;
use den_protocol::ContextBudgetReport;
use den_runtime::current_task::{
    preview_session_current_task_selection, select_session_current_task,
};
use den_service::archived_conversations;
use den_service::{
    artifacts::{self, ArtifactAccessContext},
    bears::{
        db::{self as bears_db, role_is_bear_admin},
        BearProfile,
    },
    client_sessions,
    conversation::persistence as conversation_persistence,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bears", get(list_my_bears))
        .route("/chat/conversations", get(chat_conversations))
        .route(
            "/chat/conversations/{conversation_id}",
            patch(chat_conversation_patch),
        )
        .route("/chat/history", get(chat_history))
        .route("/chat/artifacts", get(chat_artifacts))
        .route("/chat/model", get(chat_model_get).patch(chat_model_patch))
        .route("/chat/current-task", get(chat_current_task_get))
        .route("/chat/current-task", post(chat_current_task_create))
        .route(
            "/chat/current-task/selection-request",
            post(chat_current_task_selection_request),
        )
        .route("/chat/current-task/select", post(chat_current_task_select))
        .route("/chat/current-task/clear", post(chat_current_task_clear))
        .route("/chat/send", post(chat_send))
        .route_layer(login_required!(Backend, login_url = "/login"))
}

/// Membership-filtered bears for the chat UI (no provider ids exposed).
#[derive(Serialize)]
pub struct BearPublic {
    pub bear_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `user_bear.role == "admin"` for this user (bear admin, not site operator).
    pub is_bear_admin: bool,
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
        .map(|row| BearPublic {
            bear_id: row.bear.id,
            slug: row.bear.slug,
            name: row.bear.name,
            description: row.bear.description,
            is_bear_admin: role_is_bear_admin(row.membership_role.as_deref()),
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryQuery {
    pub bear_id: Uuid,
    /// Runtime conversation: `default`, interactive `conv-…`, or BearWire/headless `den-conv-…`.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Canonical cursor: messages older than this sequence number.
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub debug: bool,
}

#[derive(Debug, Deserialize)]
struct ChatArtifactsQuery {
    bear_id: Uuid,
    #[serde(default)]
    conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationsQuery {
    pub bear_id: Uuid,
}

#[derive(Serialize)]
pub struct ChatConversationRow {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_budget: Option<ContextBudgetReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_budget_updated_at: Option<String>,
}

#[derive(Serialize)]
pub struct ChatConversationsResponse {
    pub conversations: Vec<ChatConversationRow>,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationPatchBody {
    pub bear_id: Uuid,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub deleted: Option<bool>,
}

#[derive(Serialize)]
pub struct ChatConversationPatchResponse {
    pub ok: bool,
}

#[derive(Serialize)]
pub struct ChatHistoryMessage {
    #[serde(default = "chat_history_message_kind")]
    pub kind: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Typed runtime-card detail for replay; compact labels remain renderer-owned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<Value>,
}

fn chat_history_message_kind() -> String {
    "message".to_string()
}

#[derive(Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatHistoryMessage>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_budget: Option<ContextBudgetReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_budget_updated_at: Option<String>,
}

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
    pub effective_model: String,
    pub source: String,
    pub model_options: Vec<ModelOption>,
}

/// `None` / empty / `default` → agent main conversation. Existing runtime conversations use
/// interactive `conv-...` or BearWire/headless `den-conv-...` ids. The web UI may also send a
/// temporary `new-...` placeholder before Den resolves the durable conversation id.
fn normalize_client_conversation_id(raw: Option<&str>) -> Result<String, CustomError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    if s == "default" {
        return Ok("default".to_string());
    }
    let ok = (s.starts_with("conv-") || s.starts_with("den-conv-") || s.starts_with("new-"))
        && s.len() > 8
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(CustomError::ValidationError(format!(
            "invalid conversation_id (expected 'default', a runtime conv-/den-conv- id, or a pending new- id): {s}"
        )))
    }
}

#[derive(Debug, Deserialize)]
struct ChatCurrentTaskQuery {
    bear_id: Uuid,
    #[serde(default)]
    conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCurrentTaskMutation {
    bear_id: Uuid,
    conversation_id: String,
    #[serde(default)]
    task_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct ChatCurrentTaskCreate {
    bear_id: Uuid,
    conversation_id: String,
    title: String,
}

fn browser_client_session_id(user_id: i32, bear_id: Uuid, conversation_id: &str) -> String {
    format!("den-web:{user_id}:{bear_id}:{conversation_id}")
}

fn browser_session_policy() -> den_core::EffectivePolicy {
    den_core::EffectivePolicy::compile(
        den_core::TrustProfile::Pair,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Absent,
    )
}

async fn browser_client_session(
    state: &AppState,
    user_id: i32,
    bear: &den_service::bears::Bear,
    conversation_id: &str,
) -> Result<den_service::client_sessions::ClientSessionRow, CustomError> {
    if conversation_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "choose a task after the conversation is created".to_string(),
        ));
    }
    let session_id = browser_client_session_id(user_id, bear.id, conversation_id);
    client_sessions::upsert_session(
        state.sqlx_pool(),
        client_sessions::UpsertClientSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: session_id.clone(),
            conversation_id: conversation_id.to_string(),
            resolved_conversation_id: None,
            client: "den-web".to_string(),
            cwd: None,
            current_mode: Some(client_sessions::ClientSessionMode::Ask),
        },
    )
    .await?;
    client_sessions::find_for_user_bear_session_id(state.sqlx_pool(), user_id, bear.id, &session_id)
        .await?
        .ok_or_else(|| CustomError::System("browser client session was not persisted".to_string()))
}

async fn current_task_bear(
    state: &AppState,
    user_id: i32,
    bear_id: Uuid,
) -> Result<den_service::bears::Bear, CustomError> {
    if !bears_db::user_may_use_bear(state.sqlx_pool(), user_id, bear_id).await? {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }
    let bear = bears_db::get_bear(state.sqlx_pool(), bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    if !bear.work_enabled {
        return Err(CustomError::ValidationError(
            "focused task controls are disabled".to_string(),
        ));
    }
    Ok(bear)
}

async fn chat_current_task_get(
    State(state): State<AppState>,
    auth: AuthSession,
    Query(q): Query<ChatCurrentTaskQuery>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let conversation_id = normalize_client_conversation_id(q.conversation_id.as_deref())?;
    let bear = current_task_bear(&state, user_id, q.bear_id).await?;
    let session = browser_client_session(&state, user_id, &bear, &conversation_id).await?;
    let tasks = PgDocketService::from_pool(state.sqlx_pool())
        .list_tasks(
            bear.id,
            DocketTaskListFilter {
                job_id: None,
                pair_session_id: Some(session.id),
                parent_task_id: None,
                include_descendants: false,
                limit: 500,
            },
        )
        .await?;
    Ok(Json(json!({
        "session_id": session.client_session_id,
        "current_task_id": session.current_task_id,
        "tasks": tasks,
    })))
}

async fn chat_current_task_create(
    State(state): State<AppState>,
    auth: AuthSession,
    Json(body): Json<ChatCurrentTaskCreate>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let conversation_id = normalize_client_conversation_id(Some(&body.conversation_id))?;
    if conversation_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "create a task after the conversation is created".to_string(),
        ));
    }
    let title = body.title.trim();
    if title.is_empty() {
        return Err(CustomError::ValidationError(
            "task title is required".to_string(),
        ));
    }
    let bear = current_task_bear(&state, user_id, body.bear_id).await?;
    let session = browser_client_session(&state, user_id, &bear, &conversation_id).await?;
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let task = service
        .create_task(DocketTaskCreate {
            bear_id: bear.id,
            job_id: None,
            pair_session_id: Some(session.id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: title.to_string(),
            body: title.to_string(),
            completion_criteria: vec!["Complete the task".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await?;
    Ok(Json(json!({ "task": task })))
}

async fn chat_current_task_selection_request(
    State(state): State<AppState>,
    auth: AuthSession,
    Json(body): Json<ChatCurrentTaskMutation>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let conversation_id = normalize_client_conversation_id(Some(&body.conversation_id))?;
    let task_id = body
        .task_id
        .ok_or_else(|| CustomError::ValidationError("task_id is required".to_string()))?;
    let bear = current_task_bear(&state, user_id, body.bear_id).await?;
    let session = browser_client_session(&state, user_id, &bear, &conversation_id).await?;
    let title = preview_session_current_task_selection(
        state.sqlx_pool(),
        user_id,
        bear.id,
        &session.client_session_id,
        task_id,
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "confirmation_required": true,
        "task_id": task_id,
        "title": title,
    })))
}

async fn chat_current_task_select(
    State(state): State<AppState>,
    auth: AuthSession,
    Json(body): Json<ChatCurrentTaskMutation>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let conversation_id = normalize_client_conversation_id(Some(&body.conversation_id))?;
    let task_id = body
        .task_id
        .ok_or_else(|| CustomError::ValidationError("task_id is required".to_string()))?;
    let bear = current_task_bear(&state, user_id, body.bear_id).await?;
    let session = browser_client_session(&state, user_id, &bear, &conversation_id).await?;
    let policy = browser_session_policy();
    let result = select_session_current_task(
        state.sqlx_pool(),
        user_id,
        bear.id,
        &session.client_session_id,
        Some(task_id),
        &policy.capabilities,
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "current_task_id": task_id,
        "title": result.title,
        "task_list": result.task_list,
    })))
}

async fn chat_current_task_clear(
    State(state): State<AppState>,
    auth: AuthSession,
    Json(body): Json<ChatCurrentTaskMutation>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let conversation_id = normalize_client_conversation_id(Some(&body.conversation_id))?;
    let bear = current_task_bear(&state, user_id, body.bear_id).await?;
    let session = browser_client_session(&state, user_id, &bear, &conversation_id).await?;
    let policy = browser_session_policy();
    let result = select_session_current_task(
        state.sqlx_pool(),
        user_id,
        bear.id,
        &session.client_session_id,
        None,
        &policy.capabilities,
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "current_task_id": Value::Null,
        "task_list": result.task_list,
    })))
}

async fn chat_conversations(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatConversationsQuery>,
) -> Result<Json<ChatConversationsResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, q.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), q.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let default_row = || ChatConversationRow {
        id: "default".to_string(),
        title: "Main chat".to_string(),
        last_message_at: None,
        latest_context_budget: None,
        latest_context_budget_updated_at: None,
    };

    let archived_ids = archived_conversations::list_for_bear(state.sqlx_pool(), bear.id).await?;
    let mut conversations =
        conversation_persistence::list_conversations_for_bear(state.sqlx_pool(), bear.id, 100)
            .await?
            .into_iter()
            .filter_map(|row| {
                let id = row.external_conversation_id?;
                if id.starts_with("new-") || archived_ids.contains(&id) {
                    return None;
                }
                Some(ChatConversationRow {
                    id: id.clone(),
                    title: row
                        .current_title
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| {
                            if id == "default" {
                                "Main chat".to_string()
                            } else {
                                id.clone()
                            }
                        }),
                    last_message_at: Some(
                        row.updated_at
                            .format(&time::format_description::well_known::Rfc3339)
                            .ok()?,
                    ),
                    latest_context_budget: row.latest_context_budget,
                    latest_context_budget_updated_at: row
                        .latest_context_budget_updated_at
                        .and_then(|value| {
                            value
                                .format(&time::format_description::well_known::Rfc3339)
                                .ok()
                        }),
                })
            })
            .collect::<Vec<_>>();

    if !conversations.iter().any(|row| row.id == "default") {
        conversations.insert(0, default_row());
    }

    Ok(Json(ChatConversationsResponse { conversations }))
}

async fn chat_conversation_patch(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(conversation_id): Path<String>,
    Json(body): Json<ChatConversationPatchBody>,
) -> Result<Json<ChatConversationPatchResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, body.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let conv_id = normalize_client_conversation_id(Some(&conversation_id))?;
    if conv_id == "default" || conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "only saved conversations can be renamed or archived".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    conversation_persistence::get_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        &conv_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("conversation not found".to_string()))?;

    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if body.title.is_some() && title.is_none() {
        return Err(CustomError::ValidationError(
            "conversation title cannot be empty".to_string(),
        ));
    }
    if body.title.is_none() && body.archived.is_none() && body.deleted != Some(true) {
        return Err(CustomError::ValidationError(
            "no conversation update requested".to_string(),
        ));
    }

    if body.deleted == Some(true) {
        conversation_persistence::delete_conversation_and_clear_archive(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            Some(user_id),
            "delete",
        )
        .await?;
        return Ok(Json(ChatConversationPatchResponse { ok: true }));
    }

    if let Some(title) = title {
        let title = title.chars().take(120).collect::<String>();
        let _ = conversation_persistence::set_conversation_title_and_sync_client_sessions(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            &title,
        )
        .await?;
    }

    if let Some(archived) = body.archived {
        archived_conversations::set_archived(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
            Some(user_id),
            "web",
            archived,
        )
        .await?;
    }

    Ok(Json(ChatConversationPatchResponse { ok: true }))
}

async fn chat_history(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistoryResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;

    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, q.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }

    let bear = bears_db::get_bear(state.sqlx_pool(), q.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let empty = || {
        Json(ChatHistoryResponse {
            messages: vec![],
            has_more: false,
            next_before: None,
            latest_context_budget: None,
            latest_context_budget_updated_at: None,
        })
    };

    let limit = i64::from(q.limit.unwrap_or(50).clamp(1, 100));
    let before_sequence_no = q
        .before
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| {
            CustomError::ValidationError("before must be a canonical sequence number".to_string())
        })?;

    let conv_id = normalize_client_conversation_id(q.conversation_id.as_deref())?;

    let Some(conversation) = conversation_persistence::get_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        &conv_id,
    )
    .await?
    else {
        return Ok(empty());
    };

    let rows = conversation_persistence::list_projected_messages_page(
        state.sqlx_pool(),
        conversation.id,
        before_sequence_no,
        limit,
        conversation_persistence::ConversationHistoryProjection::UserHistory,
    )
    .await?;
    let (mut messages, has_more, next_before) = map_persisted_history_page(&rows, limit as usize);

    if before_sequence_no.is_none() {
        if let Some(session) = den_service::client_sessions::find_latest_for_bear_conversation(
            state.sqlx_pool(),
            bear.id,
            &conv_id,
        )
        .await?
        {
            let is_work_session = den_docket::work_runs::get_work_run_by_session(
                state.sqlx_pool(),
                &session.client_session_id,
            )
            .await?
            .is_some();
            if is_work_session {
                // ponytail: conversation rows and BearWire events have independent cursors. Keep
                // the bounded activity record on the newest page; use a unified cursor if a run
                // can exceed the store's 501-event replay ceiling.
                let event_rows = den_runtime::bearwire_events::list_bearwire_events_after(
                    state.sqlx_pool(),
                    &session.client_session_id,
                    None,
                    501,
                )
                .await?;
                messages.extend(
                    den_runtime::work_activity::project_work_activity(event_rows)
                        .into_iter()
                        .filter_map(chat_history_work_activity),
                );
                messages.sort_by(|left, right| left.created_at.cmp(&right.created_at));
            }
        }
    }

    Ok(Json(ChatHistoryResponse {
        messages,
        has_more,
        next_before,
        latest_context_budget: conversation.latest_context_budget,
        latest_context_budget_updated_at: conversation.latest_context_budget_updated_at.and_then(
            |value| {
                value
                    .format(&time::format_description::well_known::Rfc3339)
                    .ok()
            },
        ),
    }))
}

/// Access-filtered artifact citations for one durable chat conversation.
///
/// This deliberately exposes citations only: storage locations, hashes, and
/// provenance stay inside the artifact registry.
async fn chat_artifacts(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatArtifactsQuery>,
) -> Result<Json<Value>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if !bears_db::user_may_use_bear(state.sqlx_pool(), user_id, q.bear_id).await? {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }
    let conversation_id = normalize_client_conversation_id(q.conversation_id.as_deref())?;
    if conversation_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "conversation artifacts are available after the conversation is created".to_string(),
        ));
    }
    let citations = artifacts::list_conversation_artifact_citations(
        state.sqlx_pool(),
        q.bear_id,
        &conversation_id,
        ArtifactAccessContext {
            bear_id: q.bear_id,
            user_id: Some(user_id),
            profile: BearProfile::Pair,
        },
    )
    .await?;
    Ok(Json(json!({
        "conversation_id": conversation_id,
        "artifacts": citations,
    })))
}

/// Deep Chat history expects `ai`; Postgres stores `assistant`.
fn client_chat_history_role(storage_role: &str) -> String {
    match storage_role {
        "assistant" => "ai".to_string(),
        other => other.to_string(),
    }
}

fn map_persisted_history_page(
    rows: &[conversation_persistence::PersistedConversationMessage],
    page_limit: usize,
) -> (Vec<ChatHistoryMessage>, bool, Option<String>) {
    let visible_rows: Vec<_> = rows
        .iter()
        .filter_map(|row| row.to_user_history_record().map(|message| (row, message)))
        .collect();

    let mut coalesced_desc: Vec<(i64, ChatHistoryMessage)> = Vec::new();
    for (row, message) in visible_rows {
        let storage_role = message.role.clone();
        if let Some((_, last)) = coalesced_desc.last_mut() {
            if message.kind == "message"
                && last.kind == "message"
                && last.role == client_chat_history_role(&storage_role)
                && storage_role == "assistant"
                && matches!(
                    row.storage_message_type(),
                    Ok(den_service::conversation::message_types::ConversationMessageType::Assistant)
                )
            {
                last.text.push_str(&message.content);
                last.text =
                    crate::observability::chat_proxy_stream::strip_ephemeral_status_suffixes(
                        &last.text,
                    );
                continue;
            }
        }
        let text = crate::observability::chat_proxy_stream::strip_ephemeral_status_suffixes(
            &message.content,
        );
        if text.is_empty() && message.kind == "message" {
            continue;
        }
        let event = match message.kind.as_str() {
            "tool_call" => Some(json!({
                "card_kind": "tool_activity",
                "label": format!("Run {}", message.tool_name.as_deref().unwrap_or("tool")),
                "source": "den_runtime",
                "tool": {
                    "id": message.tool_call_id.clone(),
                    "name": message.tool_name.clone(),
                    "status": message.status.clone(),
                    "arguments": message.arguments.clone(),
                },
                "delivery": { "persisted": true, "visible_to_user": true, "sent_to_model": false, "derived_context": false },
                "redaction": { "state": "none" },
            })),
            "tool_result" => Some(json!({
                "card_kind": "tool_activity",
                "label": text,
                "source": "den_runtime",
                "tool": {
                    "id": message.tool_call_id.clone(),
                    "name": message.tool_name.clone(),
                    "status": message.status.clone(),
                    "result": message.raw_output.clone(),
                },
                "delivery": { "persisted": true, "visible_to_user": true, "sent_to_model": false, "derived_context": false },
                "redaction": { "state": "none" },
            })),
            _ => None,
        };
        coalesced_desc.push((
            message.sequence_no,
            ChatHistoryMessage {
                kind: message.kind,
                role: client_chat_history_role(&storage_role),
                text,
                tool_call_id: message.tool_call_id,
                tool_name: message.tool_name,
                status: message.status,
                created_at: Some(message.created_at.to_string()),
                event,
            },
        ));
    }

    let has_more = coalesced_desc.len() >= page_limit;
    let page = coalesced_desc
        .into_iter()
        .take(page_limit)
        .collect::<Vec<_>>();
    let next_before = page.last().map(|(sequence_no, _)| sequence_no.to_string());
    let messages = page
        .into_iter()
        .rev()
        .map(|(_, message)| message)
        .collect::<Vec<_>>();
    (messages, has_more, next_before)
}

fn chat_history_work_activity(
    entry: den_runtime::work_activity::WorkActivityEntry,
) -> Option<ChatHistoryMessage> {
    use den_runtime::work_activity::WorkActivityKind;

    let created_at = Some(entry.created_at.to_string());
    let text = if entry.truncated {
        format!("{} [truncated]", entry.text)
    } else {
        entry.text
    };
    let message = match entry.kind {
        WorkActivityKind::AssistantMessage => ChatHistoryMessage {
            kind: chat_history_message_kind(),
            role: "ai".to_string(),
            text,
            tool_call_id: None,
            tool_name: None,
            status: None,
            created_at,
            event: None,
        },
        WorkActivityKind::ReasoningSummary => ChatHistoryMessage {
            kind: "reasoning_delta".to_string(),
            role: "ai".to_string(),
            text,
            tool_call_id: None,
            tool_name: None,
            status: None,
            created_at,
            event: None,
        },
        WorkActivityKind::ToolCall | WorkActivityKind::ToolResult => ChatHistoryMessage {
            kind: if entry.kind == WorkActivityKind::ToolCall {
                "tool_call"
            } else {
                "tool_result"
            }
            .to_string(),
            role: "system".to_string(),
            text,
            tool_call_id: entry.tool_call_id,
            tool_name: entry.tool_name,
            status: Some(
                if entry.kind == WorkActivityKind::ToolCall {
                    "requested"
                } else {
                    "completed"
                }
                .to_string(),
            ),
            created_at,
            event: None,
        },
        WorkActivityKind::Approval | WorkActivityKind::Lifecycle => ChatHistoryMessage {
            kind: chat_history_message_kind(),
            role: "system".to_string(),
            text,
            tool_call_id: None,
            tool_name: None,
            status: None,
            created_at,
            event: None,
        },
    };
    Some(message)
}

#[derive(Debug, Deserialize)]
pub struct ChatSendRequest {
    pub bear_id: Uuid,
    pub message: String,
    /// Reserved for runtime conversation / OTID pass-through (optional).
    #[serde(default)]
    pub conversation_id: Option<String>,
}

fn chat_send_api_status_message(err: &CustomError) -> (StatusCode, String) {
    match err {
        CustomError::Anyhow(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        CustomError::System(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::Database(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::DatabaseUnavailable(s) => (StatusCode::SERVICE_UNAVAILABLE, s.clone()),
        CustomError::Session(s) => (StatusCode::INTERNAL_SERVER_ERROR, s.clone()),
        CustomError::Authentication(s) => (StatusCode::UNAUTHORIZED, s.clone()),
        CustomError::Authorization(s) => (StatusCode::FORBIDDEN, s.clone()),
        CustomError::Render(s) => (StatusCode::INTERNAL_SERVER_ERROR, s.clone()),
        CustomError::Parsing(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
        CustomError::Email(s) => (StatusCode::FAILED_DEPENDENCY, s.clone()),
        CustomError::NotFound(s) => (StatusCode::NOT_FOUND, s.clone()),
        CustomError::ValidationError(s) => (StatusCode::BAD_REQUEST, s.clone()),
    }
}

fn chat_send_error_response(err: CustomError, request_id: Uuid) -> Response {
    tracing::error!(%request_id, error = %err, "chat_send rejected");
    let (status, message) = chat_send_api_status_message(&err);
    let body = serde_json::json!({
        "error": message,
        "request_id": request_id,
    });
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    match Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body.to_string()))
    {
        Ok(r) => r,
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("response build: {e}"),
        )
            .into_response(),
    }
}

async fn web_chat_workboard_prompt_context(
    _pool: &sqlx::PgPool,
    _bear_id: Uuid,
    _user_id: i32,
    _conversation_id: &str,
    _session_id: &str,
) -> Result<String, CustomError> {
    // ponytail: local task-list lookup was removed with the Docket crate split; skip
    // prompt workboard context rather than breaking chat/deploy. Upgrade path: render
    // from Docket checkout/list APIs once conversation-scoped task lists are backed
    // by the relational Docket model.
    Ok(String::new())
}

async fn chat_model_response_for(
    state: &AppState,
    user_id: i32,
    bear_id: Uuid,
    conversation_id: Option<&str>,
) -> Result<ChatModelResponse, CustomError> {
    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }
    let bear = bears_db::get_bear(state.sqlx_pool(), bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let conv_id = normalize_client_conversation_id(conversation_id)?;
    let (configured, model_options, fetch_error) = model_catalog_select_context(state).await;
    if !configured || model_options.is_empty() {
        return Err(CustomError::System(fetch_error.unwrap_or_else(|| {
            "No Den model selection options are configured.".to_string()
        })));
    }

    let base_model = bears_db::resolve_model_for_profile(
        state.sqlx_pool(),
        &bear,
        BearProfile::Chat,
        state.config.default_llm_model.as_str(),
    )
    .await?;

    if conv_id.starts_with("new-") {
        return Ok(ChatModelResponse {
            selection_mode: "auto".to_string(),
            requested_model: None,
            selected_model: None,
            effective_model: base_model,
            source: "stance_or_bear_default".to_string(),
            model_options,
        });
    }

    let conversation = conversation_persistence::ensure_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        Some(user_id),
        &conv_id,
        None,
        None,
    )
    .await?;
    let state_row =
        conversation_persistence::get_conversation_model_state(state.sqlx_pool(), conversation.id)
            .await?;
    let effective = conversation_persistence::resolve_conversation_selected_model(
        state.sqlx_pool(),
        conversation.id,
    )
    .await?
    .unwrap_or(base_model);
    Ok(ChatModelResponse {
        selection_mode: state_row
            .as_ref()
            .map(|row| row.selection_mode.clone())
            .unwrap_or_else(|| "auto".to_string()),
        requested_model: state_row
            .as_ref()
            .and_then(|row| row.requested_model.clone()),
        selected_model: state_row
            .as_ref()
            .and_then(|row| row.selected_model.clone()),
        effective_model: effective,
        source: if state_row.as_ref().map(|row| row.selection_mode.as_str()) == Some("explicit") {
            "conversation_explicit".to_string()
        } else {
            "stance_or_bear_default".to_string()
        },
        model_options,
    })
}

async fn chat_model_get(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(q): Query<ChatModelQuery>,
) -> Result<Json<ChatModelResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    Ok(Json(
        chat_model_response_for(&state, user_id, q.bear_id, q.conversation_id.as_deref()).await?,
    ))
}

async fn chat_model_patch(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(body): Json<ChatModelPatchBody>,
) -> Result<Json<ChatModelResponse>, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let allowed = bears_db::user_may_use_bear(state.sqlx_pool(), user_id, body.bear_id).await?;
    if !allowed {
        return Err(CustomError::Authorization(
            "you do not have access to this bear".to_string(),
        ));
    }
    let bear = bears_db::get_bear(state.sqlx_pool(), body.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let conv_id = normalize_client_conversation_id(body.conversation_id.as_deref())?;
    if conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "choose a model after the conversation is created".to_string(),
        ));
    }
    let (configured, model_options, fetch_error) = model_catalog_select_context(&state).await;
    if !configured || model_options.is_empty() {
        return Err(CustomError::System(fetch_error.unwrap_or_else(|| {
            "No Den model selection options are configured.".to_string()
        })));
    }
    let mode = body.selection_mode.as_deref().unwrap_or("auto").trim();
    let conversation = conversation_persistence::ensure_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        Some(user_id),
        &conv_id,
        None,
        None,
    )
    .await?;
    den_service::model_selection::apply_conversation_model_selection(
        state.sqlx_pool(),
        conversation.id,
        mode,
        body.model.as_deref(),
        "human_selected",
        "inherit_stance_or_bear_default",
    )
    .await?;
    Ok(Json(
        chat_model_response_for(&state, user_id, body.bear_id, Some(&conv_id)).await?,
    ))
}

async fn chat_send(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(body): Json<ChatSendRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    let result = async { chat_send_inner(state, auth_session, body, request_id).await }
        .instrument(tracing::info_span!("chat_send", request_id = %request_id))
        .await;
    match result {
        Ok(r) => r.into_response(),
        Err(e) => chat_send_error_response(e, request_id),
    }
}

fn parse_set_conversation_title_request(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in [
        "set conversation title to ",
        "rename conversation to ",
        "rename this conversation to ",
        "set this conversation title to ",
    ] {
        if lower.starts_with(prefix) {
            return Some(
                trimmed[prefix.len()..]
                    .trim()
                    .trim_matches(['\"', '\''])
                    .to_string(),
            )
            .filter(|title| !title.is_empty());
        }
    }
    None
}

struct ConversationTitleRequest<'a> {
    bear: &'a den_service::bears::Bear,
    conv_id: &'a str,
    message: &'a str,
    request_id: Uuid,
}

async fn maybe_handle_direct_set_conversation_title(
    state: &AppState,
    request: ConversationTitleRequest<'_>,
) -> Result<Option<Response>, CustomError> {
    let ConversationTitleRequest {
        bear,
        conv_id,
        message,
        request_id,
    } = request;
    let Some(title) = parse_set_conversation_title_request(message) else {
        return Ok(None);
    };
    let title = title.chars().take(120).collect::<String>();
    let _ = conversation_persistence::set_conversation_title_and_sync_client_sessions(
        state.sqlx_pool(),
        bear.id,
        conv_id,
        &title,
    )
    .await?;
    let text = "Conversation title updated.";
    let body = deep_chat_sse_body_for_assistant_text(text);
    Ok(Some(chat_sse_body_response(Body::from(body), request_id)?))
}

fn chat_sse_body_response(body: Body, request_id: Uuid) -> Result<Response, CustomError> {
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .map_err(|_| CustomError::System("invalid request id for response header".to_string()))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(body)
        .map_err(|err| CustomError::System(format!("response build: {err}")))
}

fn direct_chat_sse_response(text: &str, request_id: Uuid) -> Result<Response, CustomError> {
    let body = deep_chat_sse_body_for_assistant_text(text);
    chat_sse_body_response(Body::from(body), request_id)
}

fn chat_turn_is_capabilities_meta_query(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    const PHRASES: &[&str] = &[
        "list capabilities",
        "list your capabilities",
        "list tools",
        "list your tools",
        "what tools",
        "what capabilities",
        "which tools",
        "which capabilities",
    ];
    PHRASES.iter().any(|phrase| lower.contains(phrase))
}

async fn maybe_handle_direct_capabilities_list(
    pool: &sqlx::PgPool,
    canonical_conversation_id: Uuid,
    message: &str,
    request_id: Uuid,
) -> Result<Option<Response>, CustomError> {
    if !chat_turn_is_capabilities_meta_query(message.trim()) {
        return Ok(None);
    }
    let text = den_core::tools::descriptor::render_profile_tool_surface_blurb(BearProfile::Chat);
    conversation_persistence::append_message(
        pool,
        canonical_conversation_id,
        &den_service::conversation::message_types::ConversationMessageWrite::assistant_turn(
            text.clone(),
            serde_json::json!({
                "type": "assistant_output",
                "text": text,
                "request_id": request_id.to_string(),
                "source": "direct_capabilities_list",
            }),
        ),
    )
    .await?;
    tracing::info!(
        %request_id,
        conversation_id = %canonical_conversation_id,
        "web chat capabilities list answered without LLM round-trip"
    );
    Ok(Some(direct_chat_sse_response(&text, request_id)?))
}

async fn resolve_chat_profile_binding_id(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    native_runtime: bool,
) -> Result<String, CustomError> {
    bears_db::profile_binding_id(pool, bear_id, BearProfile::Chat)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::System(if native_runtime {
                "This bear has no chat profile runtime binding. Ask an operator to provision missing profiles in Admin → Bears.".to_string()
            } else {
                "This bear is not provisioned yet (missing chat profile runtime)."
                    .to_string()
            })
        })
}

fn chat_sse_response(
    stream: BearChannelSseProxyStream,
    request_id: Uuid,
) -> Result<Response, CustomError> {
    chat_sse_body_response(Body::from_stream(stream), request_id)
}

async fn chat_send_native_inner(
    state: AppState,
    body: ChatSendRequest,
    request_id: Uuid,
    user_id: i32,
    username: &str,
    bear: den_service::bears::Bear,
    chat_binding_id: &str,
    conv_id: String,
) -> Result<Response, CustomError> {
    if state.config.llm_api_url.trim().is_empty() {
        return Err(CustomError::System(
            "Chat is unavailable: LLM_API_URL is not set (required when AGENT_RUNTIME=native)."
                .to_string(),
        ));
    }

    let membership_role =
        bears_db::membership_role_for_user(state.sqlx_pool(), user_id, body.bear_id)
            .await?
            .flatten();
    let session_id = format!("den-web:{}:{}", body.bear_id, conv_id);
    if let Some(response) = maybe_handle_direct_set_conversation_title(
        &state,
        ConversationTitleRequest {
            bear: &bear,
            conv_id: &conv_id,
            message: body.message.trim(),
            request_id,
        },
    )
    .await?
    {
        return Ok(response);
    }

    let workboard_context = web_chat_workboard_prompt_context(
        state.sqlx_pool(),
        bear.id,
        user_id,
        &conv_id,
        &session_id,
    )
    .await?;
    let upstream_message = format!("{}{}", body.message.trim(), workboard_context);

    let canonical_conversation = conversation_persistence::ensure_conversation_for_external_id(
        state.sqlx_pool(),
        bear.id,
        Some(user_id),
        &conv_id,
        None,
        None,
    )
    .await?;
    conversation_persistence::append_message(
        state.sqlx_pool(),
        canonical_conversation.id,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            body.message.trim(),
            serde_json::json!({
                "type": "user_input",
                "text": body.message.trim(),
                "request_id": request_id.to_string(),
            }),
            Some(format!("web-chat-user-input:{request_id}")),
        ),
    )
    .await?;

    if let Some(response) = maybe_handle_direct_capabilities_list(
        state.sqlx_pool(),
        canonical_conversation.id,
        body.message.trim(),
        request_id,
    )
    .await?
    {
        return Ok(response);
    }

    crate::observability::metrics::chat_send_runtime_native();

    let runtime_stream = state
        .web_chat_runtime
        .stream_chat(
            &state,
            WebChatRuntimeRequest {
                bear_id: bear.id,
                bear_slug: bear.slug.clone(),
                chat_binding_id: chat_binding_id.to_string(),
                user_id,
                username: Some(username.to_string()),
                membership_role: membership_role.clone(),
                conversation_id: conv_id.clone(),
                session_id: session_id.clone(),
                prompt: upstream_message,
                request_id,
            },
        )
        .await?;

    crate::observability::metrics::chat_send_started();

    let upstream = NativeWebChatUpstreamStream::new(runtime_stream, request_id);
    let stream = BearChannelSseProxyStream::new(
        upstream,
        request_id,
        user_id,
        body.bear_id,
        conv_id,
        state.sqlx_pool().clone(),
    );
    chat_sse_response(stream, request_id)
}

async fn chat_send_inner(
    state: AppState,
    auth_session: AuthSession,
    body: ChatSendRequest,
    request_id: Uuid,
) -> Result<Response, CustomError> {
    let session_user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = session_user.id;
    let username = session_user.username.clone();

    if body.message.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "message must not be empty".to_string(),
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

    let chat_binding_id = resolve_chat_profile_binding_id(state.sqlx_pool(), bear.id, true).await?;
    let conv_id = normalize_client_conversation_id(body.conversation_id.as_deref())?;

    chat_send_native_inner(
        state,
        body,
        request_id,
        user_id,
        username.as_str(),
        bear,
        &chat_binding_id,
        conv_id,
    )
    .await
}

#[cfg(test)]
mod browser_client_session_tests {
    use super::browser_client_session_id;
    use uuid::Uuid;

    #[test]
    fn browser_session_id_is_scoped_to_user_bear_and_conversation() {
        let bear_id = Uuid::from_u128(7);
        assert_eq!(
            browser_client_session_id(42, bear_id, "conv-chat_1"),
            "den-web:42:00000000-0000-0000-0000-000000000007:conv-chat_1"
        );
    }
}

#[cfg(test)]
mod chat_history_map_tests {
    use super::*;
    use den_service::conversation::persistence::PersistedConversationMessage;

    fn persisted_row(
        sequence_no: i64,
        role: &str,
        message_type: &str,
        text: &str,
    ) -> PersistedConversationMessage {
        PersistedConversationMessage {
            sequence_no,
            message_type: message_type.to_string(),
            role: Some(role.to_string()),
            visibility: "default".to_string(),
            content_text: text.to_string(),
            content_json: serde_json::Value::Null,
            provider_message_id: None,
            created_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn work_activity_maps_to_typed_deep_chat_history() {
        use den_runtime::work_activity::{WorkActivityEntry, WorkActivityKind};

        let message = chat_history_work_activity(WorkActivityEntry {
            id: Uuid::from_u128(7),
            first_sequence: 1,
            last_sequence: 1,
            kind: WorkActivityKind::ReasoningSummary,
            text: "Checking the implementation.".to_string(),
            tool_call_id: None,
            tool_name: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            truncated: false,
        })
        .expect("mapped activity");

        assert_eq!(message.kind, "reasoning_delta");
        assert_eq!(message.role, "ai");
        assert_eq!(message.text, "Checking the implementation.");
        assert_eq!(
            message.created_at.as_deref(),
            Some("1970-01-01 0:00:00.0 +00:00:00")
        );
    }

    #[test]
    fn map_persisted_page_strips_trailing_ephemeral_status_suffix() {
        let rows = vec![
            persisted_row(2, "assistant", "assistant", "HelloThinking…"),
            persisted_row(1, "user", "user", "Hello"),
        ];
        let (msgs, has_more, _) = map_persisted_history_page(&rows, 10);
        assert!(!has_more);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].text, "Hello");
    }

    #[test]
    fn map_persisted_page_omits_ephemeral_only_assistant_rows() {
        let rows = vec![
            persisted_row(2, "assistant", "assistant", "Thinking…"),
            persisted_row(1, "user", "user", "Hello"),
        ];
        let (msgs, _, _) = map_persisted_history_page(&rows, 10);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn map_persisted_page_emits_ai_role_for_assistant_rows() {
        // `list_messages_page` returns rows newest-first (sequence DESC).
        let rows = vec![
            persisted_row(2, "assistant", "assistant", "Hi there"),
            persisted_row(1, "user", "user", "Hello"),
        ];
        let (msgs, has_more, _) = map_persisted_history_page(&rows, 10);
        assert!(!has_more);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "ai");
        assert_eq!(msgs[1].text, "Hi there");
    }
}

#[cfg(test)]
mod conversation_id_tests {
    use super::normalize_client_conversation_id;

    #[test]
    fn normalizes_default_aliases() {
        assert_eq!(normalize_client_conversation_id(None).unwrap(), "default");
        assert_eq!(
            normalize_client_conversation_id(Some("")).unwrap(),
            "default"
        );
        assert_eq!(
            normalize_client_conversation_id(Some("default")).unwrap(),
            "default"
        );
    }

    #[test]
    fn accepts_conv_prefix_ids() {
        assert_eq!(
            normalize_client_conversation_id(Some("conv-abc12345")).unwrap(),
            "conv-abc12345"
        );
    }

    #[test]
    fn accepts_headless_den_conv_prefix_ids() {
        assert_eq!(
            normalize_client_conversation_id(Some("den-conv-5c60dc2ee7934b20821ea51b04397ccf"))
                .unwrap(),
            "den-conv-5c60dc2ee7934b20821ea51b04397ccf"
        );
    }

    #[test]
    fn accepts_pending_new_prefix_ids() {
        assert_eq!(
            normalize_client_conversation_id(Some("new-abc12345")).unwrap(),
            "new-abc12345"
        );
    }

    #[test]
    fn rejects_garbage_ids() {
        assert!(normalize_client_conversation_id(Some("../../../etc/passwd")).is_err());
        assert!(normalize_client_conversation_id(Some("conv-x")).is_err());
    }
}
