use std::{collections::HashSet, fmt};

use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::time::OffsetDateTime;
use uuid::Uuid;

use bearwire_protocol::{
    methods::{ConversationDiagnosticsRequest, ConversationHistoryRequest},
    surface::{SurfaceHistoryEvent, SurfaceResourceRef},
};
use den_core::ids::{BearId, UserId};
use den_docket::{DocketService, PgDocketService};
use den_http::errors::CustomError;
use den_runtime::{
    bearwire_events,
    work_activity::{project_work_activity, WorkActivityEntry, WorkActivityKind},
};
use den_service::{
    artifacts::{self, ArtifactAccessContext, DocketArtifactTargetKind},
    client_sessions,
    conversation::{persistence, viewer::ConversationViewer},
    DenState,
};

use crate::auth::authenticated_bear;
use crate::methods::parse_params;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DocketSurfaceEventId(Uuid);

impl fmt::Display for DocketSurfaceEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "docket:{}", self.0)
    }
}

impl DocketSurfaceEventId {
    fn new(id: Uuid) -> Self {
        Self(id)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocketTaskDefinition {
    title: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct DocketDiagnosticEventRow {
    id: Uuid,
    created_at: OffsetDateTime,
    event_type: String,
    payload: Value,
    task_id: Option<Uuid>,
}

async fn list_docket_diagnostic_events(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    user_id: i32,
    conversation_id: &str,
    limit: i64,
) -> Result<Vec<DocketDiagnosticEventRow>, den_core::DenError> {
    sqlx::query_as::<_, DocketDiagnosticEventRow>(
        r"
        WITH execution_jobs AS (
            SELECT id AS job_id
            FROM bear_jobs
            WHERE bear_id = $1 AND source_conversation_id = $2
              AND created_by_user_id = $3
        ), docket_events AS (
            SELECT events.id, events.created_at, events.event_type, events.payload,
                   events.task_id
            FROM bear_task_events events
            JOIN bear_tasks tasks ON tasks.id = events.task_id
            WHERE tasks.job_id IN (SELECT job_id FROM execution_jobs)
              AND events.event_type IN ('created', 'updated')
              AND events.payload ? 'definition'
        )
        SELECT id, created_at, event_type, payload, task_id
        FROM docket_events
        ORDER BY created_at ASC
        LIMIT $4
        ",
    )
    .bind(bear_id)
    .bind(conversation_id)
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|err| den_core::DenError::Database(format!("list docket diagnostic events: {err}")))
}

fn docket_artifact_resources(artifact_refs: &[String]) -> Vec<SurfaceResourceRef> {
    let mut seen = HashSet::new();
    artifact_refs
        .iter()
        .filter(|artifact_ref| is_canonical_artifact_ref(artifact_ref))
        .filter(|artifact_ref| seen.insert(artifact_ref.as_str()))
        .map(|artifact_ref| SurfaceResourceRef {
            label: Some("Artifact".to_string()),
            uri: None,
            name: Some(artifact_ref.clone()),
            mime_type: None,
        })
        .collect()
}

fn conversation_artifact_surface_event(
    conversation_id: &str,
    artifact_refs: &[String],
) -> Option<SurfaceHistoryEvent> {
    (!artifact_refs.is_empty()).then(|| SurfaceHistoryEvent::Message {
        id: Some(format!("conversation-artifacts:{conversation_id}")),
        role: "system".to_string(),
        text: "Conversation artifacts".to_string(),
        resources: docket_artifact_resources(artifact_refs),
        created_at: None,
    })
}

fn is_canonical_artifact_ref(value: &str) -> bool {
    value.len() == "artifact_".len() + 32
        && value.starts_with("artifact_")
        && value["artifact_".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn docket_diagnostic_surface_event(
    row: DocketDiagnosticEventRow,
    artifact_refs: &[String],
) -> Option<Value> {
    match row.event_type.as_str() {
        "created" | "updated" => {
            let definition = serde_json::from_value::<DocketTaskDefinition>(
                row.payload.get("definition")?.clone(),
            )
            .ok()?;
            let title = definition.title.as_deref().unwrap_or("untitled task");
            Some(json!(SurfaceHistoryEvent::Message {
                id: Some(DocketSurfaceEventId::new(row.id).to_string()),
                role: "system".to_string(),
                text: format!(
                    "Docket task {}: {} ({})",
                    row.event_type,
                    title,
                    row.task_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "unknown task".to_string()),
                ),
                resources: docket_artifact_resources(artifact_refs),
                created_at: Some(row.created_at.to_string()),
            }))
        }
        _ => None,
    }
}

fn orientation_diagnostic_text(data: &Value) -> String {
    let kind = data
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let orientation = data.get("orientation").unwrap_or(&Value::Null);
    let job_id = orientation
        .pointer("/job/job_id")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let task_ref = orientation
        .pointer("/task/task_ref")
        .or_else(|| orientation.pointer("/job/active_task_ref"));
    let task = task_ref
        .and_then(|task| {
            task.get("task_id")
                .or_else(|| task.get("item_id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("none");
    format!(
        "Runtime orientation: kind={} job={} task={}",
        kind, job_id, task
    )
}

fn omit_from_review_projection(event_type: &str) -> bool {
    matches!(
        event_type,
        "run.progress" | "message.delta" | "message.reasoning.delta"
    )
}

fn trimmed_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn surface_resource_refs_from_content_json(content_json: &Value) -> Vec<SurfaceResourceRef> {
    let host_context = content_json
        .get("host_context")
        .or_else(|| content_json.pointer("/prompt_context/host_context"));
    let Some(resources) = host_context
        .and_then(|context| context.get("resources"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    resources
        .iter()
        .filter_map(|resource| {
            let label = trimmed_string(resource, &["label"])
                .or_else(|| trimmed_string(resource, &["name", "title"]));
            let name = trimmed_string(resource, &["name", "title"]);
            let mime_type = trimmed_string(
                resource,
                &["mime_type", "mimeType", "media_type", "mediaType"],
            );
            let artifact_ref = trimmed_string(resource, &["artifact_ref"])
                .filter(|artifact_ref| is_canonical_artifact_ref(artifact_ref));
            let uri = artifact_ref.map(|artifact_ref| format!("artifact:{artifact_ref}"));
            if label.is_none() && uri.is_none() && name.is_none() {
                return None;
            }
            Some(SurfaceResourceRef {
                label,
                uri,
                name,
                mime_type,
            })
        })
        .collect()
}

fn surface_text_with_resource_refs(text: String, resources: &[SurfaceResourceRef]) -> String {
    if resources.is_empty() {
        return text;
    }

    let mut rendered = text;
    rendered.push_str("\n\nReferenced resources:");
    for resource in resources {
        let label = resource
            .label
            .as_deref()
            .or(resource.name.as_deref())
            .or(resource.uri.as_deref())
            .unwrap_or("unnamed resource");
        rendered.push_str("\n- ");
        rendered.push_str(label);
        if let Some(uri) = resource.uri.as_deref() {
            if uri != label {
                rendered.push_str(" — ");
                rendered.push_str(uri);
            }
        }
        if let Some(mime_type) = resource.mime_type.as_deref() {
            rendered.push_str(" (");
            rendered.push_str(mime_type);
            rendered.push(')');
        }
    }
    rendered
}

pub(super) async fn conversation_viewer(
    state: &DenState,
    bear_id: Uuid,
    user_id: i32,
) -> Result<ConversationViewer, CustomError> {
    ConversationViewer::resolve(&state.sqlx_pool, BearId::new(bear_id), UserId::new(user_id))
        .await?
        .ok_or_else(conversation_not_found)
}

fn conversation_not_found() -> CustomError {
    CustomError::NotFound("conversation not found".to_string())
}

pub(super) async fn require_conversation_access(
    viewer: &ConversationViewer,
    pool: &sqlx::PgPool,
    conversation_id: Uuid,
) -> Result<(), CustomError> {
    if !viewer.may_access_id(pool, conversation_id).await? {
        return Err(conversation_not_found());
    }
    Ok(())
}

/// Check the canonical record before creating anything. The ensure call must not
/// update a competing owner's row if another request claims the external ID first.
pub(super) async fn authorize_existing_conversation(
    viewer: &ConversationViewer,
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    external_id: &str,
) -> Result<Option<persistence::ConversationRecord>, CustomError> {
    let conversation =
        persistence::get_conversation_for_external_id(pool, bear_id, external_id).await?;
    if let Some(conversation) = &conversation {
        require_conversation_access(viewer, pool, conversation.id).await?;
    }
    Ok(conversation)
}

pub(super) async fn authorize_or_create_conversation(
    viewer: &ConversationViewer,
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    user_id: i32,
    external_id: &str,
) -> Result<persistence::ConversationRecord, CustomError> {
    if let Some(conversation) =
        authorize_existing_conversation(viewer, pool, bear_id, external_id).await?
    {
        return Ok(conversation);
    }
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        bear_id,
        Some(user_id),
        external_id,
        None,
        None,
    )
    .await?;
    require_conversation_access(viewer, pool, conversation.id).await?;
    Ok(conversation)
}

pub(crate) async fn conversation_diagnostics_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: ConversationDiagnosticsRequest = parse_params(params)?;
    let limit = request.limit.clamp(1, 100);
    let viewer = conversation_viewer(state, bear.id, user_id).await?;
    let Some(conversation) = persistence::get_conversation_for_external_id(
        &state.sqlx_pool,
        bear.id,
        &request.conversation_id,
    )
    .await?
    else {
        return Ok(json!({
            "kind": "conversation_diagnostics",
            "conversation_id": request.conversation_id,
            "missing": true,
            "records": [],
        }));
    };

    require_conversation_access(&viewer, &state.sqlx_pool, conversation.id).await?;

    let Some(session) = client_sessions::find_latest_for_bear_conversation(
        &state.sqlx_pool,
        bear.id,
        &request.conversation_id,
    )
    .await?
    else {
        return Ok(json!({
            "kind": "conversation_diagnostics",
            "conversation_id": request.conversation_id,
            "missing": false,
            "records": [],
        }));
    };
    let run_id = request.run_id.as_deref();
    let run = match run_id {
        Some(run_id) => den_runtime::turn_runs::get_run(&state.sqlx_pool, run_id).await?,
        None => {
            den_runtime::turn_runs::active_run_for_session(
                &state.sqlx_pool,
                &session.client_session_id,
            )
            .await?
        }
    };
    let Some(run) = run.filter(|run| {
        run.bear_id == bear.id
            && run.user_id == user_id
            && run.session_id == session.client_session_id
    }) else {
        return Ok(json!({
            "kind": "conversation_diagnostics",
            "conversation_id": request.conversation_id,
            "missing": false,
            "records": [],
        }));
    };

    let message_id = request
        .message_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| CustomError::ValidationError("message_id must be a UUID".to_string()))?;
    let task_id = request
        .task_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| CustomError::ValidationError("task_id must be a UUID".to_string()))?;
    let records =
        den_runtime::agent_loop::list_loop_control_decisions_for_run(&state.sqlx_pool, &run.run_id)
            .await?
            .into_iter()
            .filter(|record| {
                message_id.is_none_or(|id| record.conversation_message_id == Some(id))
                    && task_id.is_none_or(|id| record.related_docket_task_id == Some(id))
            })
            .take(limit as usize)
            .collect::<Vec<_>>();

    let checkpoints = if request.include_checkpoints {
        den_runtime::agent_loop::list_checkpoints_for_run(&state.sqlx_pool, &run.run_id)
            .await?
            .into_iter()
            .take(limit as usize)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    Ok(json!({
        "kind": "conversation_diagnostics",
        "conversation_id": conversation.external_conversation_id,
        "run_id": run.run_id,
        "records": records,
        "checkpoints": checkpoints,
    }))
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HumanHistoryKind {
    Conversation,
    Surface,
}

impl HumanHistoryKind {
    fn response_kind(self) -> &'static str {
        match self {
            Self::Conversation => "conversation_history",
            Self::Surface => "conversation_surface_history",
        }
    }

    fn records_key(self) -> &'static str {
        match self {
            Self::Conversation => "messages",
            Self::Surface => "surface_events",
        }
    }
}

pub(crate) async fn conversation_history_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    conversation_history_like_result(state, headers, params, HumanHistoryKind::Conversation).await
}

pub(crate) async fn conversation_surface_history_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    // ponytail: surface replay currently merges structured conversation rows with a small
    // allowlist of persisted BearWire events. The ceiling is pagination/order across independent
    // sequences; replace this with a dedicated persisted surface-event stream when Phase 3 grows.
    conversation_history_like_result(state, headers, params, HumanHistoryKind::Surface).await
}

fn work_activity_surface_event(entry: WorkActivityEntry) -> Option<SurfaceHistoryEvent> {
    let id = Some(format!("bearwire:{}", entry.id));
    let created_at = Some(entry.created_at.to_string());
    let text = if entry.truncated {
        format!("{} [truncated]", entry.text)
    } else {
        entry.text
    };
    match entry.kind {
        WorkActivityKind::AssistantMessage => Some(SurfaceHistoryEvent::Message {
            id,
            role: "assistant".to_string(),
            text,
            resources: Vec::new(),
            created_at,
        }),
        WorkActivityKind::ReasoningSummary => Some(SurfaceHistoryEvent::ReasoningDelta {
            id,
            role: Some("assistant".to_string()),
            text,
            source: Some("provider_reasoning".to_string()),
            replay_policy: Some("thought".to_string()),
            created_at,
        }),
        WorkActivityKind::ToolCall => Some(SurfaceHistoryEvent::ToolCall {
            id,
            role: Some("assistant".to_string()),
            tool_call_id: entry.tool_call_id?,
            tool_name: entry.tool_name.unwrap_or_else(|| "tool".to_string()),
            status: "requested".to_string(),
            arguments: Value::Null,
            created_at,
        }),
        WorkActivityKind::ToolResult => Some(SurfaceHistoryEvent::ToolResult {
            id,
            role: Some("tool".to_string()),
            tool_call_id: entry.tool_call_id?,
            tool_name: entry.tool_name.unwrap_or_else(|| "tool".to_string()),
            status: "completed".to_string(),
            text: Some(text),
            raw_output: Value::Null,
            created_at,
        }),
        WorkActivityKind::Approval | WorkActivityKind::Lifecycle => {
            Some(SurfaceHistoryEvent::Message {
                id,
                role: "system".to_string(),
                text,
                resources: Vec::new(),
                created_at,
            })
        }
    }
}

async fn conversation_history_like_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
    kind: HumanHistoryKind,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let response_kind = kind.response_kind();
    let records_key = kind.records_key();
    let request: ConversationHistoryRequest = parse_params(params)?;
    let conversation_id = request.conversation_id;
    let before_sequence_no = request.before;
    let limit = request.limit.clamp(1, 100);
    let viewer = conversation_viewer(state, bear.id, user_id).await?;

    let Some(conversation) =
        persistence::get_conversation_for_external_id(&state.sqlx_pool, bear.id, &conversation_id)
            .await?
    else {
        let mut response = json!({
            "kind": response_kind,
            "conversation_id": conversation_id,
            "has_more": false,
            "next_before": null,
            "missing": true,
        });
        response[records_key] = json!([]);
        return Ok(response);
    };

    require_conversation_access(&viewer, &state.sqlx_pool, conversation.id).await?;

    let rows = persistence::list_projected_messages_page(
        &state.sqlx_pool,
        conversation.id,
        before_sequence_no,
        limit,
        persistence::ConversationHistoryProjection::UserHistory,
    )
    .await?;
    let has_more = rows.len() >= limit as usize;
    let next_before = rows.last().map(|row| row.sequence_no.to_string());
    let mut messages = rows
        .iter()
        .rev()
        .filter_map(|row| {
            let message = row.to_user_history_record()?;
            match message.kind.as_str() {
                "tool_call" => Some(json!(SurfaceHistoryEvent::ToolCall {
                    id: message
                        .message_id
                        .or_else(|| Some(message.sequence_no.to_string())),
                    role: Some(message.role),
                    tool_call_id: message.tool_call_id?,
                    tool_name: message.tool_name?,
                    status: message.status?,
                    arguments: message.arguments,
                    created_at: Some(message.created_at.to_string()),
                })),
                "tool_result" => {
                    let text = (!message.content.trim().is_empty()).then(|| {
                        den_runtime::agent_assist::sanitize_visible_transcript_text(
                            &message.content,
                        )
                    });
                    Some(json!(SurfaceHistoryEvent::ToolResult {
                        id: message
                            .message_id
                            .or_else(|| Some(message.sequence_no.to_string())),
                        role: Some(message.role),
                        tool_call_id: message.tool_call_id?,
                        tool_name: message.tool_name?,
                        status: message.status?,
                        text,
                        raw_output: message.raw_output,
                        created_at: Some(message.created_at.to_string()),
                    }))
                }
                _ => {
                    let mut text = den_runtime::agent_assist::sanitize_visible_transcript_text(
                        &message.content,
                    );
                    if text.trim().is_empty() {
                        return None;
                    }
                    let resources = if kind == HumanHistoryKind::Surface && message.role == "user" {
                        surface_resource_refs_from_content_json(&row.content_json)
                    } else {
                        Vec::new()
                    };
                    text = surface_text_with_resource_refs(text, &resources);
                    Some(json!(SurfaceHistoryEvent::Message {
                        id: message
                            .message_id
                            .or_else(|| Some(message.sequence_no.to_string())),
                        role: message.role,
                        text,
                        resources,
                        created_at: Some(message.created_at.to_string()),
                    }))
                }
            }
        })
        .collect::<Vec<_>>();

    if kind == HumanHistoryKind::Surface && request.include_surface_enrichment {
        require_conversation_access(&viewer, &state.sqlx_pool, conversation.id).await?;
        let session = client_sessions::find_latest_for_bear_conversation(
            &state.sqlx_pool,
            bear.id,
            &conversation_id,
        )
        .await?
        .filter(|session| session.user_id == user_id);
        let mut include_related_enrichment = true;
        let safe_session = if let Some(session) = session {
            let work_run = den_docket::work_runs::get_work_run_by_session(
                &state.sqlx_pool,
                &session.client_session_id,
            )
            .await?;
            let permitted = if let Some(work_run) = work_run.as_ref() {
                work_run.bear_id == bear.id
                    && PgDocketService::from_pool(&state.sqlx_pool)
                        .get_job(bear.id, work_run.job_id)
                        .await?
                        .is_some_and(|job| job.job.created_by_user_id == user_id)
            } else {
                true
            };
            if !permitted {
                include_related_enrichment = false;
            }
            permitted.then_some((session, work_run.is_some()))
        } else {
            None
        };
        if let Some((session, is_work_session)) = safe_session {
            messages.insert(
                0,
                json!(SurfaceHistoryEvent::SessionInfoUpdate {
                    id: Some(format!("session-info:{}", session.client_session_id)),
                    role: Some("system".to_string()),
                    session_id: Some(session.client_session_id.clone()),
                    title: session.conversation_title.clone(),
                    title_updated_at: session
                        .conversation_title_updated_at
                        .map(|value| value.to_string()),
                    current_mode: Some(session.current_mode.clone()),
                    created_at: Some(
                        session
                            .conversation_title_updated_at
                            .unwrap_or(session.updated_at)
                            .to_string(),
                    ),
                }),
            );

            let surface_event_rows = bearwire_events::list_bearwire_events_after(
                &state.sqlx_pool,
                &session.client_session_id,
                None,
                limit,
            )
            .await?;
            if is_work_session {
                messages.extend(
                    project_work_activity(surface_event_rows)
                        .into_iter()
                        .filter_map(work_activity_surface_event)
                        .map(|event| json!(event)),
                );
            } else {
                for row in surface_event_rows {
                    if omit_from_review_projection(&row.event_type) {
                        continue;
                    }
                    if row.event_type == "runtime.objective_orientation" {
                        let event_id = row
                            .event
                            .event_id
                            .unwrap_or_else(|| format!("bearwire:{}", row.id));
                        messages.push(json!(SurfaceHistoryEvent::Message {
                            id: Some(event_id),
                            role: "system".to_string(),
                            text: orientation_diagnostic_text(&row.event.data),
                            resources: Vec::<SurfaceResourceRef>::new(),
                            created_at: Some(row.created_at.to_string()),
                        }));
                        continue;
                    }

                    if row.event_type == "docket.focus" {
                        let text = docket_focus_lifecycle_text(&row.event.data);
                        let event_id = row
                            .event
                            .event_id
                            .unwrap_or_else(|| format!("bearwire:{}", row.id));
                        messages.push(json!(SurfaceHistoryEvent::Message {
                            id: Some(event_id),
                            role: "system".to_string(),
                            text,
                            resources: Vec::<SurfaceResourceRef>::new(),
                            created_at: Some(row.created_at.to_string()),
                        }));
                        continue;
                    }

                    if row.event_type == "session_info_update" {
                        let title = row
                            .event
                            .data
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|title| !title.is_empty());
                        let updated_at = row.event.data.get("updated_at").and_then(Value::as_str);
                        if title.is_none() && updated_at.is_none() {
                            continue;
                        }
                        let event_id = row
                            .event
                            .event_id
                            .unwrap_or_else(|| format!("bearwire:{}", row.id));
                        messages.push(json!(SurfaceHistoryEvent::SessionInfoUpdate {
                            id: Some(event_id),
                            role: Some("system".to_string()),
                            session_id: Some(session.client_session_id.clone()),
                            title: title.map(str::to_string),
                            title_updated_at: updated_at.map(str::to_string),
                            current_mode: None,
                            created_at: Some(row.created_at.to_string()),
                        }));
                    }
                }
            }
        }

        if include_related_enrichment {
            let conversation_artifact_refs = artifacts::list_conversation_artifact_citations(
                &state.sqlx_pool,
                bear.id,
                &conversation_id,
                ArtifactAccessContext {
                    bear_id: bear.id,
                    user_id: Some(user_id),
                    profile: den_core::BearProfile::Pair,
                },
            )
            .await?
            .into_iter()
            .map(|citation| citation.artifact_ref)
            .collect::<Vec<_>>();
            if let Some(event) =
                conversation_artifact_surface_event(&conversation_id, &conversation_artifact_refs)
            {
                messages.push(json!(event));
            }

            for row in list_docket_diagnostic_events(
                &state.sqlx_pool,
                bear.id,
                user_id,
                &conversation_id,
                limit,
            )
            .await?
            {
                let artifact_refs = if let Some(task_id) = row.task_id {
                    artifacts::list_docket_artifact_citations(
                        &state.sqlx_pool,
                        bear.id,
                        DocketArtifactTargetKind::Task,
                        task_id,
                        ArtifactAccessContext {
                            bear_id: bear.id,
                            user_id: Some(user_id),
                            profile: den_core::BearProfile::Pair,
                        },
                    )
                    .await?
                    .into_iter()
                    .map(|citation| citation.artifact_ref)
                    .collect()
                } else {
                    Vec::new()
                };
                if let Some(event) = docket_diagnostic_surface_event(row, &artifact_refs) {
                    messages.push(event);
                }
            }
        }
    }

    if kind == HumanHistoryKind::Surface {
        messages.sort_by(|left, right| {
            let left_created = left
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let right_created = right
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            left_created.cmp(right_created)
        });
    }

    if kind == HumanHistoryKind::Surface {
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        for event in &messages {
            let category = match event.get("kind").and_then(Value::as_str) {
                Some("message") => match event.get("role").and_then(Value::as_str) {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    Some("system") => "system",
                    _ => "other",
                },
                Some("tool_call") => "tool_call",
                Some("tool_result") => "tool_result",
                Some("reasoning_delta") | Some("reasoning") => "reasoning",
                Some("session_info_update") => "session_info",
                _ => "other",
            };
            *counts.entry(category).or_default() += 1;
        }
        tracing::info!(
            conversation_id,
            total = messages.len(),
            user = counts.get("user").copied().unwrap_or_default(),
            assistant = counts.get("assistant").copied().unwrap_or_default(),
            system = counts.get("system").copied().unwrap_or_default(),
            tool_call = counts.get("tool_call").copied().unwrap_or_default(),
            tool_result = counts.get("tool_result").copied().unwrap_or_default(),
            reasoning = counts.get("reasoning").copied().unwrap_or_default(),
            session_info = counts.get("session_info").copied().unwrap_or_default(),
            other = counts.get("other").copied().unwrap_or_default(),
            "projected conversation surface history"
        );
    }

    let mut response = json!({
        "kind": response_kind,
        "conversation_id": conversation_id,
        "has_more": has_more,
        "next_before": next_before,
    });
    response[records_key] = json!(messages);
    Ok(response)
}

fn docket_focus_lifecycle_text(data: &Value) -> String {
    let control = data.get("execution_control");
    let state = control
        .and_then(|control| control.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("not_established");
    let attempt = control
        .and_then(|control| control.get("attempt_id"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let task_id = control
        .and_then(|control| control.get("task_id"))
        .and_then(Value::as_str)
        .or_else(|| data.get("task_id").and_then(Value::as_str))
        .unwrap_or("unknown");
    let reason = control
        .and_then(|control| control.get("reason"))
        .and_then(Value::as_str);
    match state {
        "continuation_established" => format!(
            "Docket task-loop control established\n\n- Attempt: {attempt}\n- Task: {task_id}"
        ),
        "requested" => format!(
            "Docket focus request accepted; task-loop control is not yet established.\n\n- Attempt: {attempt}\n- Current state: requested"
        ),
        "terminal_settled" => format!(
            "Docket task turn settled; task-loop control is not active.\n\n- Attempt: {attempt}\n- Task: {task_id}"
        ),
        _ => format!(
            "Docket focus did not establish task-loop control.\n\n- Attempt: {attempt}\n- Task: {task_id}\n- Reason: {}",
            reason.unwrap_or(state),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docket_focus_text_only_claims_control_for_durable_continuation() {
        let established = json!({
            "execution_control": {
                "state": "continuation_established",
                "attempt_id": "attempt-1",
                "task_id": "task-1",
            }
        });
        let pending = json!({
            "execution_control": {
                "state": "not_established",
                "attempt_id": "attempt-2",
                "task_id": "task-2",
                "reason": "turn_ended_before_durable_continuation",
            }
        });
        assert_eq!(
            docket_focus_lifecycle_text(&established),
            "Docket task-loop control established\n\n- Attempt: attempt-1\n- Task: task-1"
        );
        assert!(docket_focus_lifecycle_text(&pending)
            .starts_with("Docket focus did not establish task-loop control."));
    }

    #[test]
    fn work_activity_surface_events_preserve_safe_activity_details() {
        let entry = |kind, text: &str| WorkActivityEntry {
            id: Uuid::from_u128(7),
            first_sequence: 1,
            last_sequence: 1,
            kind,
            text: text.to_string(),
            tool_call_id: Some("call-1".to_string()),
            tool_name: Some("fs.read".to_string()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            truncated: false,
        };

        let assistant = work_activity_surface_event(entry(
            WorkActivityKind::AssistantMessage,
            "Inspecting files.",
        ))
        .expect("assistant activity");
        let tool = work_activity_surface_event(entry(WorkActivityKind::ToolCall, "safe summary"))
            .expect("tool activity");

        assert_eq!(
            serde_json::to_value(assistant).unwrap()["text"],
            "Inspecting files."
        );
        let tool = serde_json::to_value(tool).unwrap();
        assert_eq!(tool["tool_call_id"], "call-1");
        assert_eq!(tool["tool_name"], "fs.read");
        assert_eq!(tool["arguments"], Value::Null);
    }

    #[test]
    fn docket_surface_event_id_preserves_wire_string() {
        let id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(
            DocketSurfaceEventId::new(id).to_string(),
            "docket:11111111-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn review_projection_omits_transient_stream_events() {
        assert!(omit_from_review_projection("run.progress"));
        assert!(omit_from_review_projection("message.delta"));
        assert!(omit_from_review_projection("message.reasoning.delta"));
        assert!(!omit_from_review_projection("session_info_update"));
        assert!(!omit_from_review_projection(
            "runtime.objective_orientation"
        ));
    }

    #[test]
    fn docket_artifact_refs_render_only_opaque_identifiers() {
        let row = DocketDiagnosticEventRow {
            id: Uuid::from_u128(1),
            created_at: OffsetDateTime::UNIX_EPOCH,
            event_type: "created".to_string(),
            payload: json!({"definition": {"title": "Ship it"}}),
            task_id: Some(Uuid::from_u128(3)),
        };
        let event = docket_diagnostic_surface_event(
            row,
            &[
                "artifact_0123456789abcdef0123456789abcdef".to_string(),
                "artifact_0123456789abcdef0123456789abcdef".to_string(),
                "storage_key=secret/path".to_string(),
                "artifact_0123456789abcdef0123456789abcdeF".to_string(),
            ],
        )
        .expect("surface event");
        let message = event.as_object().expect("message event");
        assert_eq!(
            message["resources"],
            json!([{
                "label": "Artifact",
                "name": "artifact_0123456789abcdef0123456789abcdef",
            }])
        );
        let text = message["text"].as_str().expect("message text");
        assert!(!text.contains("artifact_"));
        assert!(!text.contains("storage_key"));
        assert!(!text.contains("content_sha256"));
    }

    #[test]
    fn conversation_artifact_event_renders_only_canonical_refs() {
        let event = conversation_artifact_surface_event(
            "conversation-1",
            &[
                "artifact_0123456789abcdef0123456789abcdef".to_string(),
                "https://untrusted.invalid/object-key".to_string(),
            ],
        )
        .expect("nonempty inputs produce an event");

        let event = serde_json::to_value(event).unwrap();
        assert_eq!(event["text"], "Conversation artifacts");
        assert_eq!(
            event["resources"][0]["name"],
            "artifact_0123456789abcdef0123456789abcdef"
        );
        assert_eq!(event["resources"].as_array().unwrap().len(), 1);
        assert!(!event.to_string().contains("untrusted.invalid"));
    }

    #[test]
    fn host_context_resources_only_project_canonical_artifact_refs() {
        let resources = surface_resource_refs_from_content_json(&json!({
            "host_context": {
                "resources": [
                    {
                        "label": "Build report",
                        "artifact_ref": "artifact_0123456789abcdef0123456789abcdef",
                        "url": "https://untrusted.invalid/report"
                    },
                    {
                        "label": "Untrusted link",
                        "url": "https://untrusted.invalid/secret"
                    }
                ]
            }
        }));

        assert_eq!(
            resources,
            vec![
                SurfaceResourceRef {
                    label: Some("Build report".to_string()),
                    uri: Some("artifact:artifact_0123456789abcdef0123456789abcdef".to_string()),
                    name: None,
                    mime_type: None,
                },
                SurfaceResourceRef {
                    label: Some("Untrusted link".to_string()),
                    uri: None,
                    name: None,
                    mime_type: None,
                },
            ]
        );
    }
    #[test]
    fn docket_task_definition_rejects_unknown_fields() {
        assert!(serde_json::from_value::<DocketTaskDefinition>(json!({
            "title": "Ship it",
            "unexpected": true,
        }))
        .is_err());
    }
}
