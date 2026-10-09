use den_core::{
    tools::result_compaction::{truncate_str, ToolResultStatus},
    DenError,
};
use sqlx::PgPool;
use tracing::Instrument;
use uuid::Uuid;

use crate::conversation_message_types::{
    ConversationMessageRole, ConversationMessageType, ConversationMessageVisibility,
    ConversationMessageWrite,
};

use super::persistence::{append_message, ensure_conversation_for_external_id, list_messages_page};
use den_core::conversation_ids::is_acp_history_target;

pub fn canonical_persistence_enabled_for_conversation(external_conversation_id: &str) -> bool {
    is_acp_history_target(external_conversation_id)
}

#[derive(Debug, Clone)]
pub struct ConversationPersistenceContext {
    pub pool: PgPool,
    pub bear_id: Uuid,
    pub user_id: Option<i32>,
    pub external_conversation_id: String,
    pub source_session_id: Option<String>,
    pub request_id: Option<String>,
    pub persistence_scope_id: String,
    pub skip_persistence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEventDedupKey {
    pub event: String,
    pub scope_id: String,
    pub request_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub provider_message_id: Option<String>,
}

impl CanonicalEventDedupKey {
    pub fn source_event_id(&self) -> String {
        serde_json::json!({
            "event": self.event,
            "scope_id": self.scope_id,
            "request_id": self.request_id,
            "tool_call_id": self.tool_call_id,
            "provider_message_id": self.provider_message_id,
        })
        .to_string()
    }
}

#[derive(Debug, Clone)]
pub enum CanonicalConversationRecord {
    VisibleMessage {
        role: CanonicalVisibleRole,
        text: String,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    },
    StructuredEvent {
        message_type: ConversationMessageType,
        role: Option<ConversationMessageRole>,
        visibility: ConversationMessageVisibility,
        content_text: String,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct CanonicalToolResultRecord {
    pub tool_name: String,
    pub tool_call_id: String,
    pub approval_request_id: Option<String>,
    pub status: ToolResultStatus,
    pub content: Option<String>,
    pub structured_content: serde_json::Value,
    pub output_summary: String,
    pub output_preview: Option<String>,
    pub diagnostic: serde_json::Value,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CanonicalToolRequestRecord {
    pub tool_name: String,
    pub tool_call_id: String,
    pub request_id: String,
    pub approval_request_id: Option<String>,
    pub args: serde_json::Value,
    pub approval_required: bool,
    pub approval_reason: Option<String>,
    pub route: String,
}

#[derive(Debug, Clone)]
pub struct ConversationEventProvenance {
    pub source: String,
    pub scope_id: String,
}

#[derive(Debug, Clone, Copy)]
pub enum CanonicalVisibleRole {
    User,
    Assistant,
}

impl CanonicalVisibleRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

impl ConversationEventProvenance {
    pub fn client_session(scope_id: impl Into<String>) -> Self {
        Self {
            source: "client_stream".to_string(),
            scope_id: scope_id.into(),
        }
    }

    pub fn as_content_json(&self, event: &str) -> serde_json::Value {
        serde_json::json!({
            "source": self.source,
            "event": event,
            "scope_id": self.scope_id,
        })
    }
}

fn canonical_tool_result_preview(
    content: Option<&str>,
    structured_content: &serde_json::Value,
) -> Option<String> {
    let preview = content
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            structured_content
                .get("content")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| match structured_content {
            serde_json::Value::Null => None,
            value => {
                let rendered = value.to_string();
                Some(rendered).filter(|text| text != "null" && text != "{}" && text != "[]")
            }
        });
    preview.map(|preview| truncate_str(&preview, 512).0)
}

fn canonical_tool_result_summary(tool_name: &str, status: &str, preview: Option<&str>) -> String {
    match preview.filter(|value| !value.is_empty()) {
        Some(preview) => format!("Used {tool_name} ({status}): {preview}"),
        None => format!("Used {tool_name} ({status})"),
    }
}

impl CanonicalToolResultRecord {
    pub fn new(
        tool_name: Option<String>,
        tool_call_id: impl Into<String>,
        approval_request_id: Option<String>,
        status: ToolResultStatus,
        content: Option<String>,
        structured_content: serde_json::Value,
        diagnostic: serde_json::Value,
        request_id: Option<String>,
    ) -> Self {
        let tool_name = tool_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "tool".to_string());
        let content = content
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let output_preview = canonical_tool_result_preview(content.as_deref(), &structured_content);
        let output_summary =
            canonical_tool_result_summary(&tool_name, status.as_str(), output_preview.as_deref());
        Self {
            tool_name,
            tool_call_id: tool_call_id.into(),
            approval_request_id,
            status,
            content,
            structured_content,
            output_summary,
            output_preview,
            diagnostic,
            request_id,
        }
    }
}

impl CanonicalToolRequestRecord {
    pub fn new(
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        request_id: impl Into<String>,
        approval_request_id: Option<String>,
        args: serde_json::Value,
        approval_required: bool,
        approval_reason: Option<String>,
        route: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            request_id: request_id.into(),
            approval_request_id,
            args,
            approval_required,
            approval_reason,
            route: route.into(),
        }
    }
}

fn content_json_dedup_key(content_json: &serde_json::Value) -> Option<CanonicalEventDedupKey> {
    let object = content_json.as_object()?;
    let event = object.get("event")?.as_str()?.to_string();
    let scope_id = object.get("scope_id")?.as_str()?.to_string();
    let request_id = object
        .get("request_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let tool_call_id = object
        .get("tool_call_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let provider_message_id = object
        .get("provider_message_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Some(CanonicalEventDedupKey {
        event,
        scope_id,
        request_id,
        tool_call_id,
        provider_message_id,
    })
}

impl CanonicalConversationRecord {
    pub fn visible_user_message(
        text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::VisibleMessage {
            role: CanonicalVisibleRole::User,
            text: text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn visible_assistant_message(
        text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::VisibleMessage {
            role: CanonicalVisibleRole::Assistant,
            text: text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn model_visible_hidden_assistant_message(
        text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::StructuredEvent {
            message_type: ConversationMessageType::Assistant,
            role: Some(ConversationMessageRole::Assistant),
            visibility: ConversationMessageVisibility::HiddenFromUser,
            content_text: text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn tool_event(
        content_text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::StructuredEvent {
            message_type: ConversationMessageType::ToolResult,
            role: Some(ConversationMessageRole::System),
            visibility: ConversationMessageVisibility::DiagnosticOnly,
            content_text: content_text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn tool_call_event(
        content_text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::StructuredEvent {
            message_type: ConversationMessageType::ToolCall,
            role: Some(ConversationMessageRole::System),
            visibility: ConversationMessageVisibility::DiagnosticOnly,
            content_text: content_text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn workflow_event(
        content_text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::StructuredEvent {
            message_type: ConversationMessageType::WorkflowEvent,
            role: Some(ConversationMessageRole::System),
            visibility: ConversationMessageVisibility::DiagnosticOnly,
            content_text: content_text.into(),
            content_json,
            provider_message_id,
        }
    }

    pub fn assistant_output(
        text: impl Into<String>,
        provenance: &ConversationEventProvenance,
        provider_message_id: Option<String>,
        request_id: Option<String>,
    ) -> Self {
        let mut content_json = provenance.as_content_json("assistant_output");
        if let Some(request_id) = request_id {
            content_json["request_id"] = serde_json::json!(request_id);
        }
        if let Some(provider_message_id) = provider_message_id.as_ref() {
            content_json["provider_message_id"] = serde_json::json!(provider_message_id);
        }
        Self::visible_assistant_message(text, content_json, provider_message_id)
    }

    pub fn conversation_resolved(
        conversation_id: impl Into<String>,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        let conversation_id = conversation_id.into();
        Self::workflow_event(
            "Conversation resolved",
            serde_json::json!({
                "source": provenance.source,
                "event": "conversation_resolved",
                "scope_id": provenance.scope_id,
                "conversation_id": conversation_id,
            }),
            None,
        )
    }

    pub fn tool_request(
        record: CanonicalToolRequestRecord,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        Self::tool_call_event(
            format!("Tool request: {}", record.tool_name),
            serde_json::json!({
                "source": provenance.source,
                "event": "tool_request",
                "scope_id": provenance.scope_id,
                "request_id": record.request_id,
                "tool_call_id": record.tool_call_id,
                "approval_request_id": record.approval_request_id,
                "tool_name": record.tool_name,
                "args": record.args,
                "approval_required": record.approval_required,
                "approval_reason": record.approval_reason,
                "route": record.route,
            }),
            None,
        )
    }

    pub fn tool_result(
        record: CanonicalToolResultRecord,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        Self::tool_event(
            format!("Tool result: {}", record.tool_name),
            serde_json::json!({
                "source": provenance.source,
                "event": "tool_result",
                "scope_id": provenance.scope_id,
                "tool_call_id": record.tool_call_id,
                "approval_request_id": record.approval_request_id,
                "tool_name": record.tool_name,
                "status": record.status.as_str(),
                "content": record.content,
                "structured_content": record.structured_content,
                "output_summary": record.output_summary,
                "output_preview": record.output_preview,
                "diagnostic": record.diagnostic,
                "request_id": record.request_id,
            }),
            None,
        )
    }

    /// A genuine model tool exchange belongs to model replay, not diagnostic-only
    /// storage. Human history remains a separate projection.
    pub fn model_tool_request(
        record: CanonicalToolRequestRecord,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        Self::model_tool_projection(Self::tool_request(record, provenance))
    }

    pub fn model_tool_result(
        record: CanonicalToolResultRecord,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        Self::model_tool_projection(Self::tool_result(record, provenance))
    }

    fn model_tool_projection(mut record: Self) -> Self {
        if let Self::StructuredEvent { visibility, .. } = &mut record {
            *visibility = ConversationMessageVisibility::HiddenFromUser;
        }
        record
    }

    pub fn turn_outcome(
        status: &str,
        reason: &str,
        request_id: impl Into<String>,
        retryable: bool,
        scope: serde_json::Value,
        diagnostics: serde_json::Value,
        provenance: &ConversationEventProvenance,
    ) -> Self {
        Self::workflow_event(
            format!("Turn outcome: {status} / {reason}"),
            serde_json::json!({
                "source": provenance.source,
                "event": "turn_result",
                "scope_id": provenance.scope_id,
                "status": status,
                "reason": reason,
                "request_id": request_id.into(),
                "retryable": retryable,
                "scope": scope,
                "diagnostics": diagnostics,
            }),
            None,
        )
    }

    pub fn structured_event(
        message_type: ConversationMessageType,
        role: Option<ConversationMessageRole>,
        visibility: ConversationMessageVisibility,
        content_text: impl Into<String>,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        Self::normalized_structured_event(
            message_type,
            role,
            visibility,
            content_text.into(),
            content_json,
            provider_message_id,
        )
    }

    pub fn normalized_structured_event(
        message_type: ConversationMessageType,
        role: Option<ConversationMessageRole>,
        visibility: ConversationMessageVisibility,
        content_text: String,
        content_json: serde_json::Value,
        provider_message_id: Option<String>,
    ) -> Self {
        match message_type {
            ConversationMessageType::ToolResult => {
                Self::tool_event(content_text, content_json, provider_message_id)
            }
            ConversationMessageType::ToolCall => {
                Self::tool_call_event(content_text, content_json, provider_message_id)
            }
            ConversationMessageType::WorkflowEvent
                if visibility == ConversationMessageVisibility::DiagnosticOnly =>
            {
                Self::workflow_event(content_text, content_json, provider_message_id)
            }
            _ => Self::StructuredEvent {
                message_type,
                role,
                visibility,
                content_text,
                content_json,
                provider_message_id,
            },
        }
    }

    pub fn to_write(&self, source_event_id: Option<String>) -> ConversationMessageWrite {
        match self {
            Self::VisibleMessage {
                role,
                text,
                content_json,
                provider_message_id,
            } => {
                let (message_type, message_role) = match role {
                    CanonicalVisibleRole::User => {
                        (ConversationMessageType::User, ConversationMessageRole::User)
                    }
                    CanonicalVisibleRole::Assistant => (
                        ConversationMessageType::Assistant,
                        ConversationMessageRole::Assistant,
                    ),
                };
                ConversationMessageWrite {
                    message_type,
                    role: Some(message_role),
                    visibility: ConversationMessageVisibility::Default,
                    content_text: text.clone(),
                    content_json: content_json.clone(),
                    provider_message_id: provider_message_id.clone(),
                    source_event_id,
                    created_at: None,
                }
            }
            Self::StructuredEvent {
                message_type,
                role,
                visibility,
                content_text,
                content_json,
                provider_message_id,
            } => ConversationMessageWrite {
                message_type: *message_type,
                role: *role,
                visibility: *visibility,
                content_text: content_text.clone(),
                content_json: content_json.clone(),
                provider_message_id: provider_message_id.clone(),
                source_event_id,
                created_at: None,
            },
        }
    }

    fn storage_json(&self) -> serde_json::Value {
        match self {
            Self::VisibleMessage { content_json, .. } => content_json.clone(),
            Self::StructuredEvent { content_json, .. } => content_json.clone(),
        }
    }

    fn dedup_key(&self) -> Option<CanonicalEventDedupKey> {
        content_json_dedup_key(&self.storage_json())
    }
}

fn canonical_record_source_event_id(record: &CanonicalConversationRecord) -> Option<String> {
    record.dedup_key().map(|key| key.source_event_id())
}

async fn canonical_record_already_persisted(
    context: &ConversationPersistenceContext,
    conversation_id: Uuid,
    record: &CanonicalConversationRecord,
) -> Result<bool, DenError> {
    let Some(expected) = record.dedup_key() else {
        return Ok(false);
    };
    let recent = list_messages_page(&context.pool, conversation_id, None, 32).await?;
    for message in recent {
        if let Ok(content_json) = serde_json::from_str::<serde_json::Value>(&message.content_text) {
            if content_json_dedup_key(&content_json).as_ref() == Some(&expected) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub async fn persist_canonical_conversation_record_with_id(
    context: &ConversationPersistenceContext,
    record: &CanonicalConversationRecord,
) -> Result<Option<Uuid>, DenError> {
    if context.skip_persistence
        || !canonical_persistence_enabled_for_conversation(&context.external_conversation_id)
    {
        return Ok(None);
    }
    let canonical = ensure_conversation_for_external_id(
        &context.pool,
        context.bear_id,
        context.user_id,
        &context.external_conversation_id,
        context.source_session_id.as_deref(),
        None,
    )
    .await?;
    let source_event_id = canonical_record_source_event_id(record);
    if source_event_id.is_none()
        && canonical_record_already_persisted(context, canonical.id, record).await?
    {
        return Ok(None);
    }
    let write = record.to_write(source_event_id);
    let appended = append_message(&context.pool, canonical.id, &write).await?;
    Ok(Some(appended.id))
}

pub async fn persist_canonical_conversation_record(
    context: &ConversationPersistenceContext,
    record: &CanonicalConversationRecord,
) -> Result<(), DenError> {
    if context.skip_persistence {
        return Ok(());
    }
    if !canonical_persistence_enabled_for_conversation(&context.external_conversation_id) {
        return Ok(());
    }
    let canonical = ensure_conversation_for_external_id(
        &context.pool,
        context.bear_id,
        context.user_id,
        &context.external_conversation_id,
        context.source_session_id.as_deref(),
        None,
    )
    .await?;
    let source_event_id = canonical_record_source_event_id(record);
    if source_event_id.is_none()
        && canonical_record_already_persisted(context, canonical.id, record).await?
    {
        return Ok(());
    }
    let write = record.to_write(source_event_id);
    append_message(&context.pool, canonical.id, &write).await?;
    Ok(())
}

pub fn canonical_persistence_context(
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    external_conversation_id: String,
    source_session_id: Option<String>,
    request_id: Option<String>,
    persistence_scope_id: String,
    skip_persistence: bool,
) -> ConversationPersistenceContext {
    ConversationPersistenceContext {
        pool,
        bear_id,
        user_id,
        external_conversation_id,
        source_session_id,
        request_id,
        persistence_scope_id,
        skip_persistence,
    }
}

#[cfg(any(test, feature = "test-util"))]
pub fn normalize_persisted_gateway_record(
    message_type: &str,
    role: Option<&str>,
    visibility: &str,
    content_text: String,
    content_json: serde_json::Value,
    provider_message_id: Option<String>,
) -> CanonicalConversationRecord {
    match message_type {
        "message" => match role {
            Some("user") => CanonicalConversationRecord::visible_user_message(
                content_text,
                content_json,
                provider_message_id,
            ),
            _ => CanonicalConversationRecord::visible_assistant_message(
                content_text,
                content_json,
                provider_message_id,
            ),
        },
        _ => {
            let message_type = ConversationMessageType::try_from_storage(message_type)
                .unwrap_or(ConversationMessageType::WorkflowEvent);
            let visibility = ConversationMessageVisibility::try_from_storage(visibility)
                .unwrap_or(ConversationMessageVisibility::DiagnosticOnly);
            let role = role.and_then(|value| ConversationMessageRole::try_from_storage(value).ok());
            CanonicalConversationRecord::normalized_structured_event(
                message_type,
                role,
                visibility,
                content_text,
                content_json,
                provider_message_id,
            )
        }
    }
}

pub fn spawn_persist_canonical_conversation_record(
    context: ConversationPersistenceContext,
    record: CanonicalConversationRecord,
) {
    if context.skip_persistence {
        return;
    }
    let span_request_id = context.request_id.clone();
    let span_scope = context.persistence_scope_id.clone();
    tokio::spawn(
        async move {
            if let Err(err) = persist_canonical_conversation_record(&context, &record).await {
                tracing::warn!(
                    request_id = ?context.request_id,
                    persistence_scope_id = %context.persistence_scope_id,
                    error = %err,
                    "canonical conversation record persistence failed"
                );
            }
        }
        .instrument(tracing::info_span!(
            "persist_canonical_conversation_record",
            request_id = ?span_request_id,
            persistence_scope_id = %span_scope,
        )),
    );
}

pub fn spawn_persist_assistant_output(
    context: ConversationPersistenceContext,
    content_text: String,
    provenance: &ConversationEventProvenance,
    provider_message_id: Option<String>,
    request_id: Option<String>,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::assistant_output(
            content_text,
            provenance,
            provider_message_id,
            request_id,
        ),
    );
}

#[derive(Debug, Clone)]
pub struct TurnOutcomeRecord {
    pub status: String,
    pub reason: String,
    pub request_id: String,
    pub retryable: bool,
    pub scope: serde_json::Value,
    pub diagnostics: serde_json::Value,
}

pub fn spawn_persist_turn_outcome(
    context: ConversationPersistenceContext,
    outcome: &TurnOutcomeRecord,
    provenance: &ConversationEventProvenance,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::turn_outcome(
            &outcome.status,
            &outcome.reason,
            outcome.request_id.clone(),
            outcome.retryable,
            outcome.scope.clone(),
            outcome.diagnostics.clone(),
            provenance,
        ),
    );
}

pub fn spawn_persist_model_tool_result(
    context: ConversationPersistenceContext,
    record: CanonicalToolResultRecord,
    provenance: &ConversationEventProvenance,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::model_tool_result(record, provenance),
    );
}

pub fn spawn_persist_model_tool_request(
    context: ConversationPersistenceContext,
    record: CanonicalToolRequestRecord,
    provenance: &ConversationEventProvenance,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::model_tool_request(record, provenance),
    );
}

pub fn spawn_persist_tool_result(
    context: ConversationPersistenceContext,
    record: CanonicalToolResultRecord,
    provenance: &ConversationEventProvenance,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::tool_result(record, provenance),
    );
}

pub fn spawn_persist_tool_request(
    context: ConversationPersistenceContext,
    record: CanonicalToolRequestRecord,
    provenance: &ConversationEventProvenance,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::tool_request(record, provenance),
    );
}

pub fn spawn_persist_workflow_event(
    context: ConversationPersistenceContext,
    content_text: String,
    content_json: serde_json::Value,
    provider_message_id: Option<String>,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::workflow_event(
            content_text,
            content_json,
            provider_message_id,
        ),
    );
}

pub fn spawn_persist_operational_outcome_message(
    context: ConversationPersistenceContext,
    content_text: String,
    content_json: serde_json::Value,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::model_visible_hidden_assistant_message(
            content_text,
            content_json,
            None,
        ),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionSource {
    ClientStream,
    MemoryProposals,
    PairReflection,
    ReflectionConductor,
    DenTools,
}

impl ProjectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientStream => "client_stream",
            Self::MemoryProposals => "memory_proposals",
            Self::PairReflection => "pair_reflection",
            Self::ReflectionConductor => "reflection_conductor",
            Self::DenTools => "den_core::tools",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionEventKind {
    MemoryProposalCreated,
    MemoryProposalResolved,
    PairReflectionCompleted,
    MemoryCurateEnqueued,
    MemoryCurateStarted,
    MemoryCurateCompleted,
    MemoryCurateFailed,
    MemoryReviewRequested,
}

impl ProjectionEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryProposalCreated => "memory_proposal_created",
            Self::MemoryProposalResolved => "memory_proposal_resolved",
            Self::PairReflectionCompleted => "pair_reflection_completed",
            Self::MemoryCurateEnqueued => "memory_curate_enqueued",
            Self::MemoryCurateStarted => "memory_curate_started",
            Self::MemoryCurateCompleted => "memory_curate_completed",
            Self::MemoryCurateFailed => "memory_curate_failed",
            Self::MemoryReviewRequested => "memory_review_requested",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectionProvenance {
    pub source: ProjectionSource,
    pub scope_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryProposalCreatedPayload {
    pub proposal_id: Uuid,
    pub source_profile: String,
    pub suggested_action: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryProposalResolvedPayload {
    pub proposal_id: Uuid,
    pub source_profile: String,
    pub suggested_action: String,
    pub title: String,
    pub status: String,
    pub reviewer_profile: Option<String>,
    pub result_path: Option<String>,
    pub result_commit: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PairReflectionCompletedPayload {
    pub reflection_run_id: Uuid,
    pub client_session_id: String,
    pub trigger: String,
    pub status: String,
    pub summary_path: Option<String>,
    pub summary_commit: Option<String>,
    pub considered_message_count: i32,
    pub completed_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryCurateEnqueuedPayload {
    pub reflection_run_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub proposal_ids: Vec<Uuid>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<time::Date>,
    pub created_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryCurateStartedPayload {
    pub reflection_run_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub proposal_ids: Vec<Uuid>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<time::Date>,
    pub started_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryCurateCompletedPayload {
    pub reflection_run_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub proposal_ids: Vec<Uuid>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<time::Date>,
    pub completed_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryCurateFailedPayload {
    pub reflection_run_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub proposal_ids: Vec<Uuid>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<time::Date>,
    pub error: Option<String>,
    pub completed_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryReviewRequestedPayload {
    pub proposal_id: Uuid,
    pub source_profile: String,
    pub title: String,
    pub suggested_action: String,
    pub status: String,
    pub source_paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProjectionEvent {
    MemoryProposalCreated(MemoryProposalCreatedPayload),
    MemoryProposalResolved(MemoryProposalResolvedPayload),
    PairReflectionCompleted(PairReflectionCompletedPayload),
    MemoryCurateEnqueued(MemoryCurateEnqueuedPayload),
    MemoryCurateStarted(MemoryCurateStartedPayload),
    MemoryCurateCompleted(MemoryCurateCompletedPayload),
    MemoryCurateFailed(MemoryCurateFailedPayload),
    MemoryReviewRequested(MemoryReviewRequestedPayload),
}

impl ProjectionEvent {
    pub fn kind(&self) -> ProjectionEventKind {
        match self {
            Self::MemoryProposalCreated(_) => ProjectionEventKind::MemoryProposalCreated,
            Self::MemoryProposalResolved(_) => ProjectionEventKind::MemoryProposalResolved,
            Self::PairReflectionCompleted(_) => ProjectionEventKind::PairReflectionCompleted,
            Self::MemoryCurateEnqueued(_) => ProjectionEventKind::MemoryCurateEnqueued,
            Self::MemoryCurateStarted(_) => ProjectionEventKind::MemoryCurateStarted,
            Self::MemoryCurateCompleted(_) => ProjectionEventKind::MemoryCurateCompleted,
            Self::MemoryCurateFailed(_) => ProjectionEventKind::MemoryCurateFailed,
            Self::MemoryReviewRequested(_) => ProjectionEventKind::MemoryReviewRequested,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Projection {
    pub provenance: ProjectionProvenance,
    pub event: ProjectionEvent,
    pub workflow_text: String,
    pub visible_summary: Option<String>,
}

pub fn memory_proposal_created_projection(
    provenance: ProjectionProvenance,
    proposal_id: Uuid,
    source_profile: &str,
    suggested_action: &str,
    title: &str,
    status: &str,
) -> Projection {
    Projection {
        provenance,
        event: ProjectionEvent::MemoryProposalCreated(MemoryProposalCreatedPayload {
            proposal_id,
            source_profile: source_profile.to_string(),
            suggested_action: suggested_action.to_string(),
            title: title.to_string(),
            status: status.to_string(),
        }),
        workflow_text: format!("Memory proposal created: {title}"),
        visible_summary: Some(format!(
            "Review requested for memory proposal '{}' from {}.",
            title, source_profile
        )),
    }
}

pub fn memory_proposal_resolved_projection(
    provenance: ProjectionProvenance,
    proposal_id: Uuid,
    source_profile: &str,
    suggested_action: &str,
    title: &str,
    status: &str,
    reviewer_profile: Option<String>,
    result_path: Option<String>,
    result_commit: Option<String>,
) -> Projection {
    let visible_summary = match status {
        "approved" => Some(match result_path.as_deref() {
            Some(path) => format!(
                "Memory proposal '{}' was approved and applied at {path}.",
                title
            ),
            None => format!("Memory proposal '{}' was approved.", title),
        }),
        "rejected" => Some(format!("Memory proposal '{}' was rejected.", title)),
        _ => None,
    };
    Projection {
        provenance,
        event: ProjectionEvent::MemoryProposalResolved(MemoryProposalResolvedPayload {
            proposal_id,
            source_profile: source_profile.to_string(),
            suggested_action: suggested_action.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            reviewer_profile,
            result_path,
            result_commit,
        }),
        workflow_text: format!("Memory proposal resolved: {} ({})", title, status),
        visible_summary,
    }
}

pub fn memory_review_requested_projection(
    provenance: ProjectionProvenance,
    proposal_id: Uuid,
    source_profile: &str,
    suggested_action: &str,
    title: &str,
    status: &str,
    source_paths: Vec<String>,
) -> Projection {
    Projection {
        provenance,
        event: ProjectionEvent::MemoryReviewRequested(MemoryReviewRequestedPayload {
            proposal_id,
            source_profile: source_profile.to_string(),
            title: title.to_string(),
            suggested_action: suggested_action.to_string(),
            status: status.to_string(),
            source_paths,
        }),
        workflow_text: format!("Memory review requested: {title}"),
        visible_summary: Some(format!(
            "Review requested for memory proposal '{}' from {}.",
            title, source_profile
        )),
    }
}

pub fn memory_curate_enqueued_projection(
    provenance: ProjectionProvenance,
    reflection_run_id: Uuid,
    lane: String,
    trigger: String,
    status: String,
    proposal_ids: Vec<Uuid>,
    conversation_key: Option<String>,
    conversation_date: Option<time::Date>,
    created_at: time::OffsetDateTime,
) -> Projection {
    let proposal_count = proposal_ids.len();
    Projection {
        provenance,
        event: ProjectionEvent::MemoryCurateEnqueued(MemoryCurateEnqueuedPayload {
            reflection_run_id,
            lane,
            trigger,
            status,
            proposal_ids,
            conversation_key,
            conversation_date,
            created_at,
        }),
        workflow_text: format!("Memory curate enqueued with {proposal_count} proposal(s)"),
        visible_summary: Some(format!(
            "Memory curate was queued for {proposal_count} proposal(s)."
        )),
    }
}

pub fn memory_curate_started_projection(
    provenance: ProjectionProvenance,
    reflection_run_id: Uuid,
    lane: String,
    trigger: String,
    status: String,
    proposal_ids: Vec<Uuid>,
    conversation_key: Option<String>,
    conversation_date: Option<time::Date>,
    started_at: Option<time::OffsetDateTime>,
) -> Projection {
    let proposal_count = proposal_ids.len();
    Projection {
        provenance,
        event: ProjectionEvent::MemoryCurateStarted(MemoryCurateStartedPayload {
            reflection_run_id,
            lane,
            trigger,
            status,
            proposal_ids,
            conversation_key,
            conversation_date,
            started_at,
        }),
        workflow_text: format!("Memory curate started with {proposal_count} proposal(s)"),
        visible_summary: Some(format!(
            "Memory curate started for {proposal_count} proposal(s)."
        )),
    }
}

pub fn memory_curate_completed_projection(
    provenance: ProjectionProvenance,
    reflection_run_id: Uuid,
    lane: String,
    trigger: String,
    status: String,
    proposal_ids: Vec<Uuid>,
    conversation_key: Option<String>,
    conversation_date: Option<time::Date>,
    completed_at: Option<time::OffsetDateTime>,
) -> Projection {
    let proposal_count = proposal_ids.len();
    Projection {
        provenance,
        event: ProjectionEvent::MemoryCurateCompleted(MemoryCurateCompletedPayload {
            reflection_run_id,
            lane,
            trigger,
            status,
            proposal_ids,
            conversation_key,
            conversation_date,
            completed_at,
        }),
        workflow_text: format!("Memory curate completed with {proposal_count} proposal(s)"),
        visible_summary: Some(format!(
            "Memory curate completed for {proposal_count} proposal(s)."
        )),
    }
}

pub fn memory_curate_failed_projection(
    provenance: ProjectionProvenance,
    reflection_run_id: Uuid,
    lane: String,
    trigger: String,
    status: String,
    proposal_ids: Vec<Uuid>,
    conversation_key: Option<String>,
    conversation_date: Option<time::Date>,
    error: Option<String>,
    completed_at: Option<time::OffsetDateTime>,
) -> Projection {
    let proposal_count = proposal_ids.len();
    let visible_summary = Some(match error.as_deref() {
        Some(message) if !message.trim().is_empty() => {
            format!("Memory curate failed for {proposal_count} proposal(s): {message}")
        }
        _ => format!("Memory curate failed for {proposal_count} proposal(s)."),
    });
    Projection {
        provenance,
        event: ProjectionEvent::MemoryCurateFailed(MemoryCurateFailedPayload {
            reflection_run_id,
            lane,
            trigger,
            status,
            proposal_ids,
            conversation_key,
            conversation_date,
            error,
            completed_at,
        }),
        workflow_text: format!("Memory curate failed with {proposal_count} proposal(s)"),
        visible_summary,
    }
}

impl Projection {
    pub fn workflow_content_json(&self) -> serde_json::Value {
        let mut value = match serde_json::to_value(&self.event) {
            Ok(serde_json::Value::Object(map)) => serde_json::Value::Object(map),
            Ok(other) => {
                tracing::warn!(
                    kind = self.event.kind().as_str(),
                    value = %other,
                    "projection event serialized to non-object JSON"
                );
                serde_json::json!({
                    "type": self.event.kind().as_str(),
                    "projection_serialization_shape": "non_object"
                })
            }
            Err(err) => {
                tracing::warn!(
                    kind = self.event.kind().as_str(),
                    error = %err,
                    "projection event serialization failed"
                );
                serde_json::json!({
                    "type": self.event.kind().as_str(),
                    "projection_serialization_error": err.to_string()
                })
            }
        };
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "source".to_string(),
                serde_json::json!(self.provenance.source.as_str()),
            );
            object.insert(
                "scope_id".to_string(),
                serde_json::json!(self.provenance.scope_id),
            );
        }
        value
    }

    pub fn workflow_record(&self) -> CanonicalConversationRecord {
        CanonicalConversationRecord::workflow_event(
            self.workflow_text.clone(),
            self.workflow_content_json(),
            None,
        )
    }

    pub fn visible_summary_record(&self) -> Option<CanonicalConversationRecord> {
        self.visible_summary.as_ref().map(|text| {
            CanonicalConversationRecord::visible_assistant_message(
                text.clone(),
                serde_json::json!({}),
                None,
            )
        })
    }
}

pub async fn persist_projection(
    context: &ConversationPersistenceContext,
    projection: &Projection,
) -> Result<(), DenError> {
    persist_canonical_conversation_record(context, &projection.workflow_record()).await?;
    if let Some(record) = projection.visible_summary_record() {
        persist_canonical_conversation_record(context, &record).await?;
    }
    Ok(())
}

pub fn spawn_persist_projection(context: ConversationPersistenceContext, projection: Projection) {
    if context.skip_persistence {
        return;
    }
    let span_request_id = context.request_id.clone();
    let span_scope = context.persistence_scope_id.clone();
    tokio::spawn(
        async move {
            if let Err(err) = persist_projection(&context, &projection).await {
                tracing::warn!(
                    request_id = ?context.request_id,
                    persistence_scope_id = %context.persistence_scope_id,
                    error = %err,
                    "projection persistence failed"
                );
            }
        }
        .instrument(tracing::info_span!(
            "persist_projection",
            request_id = ?span_request_id,
            persistence_scope_id = %span_scope,
        )),
    );
}

pub fn project_to_conversation(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: Option<&str>,
    projection: Projection,
) {
    let Some(conversation_id) = conversation_id.filter(|id| id.starts_with("conv-")) else {
        return;
    };
    let context = canonical_persistence_context(
        pool.clone(),
        bear_id,
        user_id,
        conversation_id.to_string(),
        None,
        None,
        projection.provenance.scope_id.clone(),
        false,
    );
    spawn_persist_projection(context, projection);
}

pub fn spawn_persist_assistant_summary_message(
    context: ConversationPersistenceContext,
    text: String,
    provider_message_id: Option<String>,
) {
    spawn_persist_canonical_conversation_record(
        context,
        CanonicalConversationRecord::visible_assistant_message(
            text,
            serde_json::json!({}),
            provider_message_id,
        ),
    );
}

#[cfg(test)]
mod tests;
