use den_core::tools::result_compaction::ToolResultStatus;
use den_core::DenError;
use den_protocol::ContextBudgetReport;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgRow, types::Json, PgPool, Row};
use uuid::Uuid;

use crate::archived_conversations;
use crate::conversation_message_types::{
    ConversationMessageRole, ConversationMessageType, ConversationMessageVisibility,
    ConversationMessageWrite,
};

#[derive(Debug, Clone, Serialize)]
pub struct ConversationRecord {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub external_conversation_id: Option<String>,
    pub source_client_session_id: Option<String>,
    pub current_title: Option<String>,
    pub latest_context_budget: Option<ContextBudgetReport>,
    pub latest_context_budget_updated_at: Option<time::OffsetDateTime>,
    pub updated_at: time::OffsetDateTime,
}

/// Shared SQL row for Bear-wide and human-visible conversation listings.
#[derive(sqlx::FromRow)]
pub(super) struct ConversationRow {
    pub(super) id: Uuid,
    pub(super) bear_id: Uuid,
    pub(super) external_conversation_id: Option<String>,
    pub(super) source_client_session_id: Option<String>,
    pub(super) current_title: Option<String>,
    pub(super) latest_context_budget_json: Option<Json<serde_json::Value>>,
    pub(super) latest_context_budget_updated_at: Option<time::OffsetDateTime>,
    pub(super) updated_at: time::OffsetDateTime,
}

impl TryFrom<ConversationRow> for ConversationRecord {
    type Error = DenError;

    fn try_from(row: ConversationRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            bear_id: row.bear_id,
            external_conversation_id: row.external_conversation_id,
            source_client_session_id: row.source_client_session_id,
            current_title: row.current_title,
            latest_context_budget: row
                .latest_context_budget_json
                .map(|value| {
                    serde_json::from_value(value.0).map_err(|err| {
                        DenError::Parsing(format!(
                            "decode conversation latest_context_budget_json payload: {err}"
                        ))
                    })
                })
                .transpose()?,
            latest_context_budget_updated_at: row.latest_context_budget_updated_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationModelState {
    pub conversation_id: Uuid,
    pub selection_mode: String,
    pub requested_model: Option<String>,
    pub selected_model: Option<String>,
    pub selected_reason: Option<String>,
    pub actual_last_model: Option<String>,
    pub actual_last_provider: Option<String>,
    pub fallback_count: i32,
    pub metadata_json: serde_json::Value,
}

fn db_err(context: &'static str) -> impl FnOnce(sqlx::Error) -> DenError {
    move |err| match DenError::from(err) {
        DenError::Database(message) => DenError::Database(format!("{context}: {message}")),
        DenError::DatabaseUnavailable(message) => {
            DenError::DatabaseUnavailable(format!("{context}: {message}"))
        }
        other => other,
    }
}

fn db_decode(field: &'static str) -> impl FnOnce(sqlx::Error) -> DenError {
    move |err| DenError::Database(format!("decode conversation {field}: {err}"))
}

async fn rollback_append_message_tx(
    tx: sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), DenError> {
    tx.rollback()
        .await
        .map_err(db_err("rollback append conversation message tx"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationHistoryProjection {
    UserHistory,
    ModelTranscript,
}

/// Identity and ordering assigned to a canonical conversation message.
///
/// `id` is stable across idempotent retries; `sequence_no` orders messages within
/// one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendedConversationMessage {
    pub id: Uuid,
    pub sequence_no: i64,
}

#[derive(Debug, Clone)]
pub struct PersistedConversationMessage {
    pub sequence_no: i64,
    pub message_type: String,
    pub role: Option<String>,
    pub visibility: String,
    pub content_text: String,
    pub content_json: serde_json::Value,
    pub provider_message_id: Option<String>,
    pub created_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedTranscriptMessage {
    pub sequence_no: i64,
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PersistedToolRequestPayload {
    pub event: String,
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub approval_request_id: Option<String>,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default)]
    pub approval_required: bool,
    #[serde(default)]
    pub approval_reason: Option<String>,
    #[serde(default)]
    pub route: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PersistedToolResultPayload {
    pub event: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub status: ToolResultStatus,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub structured_content: serde_json::Value,
    #[serde(default)]
    pub output_summary: Option<String>,
    #[serde(default)]
    pub output_preview: Option<String>,
    #[serde(default)]
    pub approval_request_id: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub diagnostic: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistedTranscriptRecord {
    Message(PersistedTranscriptMessage),
    ToolCall {
        sequence_no: i64,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        created_at: time::OffsetDateTime,
    },
    ToolResult {
        sequence_no: i64,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        status: Option<String>,
        content: Option<String>,
        structured_content: serde_json::Value,
        created_at: time::OffsetDateTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedUserHistoryMessage {
    pub sequence_no: i64,
    pub kind: String,
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub status: Option<String>,
    pub arguments: serde_json::Value,
    pub raw_output: serde_json::Value,
    pub created_at: time::OffsetDateTime,
}

fn strict_typed_payloads_enabled() -> bool {
    std::env::var("BEARS_STRICT_TYPED_PAYLOADS")
        .ok()
        .as_deref()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}

fn tool_result_user_history_summary(content_json: &serde_json::Value) -> Option<String> {
    let payload = PersistedToolResultPayload::try_from(content_json).ok()?;
    if let Some(summary) = payload
        .output_summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(summary.to_string());
    }
    let tool_name = payload.tool_name.as_deref().unwrap_or("tool");
    let status = payload.status.as_str();
    let content = payload
        .output_preview
        .as_deref()
        .or(payload.content.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let prefix = format!("Used {tool_name} ({status})");
    Some(match content {
        Some(content) => format!("{prefix}: {content}"),
        None => prefix,
    })
}

impl PersistedConversationMessage {
    pub fn tool_request_payload(&self) -> Result<Option<PersistedToolRequestPayload>, DenError> {
        if self.storage_message_type().ok() != Some(ConversationMessageType::ToolCall) {
            return Ok(None);
        }
        let payload = PersistedToolRequestPayload::try_from(self.content_json_value())?;
        Ok(Some(payload))
    }

    pub fn tool_result_payload(&self) -> Result<Option<PersistedToolResultPayload>, DenError> {
        if self.storage_message_type().ok() != Some(ConversationMessageType::ToolResult) {
            return Ok(None);
        }
        let payload = PersistedToolResultPayload::try_from(self.content_json_value())?;
        Ok(Some(payload))
    }

    pub fn storage_message_type(&self) -> Result<ConversationMessageType, DenError> {
        ConversationMessageType::try_from_storage(&self.message_type)
    }

    pub fn storage_visibility(&self) -> Result<ConversationMessageVisibility, DenError> {
        ConversationMessageVisibility::try_from_storage(&self.visibility)
    }

    pub fn storage_role(&self) -> Result<Option<ConversationMessageRole>, DenError> {
        match self.role.as_deref() {
            None => Ok(None),
            Some(role) => ConversationMessageRole::try_from_storage(role).map(Some),
        }
    }

    pub fn transcript_role(&self) -> Option<&'static str> {
        match (self.message_type.as_str(), self.role.as_deref()) {
            ("user", _) => Some("user"),
            ("assistant", _) => Some("assistant"),
            ("message", Some("user")) => Some("user"),
            ("message", Some("assistant")) => Some("assistant"),
            (_, Some("user"))
                if self.storage_message_type().ok() == Some(ConversationMessageType::User) =>
            {
                Some("user")
            }
            (_, Some("assistant"))
                if self.storage_message_type().ok() == Some(ConversationMessageType::Assistant) =>
            {
                Some("assistant")
            }
            _ => None,
        }
    }

    pub fn to_model_transcript_message(&self) -> Option<PersistedTranscriptMessage> {
        let visibility = self.storage_visibility().ok()?;
        if !visibility.is_model_transcript_visible() {
            return None;
        }
        let role = self.transcript_role()?;
        Some(PersistedTranscriptMessage {
            sequence_no: self.sequence_no,
            message_id: self.provider_message_id.clone(),
            role: role.to_string(),
            content: self.content_text.clone(),
            created_at: self.created_at,
        })
    }

    pub fn to_model_transcript_record(&self) -> Option<PersistedTranscriptRecord> {
        if !self
            .storage_visibility()
            .ok()?
            .is_model_transcript_visible()
        {
            return None;
        }
        if let Some(message) = self.to_model_transcript_message() {
            return Some(PersistedTranscriptRecord::Message(message));
        }

        match self.storage_message_type().ok()? {
            ConversationMessageType::ToolCall => {
                let payload = self.tool_request_payload().ok()??;
                Some(PersistedTranscriptRecord::ToolCall {
                    sequence_no: self.sequence_no,
                    tool_call_id: payload.tool_call_id,
                    tool_name: payload.tool_name,
                    arguments: payload.args,
                    created_at: self.created_at,
                })
            }
            ConversationMessageType::ToolResult => {
                let payload = self.tool_result_payload().ok()??;
                let content = payload.content.clone().or_else(|| {
                    if self.content_text.trim().is_empty() {
                        None
                    } else {
                        Some(self.content_text.clone())
                    }
                });
                Some(PersistedTranscriptRecord::ToolResult {
                    sequence_no: self.sequence_no,
                    tool_call_id: payload.tool_call_id,
                    tool_name: payload.tool_name,
                    status: Some(payload.status.as_str().to_string()),
                    content,
                    structured_content: payload.structured_content,
                    created_at: self.created_at,
                })
            }
            _ => None,
        }
    }

    pub fn to_user_history_transcript_message(&self) -> Option<PersistedTranscriptMessage> {
        let visibility = self.storage_visibility().ok()?;
        if !visibility.is_user_history_visible() {
            return None;
        }
        self.to_model_transcript_message()
    }

    pub fn to_model_history_record(&self) -> Option<PersistedUserHistoryMessage> {
        if let Some(message) = self.to_model_transcript_message() {
            return Some(PersistedUserHistoryMessage {
                sequence_no: message.sequence_no,
                kind: "message".to_string(),
                message_id: message.message_id,
                role: message.role,
                content: message.content,
                tool_call_id: None,
                tool_name: None,
                status: None,
                arguments: serde_json::Value::Null,
                raw_output: serde_json::Value::Null,
                created_at: message.created_at,
            });
        }
        if !self
            .storage_visibility()
            .ok()?
            .is_model_transcript_visible()
        {
            return None;
        }
        self.to_tool_history_record()
    }

    pub fn to_user_history_record(&self) -> Option<PersistedUserHistoryMessage> {
        if let Some(message) = self.to_user_history_transcript_message() {
            return Some(PersistedUserHistoryMessage {
                sequence_no: message.sequence_no,
                kind: "message".to_string(),
                message_id: message.message_id,
                role: message.role,
                content: message.content,
                tool_call_id: None,
                tool_name: None,
                status: None,
                arguments: serde_json::Value::Null,
                raw_output: serde_json::Value::Null,
                created_at: message.created_at,
            });
        }

        if !self.storage_visibility().ok()?.is_user_history_visible() {
            return None;
        }
        self.to_tool_history_record()
    }

    fn to_tool_history_record(&self) -> Option<PersistedUserHistoryMessage> {
        match self.storage_message_type().ok()? {
            ConversationMessageType::ToolCall => {
                let payload = self.tool_request_payload().ok()??;
                Some(PersistedUserHistoryMessage {
                    sequence_no: self.sequence_no,
                    kind: "tool_call".to_string(),
                    message_id: Some(payload.tool_call_id.clone()),
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_call_id: Some(payload.tool_call_id),
                    tool_name: Some(payload.tool_name),
                    status: Some("pending".to_string()),
                    arguments: payload.args,
                    raw_output: serde_json::Value::Null,
                    created_at: self.created_at,
                })
            }
            ConversationMessageType::ToolResult => {
                let payload = self.tool_result_payload().ok()??;
                let raw_output = if !payload.structured_content.is_null() {
                    payload.structured_content.clone()
                } else if let Some(content) =
                    payload.content.as_ref().or(payload.output_preview.as_ref())
                {
                    serde_json::json!({ "content": content })
                } else {
                    serde_json::Value::Null
                };
                Some(PersistedUserHistoryMessage {
                    sequence_no: self.sequence_no,
                    kind: "tool_result".to_string(),
                    message_id: payload
                        .tool_call_id
                        .clone()
                        .or_else(|| self.provider_message_id.clone()),
                    role: "assistant".to_string(),
                    content: tool_result_user_history_summary(self.content_json_value())
                        .unwrap_or_default(),
                    tool_call_id: payload.tool_call_id,
                    tool_name: payload.tool_name,
                    status: Some(payload.status.as_str().to_string()),
                    arguments: serde_json::Value::Null,
                    raw_output,
                    created_at: self.created_at,
                })
            }
            _ => None,
        }
    }

    /// Rows that may be replayed into model transcript context.
    pub fn is_model_transcript_visible(&self) -> bool {
        self.to_model_transcript_record().is_some()
    }

    /// Rows that may be shown in user-facing conversation history.
    pub fn is_user_history_visible(&self) -> bool {
        self.to_user_history_record().is_some()
    }

    pub fn is_transcript_visible(&self) -> bool {
        self.is_model_transcript_visible()
    }

    fn content_json_value(&self) -> &serde_json::Value {
        &self.content_json
    }
}

impl TryFrom<&serde_json::Value> for PersistedToolRequestPayload {
    type Error = DenError;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let payload: Self = serde_json::from_value(value.clone()).map_err(|err| {
            DenError::Parsing(format!("decode persisted tool_request payload: {err}"))
        })?;
        if payload.event != "tool_request" {
            return Err(DenError::ValidationError(format!(
                "unexpected persisted tool request event: {}",
                payload.event
            )));
        }
        Ok(payload)
    }
}

impl TryFrom<&serde_json::Value> for PersistedToolResultPayload {
    type Error = DenError;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let payload: Self = serde_json::from_value(value.clone()).map_err(|err| {
            DenError::Parsing(format!("decode persisted tool_result payload: {err}"))
        })?;
        if payload.event != "tool_result" {
            return Err(DenError::ValidationError(format!(
                "unexpected persisted tool result event: {}",
                payload.event
            )));
        }
        if strict_typed_payloads_enabled()
            && payload.status != ToolResultStatus::Incomplete
            && payload
                .output_summary
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
        {
            return Err(DenError::ValidationError(
                "strict typed payloads require output_summary on persisted tool_result rows"
                    .to_string(),
            ));
        }
        Ok(payload)
    }
}

pub async fn ensure_conversation_for_external_id(
    pool: &PgPool,
    bear_id: Uuid,
    created_by_user_id: Option<i32>,
    external_conversation_id: &str,
    source_client_session_id: Option<&str>,
    current_title: Option<&str>,
) -> Result<ConversationRecord, DenError> {
    let inserted_row = sqlx::query(
        r"
        INSERT INTO conversations (
            bear_id,
            created_by_user_id,
            external_conversation_id,
            source_client_session_id,
            current_title
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        RETURNING id, bear_id, external_conversation_id, source_client_session_id, current_title, latest_context_budget_json, latest_context_budget_updated_at, updated_at
        ",
    )
    .bind(bear_id)
    .bind(created_by_user_id)
    .bind(external_conversation_id)
    .bind(source_client_session_id)
    .bind(current_title)
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("upsert conversation insert: {err}")))?;

    let row = if let Some(row) = inserted_row {
        row
    } else {
        sqlx::query(
            r"
            UPDATE conversations
            SET source_client_session_id = COALESCE($3, conversations.source_client_session_id),
                current_title = COALESCE($4, conversations.current_title)
            WHERE bear_id = $1
              AND external_conversation_id = $2
            RETURNING id, bear_id, external_conversation_id, source_client_session_id, current_title, latest_context_budget_json, latest_context_budget_updated_at, updated_at
            ",
        )
        .bind(bear_id)
        .bind(external_conversation_id)
        .bind(source_client_session_id)
        .bind(current_title)
        .fetch_one(pool)
        .await
        .map_err(|err| DenError::Database(format!("upsert conversation update: {err}")))?
    };

    Ok(ConversationRecord {
        id: row
            .try_get("id")
            .map_err(|err| DenError::Database(format!("decode conversation id: {err}")))?,
        bear_id: row
            .try_get("bear_id")
            .map_err(|err| DenError::Database(format!("decode conversation bear_id: {err}")))?,
        external_conversation_id: row.try_get("external_conversation_id").map_err(|err| {
            DenError::Database(format!(
                "decode conversation external_conversation_id: {err}"
            ))
        })?,
        source_client_session_id: row.try_get("source_client_session_id").map_err(|err| {
            DenError::Database(format!(
                "decode conversation source_client_session_id: {err}"
            ))
        })?,
        current_title: row.try_get("current_title").map_err(|err| {
            DenError::Database(format!("decode conversation current_title: {err}"))
        })?,
        latest_context_budget: row
            .try_get::<Option<Json<serde_json::Value>>, _>("latest_context_budget_json")
            .map_err(|err| {
                DenError::Database(format!(
                    "decode conversation latest_context_budget_json: {err}"
                ))
            })?
            .map(|value| {
                serde_json::from_value(value.0).map_err(|err| {
                    DenError::Parsing(format!(
                        "decode conversation latest_context_budget_json payload: {err}"
                    ))
                })
            })
            .transpose()?,
        latest_context_budget_updated_at: row.try_get("latest_context_budget_updated_at").map_err(
            |err| {
                DenError::Database(format!(
                    "decode conversation latest_context_budget_updated_at: {err}"
                ))
            },
        )?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|err| DenError::Database(format!("decode conversation updated_at: {err}")))?,
    })
}

pub async fn get_conversation_by_id(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<ConversationRecord>, DenError> {
    let row = sqlx::query(
        r"
        SELECT id, bear_id, external_conversation_id, source_client_session_id, current_title, latest_context_budget_json, latest_context_budget_updated_at, updated_at
        FROM conversations
        WHERE id = $1
        LIMIT 1
        ",
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("get conversation by id: {err}")))?;

    row.map(|row| {
        Ok(ConversationRecord {
            id: row
                .try_get("id")
                .map_err(|err| DenError::Database(format!("decode conversation id: {err}")))?,
            bear_id: row
                .try_get("bear_id")
                .map_err(|err| DenError::Database(format!("decode conversation bear_id: {err}")))?,
            external_conversation_id: row.try_get("external_conversation_id").map_err(|err| {
                DenError::Database(format!(
                    "decode conversation external_conversation_id: {err}"
                ))
            })?,
            source_client_session_id: row.try_get("source_client_session_id").map_err(|err| {
                DenError::Database(format!(
                    "decode conversation source_client_session_id: {err}"
                ))
            })?,
            current_title: row.try_get("current_title").map_err(|err| {
                DenError::Database(format!("decode conversation current_title: {err}"))
            })?,
            latest_context_budget: row
                .try_get::<Option<Json<serde_json::Value>>, _>("latest_context_budget_json")
                .map_err(|err| {
                    DenError::Database(format!(
                        "decode conversation latest_context_budget_json: {err}"
                    ))
                })?
                .map(|value| {
                    serde_json::from_value(value.0).map_err(|err| {
                        DenError::Parsing(format!(
                            "decode conversation latest_context_budget_json payload: {err}"
                        ))
                    })
                })
                .transpose()?,
            latest_context_budget_updated_at: row
                .try_get("latest_context_budget_updated_at")
                .map_err(|err| {
                    DenError::Database(format!(
                        "decode conversation latest_context_budget_updated_at: {err}"
                    ))
                })?,
            updated_at: row.try_get("updated_at").map_err(|err| {
                DenError::Database(format!("decode conversation updated_at: {err}"))
            })?,
        })
    })
    .transpose()
}

pub async fn get_conversation_for_external_id(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
) -> Result<Option<ConversationRecord>, DenError> {
    let row = sqlx::query(
        r"
        SELECT id, bear_id, external_conversation_id, source_client_session_id, current_title, latest_context_budget_json, latest_context_budget_updated_at, updated_at
        FROM conversations
        WHERE bear_id = $1
          AND external_conversation_id = $2
        LIMIT 1
        ",
    )
    .bind(bear_id)
    .bind(external_conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("get conversation by external id: {err}")))?;

    row.map(|row| {
        Ok(ConversationRecord {
            id: row
                .try_get("id")
                .map_err(|err| DenError::Database(format!("decode conversation id: {err}")))?,
            bear_id: row
                .try_get("bear_id")
                .map_err(|err| DenError::Database(format!("decode conversation bear_id: {err}")))?,
            external_conversation_id: row.try_get("external_conversation_id").map_err(|err| {
                DenError::Database(format!(
                    "decode conversation external_conversation_id: {err}"
                ))
            })?,
            source_client_session_id: row.try_get("source_client_session_id").map_err(|err| {
                DenError::Database(format!(
                    "decode conversation source_client_session_id: {err}"
                ))
            })?,
            current_title: row.try_get("current_title").map_err(|err| {
                DenError::Database(format!("decode conversation current_title: {err}"))
            })?,
            latest_context_budget: row
                .try_get::<Option<Json<serde_json::Value>>, _>("latest_context_budget_json")
                .map_err(|err| {
                    DenError::Database(format!(
                        "decode conversation latest_context_budget_json: {err}"
                    ))
                })?
                .map(|value| {
                    serde_json::from_value(value.0).map_err(|err| {
                        DenError::Parsing(format!(
                            "decode conversation latest_context_budget_json payload: {err}"
                        ))
                    })
                })
                .transpose()?,
            latest_context_budget_updated_at: row
                .try_get("latest_context_budget_updated_at")
                .map_err(|err| {
                    DenError::Database(format!(
                        "decode conversation latest_context_budget_updated_at: {err}"
                    ))
                })?,
            updated_at: row.try_get("updated_at").map_err(|err| {
                DenError::Database(format!("decode conversation updated_at: {err}"))
            })?,
        })
    })
    .transpose()
}

pub async fn delete_conversation_for_external_id(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
) -> Result<u64, DenError> {
    let result = sqlx::query(
        r"
        DELETE FROM conversations
        WHERE bear_id = $1
          AND external_conversation_id = $2
        ",
    )
    .bind(bear_id)
    .bind(external_conversation_id)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("delete conversation by external id: {err}")))?;
    Ok(result.rows_affected())
}

pub async fn delete_conversation_and_clear_archive(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    archived_by_user_id: Option<i32>,
    source: &str,
) -> Result<u64, DenError> {
    let deleted =
        delete_conversation_for_external_id(pool, bear_id, external_conversation_id).await?;
    archived_conversations::set_archived(
        pool,
        bear_id,
        external_conversation_id,
        archived_by_user_id,
        source,
        false,
    )
    .await?;
    Ok(deleted)
}

pub async fn set_conversation_title(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    title: &str,
) -> Result<u64, DenError> {
    let normalized = title.trim();
    if normalized.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r"
        UPDATE conversations
        SET current_title = $3,
            updated_at = NOW()
        WHERE bear_id = $1
          AND external_conversation_id = $2
        ",
    )
    .bind(bear_id)
    .bind(external_conversation_id)
    .bind(normalized)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("update conversation title: {err}")))?;
    Ok(result.rows_affected())
}

pub async fn set_conversation_title_and_sync_client_sessions(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    title: &str,
) -> Result<u64, DenError> {
    let _ = set_conversation_title(pool, bear_id, external_conversation_id, title).await?;
    crate::client_sessions::set_title_for_bear_conversation(
        pool,
        bear_id,
        external_conversation_id,
        title,
    )
    .await
}

pub async fn update_latest_context_budget(
    pool: &PgPool,
    bear_id: Uuid,
    external_conversation_id: &str,
    source_client_session_id: Option<&str>,
    budget: &ContextBudgetReport,
) -> Result<(), DenError> {
    let conversation = ensure_conversation_for_external_id(
        pool,
        bear_id,
        None,
        external_conversation_id,
        source_client_session_id,
        None,
    )
    .await?;
    let budget_json = serde_json::to_value(budget)
        .map_err(|err| DenError::System(format!("serialize context budget report: {err}")))?;
    sqlx::query(
        r"
        UPDATE conversations
        SET latest_context_budget_json = $2,
            latest_context_budget_updated_at = NOW()
        WHERE id = $1
        ",
    )
    .bind(conversation.id)
    .bind(budget_json)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("update latest context budget: {err}")))?;
    Ok(())
}

pub async fn list_conversations_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<ConversationRecord>, DenError> {
    let rows = sqlx::query_as!(
        ConversationRow,
        r#"
        SELECT id, bear_id, external_conversation_id, source_client_session_id, current_title,
               latest_context_budget_json AS "latest_context_budget_json?: Json<serde_json::Value>",
               latest_context_budget_updated_at, updated_at
        FROM conversations
        WHERE bear_id = $1
        ORDER BY updated_at DESC
        LIMIT $2
        "#,
        bear_id,
        limit.clamp(1, 200),
    )
    .fetch_all(pool)
    .await
    .map_err(|err| DenError::Database(format!("list conversations for bear: {err}")))?;

    rows.into_iter().map(ConversationRecord::try_from).collect()
}

pub async fn list_messages_page(
    pool: &PgPool,
    conversation_id: Uuid,
    before_sequence_no: Option<i64>,
    limit: i64,
) -> Result<Vec<PersistedConversationMessage>, DenError> {
    let rows = sqlx::query(
        r"
        SELECT sequence_no, message_type, role, visibility, content_text, content_json, provider_message_id, created_at
        FROM conversation_messages
        WHERE conversation_id = $1
          AND ($2::bigint IS NULL OR sequence_no < $2)
        ORDER BY sequence_no DESC
        LIMIT $3
        ",
    )
    .bind(conversation_id)
    .bind(before_sequence_no)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await
    .map_err(db_err("list conversation messages"))?;

    decode_message_rows(rows)
}

pub async fn list_projected_messages_page(
    pool: &PgPool,
    conversation_id: Uuid,
    before_sequence_no: Option<i64>,
    limit: i64,
    projection: ConversationHistoryProjection,
) -> Result<Vec<PersistedConversationMessage>, DenError> {
    const RAW_PAGE_SIZE: i64 = 100;
    let projected_limit = limit.clamp(1, 100) as usize;
    let mut cursor = before_sequence_no;
    let mut projected = Vec::with_capacity(projected_limit);

    loop {
        let rows = list_messages_page(pool, conversation_id, cursor, RAW_PAGE_SIZE).await?;
        let raw_count = rows.len();
        cursor = rows.last().map(|row| row.sequence_no);
        for row in rows {
            let included = match projection {
                ConversationHistoryProjection::UserHistory => row.is_user_history_visible(),
                ConversationHistoryProjection::ModelTranscript => row.is_model_transcript_visible(),
            };
            if included {
                projected.push(row);
                if projected.len() == projected_limit {
                    return Ok(projected);
                }
            }
        }
        if raw_count < RAW_PAGE_SIZE as usize || cursor.is_none() {
            return Ok(projected);
        }
    }
}

fn decode_message_rows(rows: Vec<PgRow>) -> Result<Vec<PersistedConversationMessage>, DenError> {
    rows.into_iter()
        .map(|row| {
            Ok(PersistedConversationMessage {
                sequence_no: row
                    .try_get("sequence_no")
                    .map_err(db_decode("message sequence_no"))?,
                message_type: row
                    .try_get("message_type")
                    .map_err(db_decode("message message_type"))?,
                role: row.try_get("role").map_err(db_decode("message role"))?,
                visibility: row
                    .try_get("visibility")
                    .map_err(db_decode("message visibility"))?,
                content_text: row
                    .try_get("content_text")
                    .map_err(db_decode("message content_text"))?,
                content_json: row
                    .try_get("content_json")
                    .map_err(db_decode("message content_json"))?,
                provider_message_id: row
                    .try_get("provider_message_id")
                    .map_err(db_decode("message provider_message_id"))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(db_decode("message created_at"))?,
            })
        })
        .collect()
}

pub async fn append_message(
    pool: &PgPool,
    conversation_id: Uuid,
    message: &ConversationMessageWrite,
) -> Result<AppendedConversationMessage, DenError> {
    let message_type = message.message_type.as_str();
    let role = message.role.map(|r| r.as_str());
    let visibility = message.visibility.as_str();
    let content_text = message.content_text.as_str();
    let content_json = message.content_json.clone();
    let provider_message_id = message.provider_message_id.as_deref();
    let source_event_id = message.source_event_id.as_deref();
    let created_at = message.created_at.as_deref();
    let mut tx = pool
        .begin()
        .await
        .map_err(db_err("begin append conversation message tx"))?;

    if let Some(source_event_id) = source_event_id {
        if let Some(existing) = sqlx::query(
            r"SELECT id, sequence_no FROM conversation_messages
               WHERE conversation_id = $1 AND source_event_id = $2 LIMIT 1",
        )
        .bind(conversation_id)
        .bind(source_event_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err("lookup conversation message source_event_id"))?
        {
            rollback_append_message_tx(tx).await?;
            return Ok(AppendedConversationMessage {
                id: existing.try_get("id").map_err(db_decode("message id"))?,
                sequence_no: existing
                    .try_get("sequence_no")
                    .map_err(db_decode("message sequence_no"))?,
            });
        }
    }

    let allocator_row = sqlx::query(
        r"UPDATE conversations SET next_message_sequence = next_message_sequence + 1,
           updated_at = NOW() WHERE id = $1
           RETURNING next_message_sequence - 1 AS sequence_no",
    )
    .bind(conversation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err("allocate conversation message sequence"))?;
    let sequence_no: i64 = allocator_row
        .try_get("sequence_no")
        .map_err(db_decode("allocated sequence_no"))?;

    let inserted = sqlx::query(
        r"INSERT INTO conversation_messages (
             conversation_id, sequence_no, message_type, role, visibility,
             content_text, content_json, source_event_id, provider_message_id, created_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10::timestamptz, NOW()))
           RETURNING id",
    )
    .bind(conversation_id)
    .bind(sequence_no)
    .bind(message_type)
    .bind(role)
    .bind(visibility)
    .bind(content_text)
    .bind(content_json)
    .bind(source_event_id)
    .bind(provider_message_id)
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await;

    let id = match inserted {
        Ok(row) => row
            .try_get("id")
            .map_err(db_decode("inserted message id"))?,
        Err(err) => {
            rollback_append_message_tx(tx).await?;
            if let Some(source_event_id) = source_event_id {
                if let Some(existing) = sqlx::query(
                    r"SELECT id, sequence_no FROM conversation_messages
                       WHERE conversation_id = $1 AND source_event_id = $2 LIMIT 1",
                )
                .bind(conversation_id)
                .bind(source_event_id)
                .fetch_optional(pool)
                .await
                .map_err(db_err(
                    "reload duplicate conversation message after insert error",
                ))? {
                    return Ok(AppendedConversationMessage {
                        id: existing.try_get("id").map_err(db_decode("message id"))?,
                        sequence_no: existing
                            .try_get("sequence_no")
                            .map_err(db_decode("message sequence_no"))?,
                    });
                }
            }
            return Err(DenError::Database(format!(
                "append conversation message: {err}"
            )));
        }
    };

    tx.commit()
        .await
        .map_err(db_err("commit append conversation message tx"))?;
    Ok(AppendedConversationMessage { id, sequence_no })
}

pub async fn insert_message_if_absent(
    pool: &PgPool,
    conversation_id: Uuid,
    sequence_no: i64,
    message: &ConversationMessageWrite,
) -> Result<(), DenError> {
    let message_type = message.message_type.as_str();
    let role = message.role.map(|r| r.as_str());
    let visibility = message.visibility.as_str();
    let content_text = message.content_text.as_str();
    let content_json = message.content_json.clone();
    let provider_message_id = message.provider_message_id.as_deref();
    let created_at = message.created_at.as_deref();
    sqlx::query(
        r"
        INSERT INTO conversation_messages (
            conversation_id,
            sequence_no,
            message_type,
            role,
            visibility,
            content_text,
            content_json,
            provider_message_id,
            created_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8,
            COALESCE($9::timestamptz, NOW())
        )
        ON CONFLICT (conversation_id, sequence_no) DO NOTHING
        ",
    )
    .bind(conversation_id)
    .bind(sequence_no)
    .bind(message_type)
    .bind(role)
    .bind(visibility)
    .bind(content_text)
    .bind(content_json)
    .bind(provider_message_id)
    .bind(created_at)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("insert conversation message: {err}")))?;
    Ok(())
}

pub async fn count_visible_messages(pool: &PgPool, conversation_id: Uuid) -> Result<i64, DenError> {
    sqlx::query_scalar::<_, i64>(
        r"
        SELECT COUNT(*)::bigint
        FROM conversation_messages
        WHERE conversation_id = $1
          AND visibility != 'diagnostic_only'
        ",
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await
    .map_err(|err| DenError::Database(format!("count visible conversation messages: {err}")))
}

#[cfg(test)]
mod tests;

pub async fn get_conversation_model_state(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<ConversationModelState>, DenError> {
    let row = sqlx::query(
        r"
        SELECT conversation_id, selection_mode, requested_model, selected_model,
               selected_reason, actual_last_model, actual_last_provider,
               fallback_count, metadata_json
        FROM conversation_model_state
        WHERE conversation_id = $1
        ",
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("get conversation model state: {err}")))?;

    row.map(decode_conversation_model_state).transpose()
}

/// Establishes an automatic selection only when the conversation has no model state.
/// This must not overwrite an explicit human selection or an existing automatic choice.
pub async fn establish_conversation_default_model_state(
    pool: &PgPool,
    conversation_id: Uuid,
    selected_model: &str,
    selected_reason: &str,
) -> Result<bool, DenError> {
    let inserted = sqlx::query(
        r"
        INSERT INTO conversation_model_state (
            conversation_id, selection_mode, requested_model, selected_model,
            selected_reason, updated_at
        ) VALUES ($1, 'auto', NULL, $2, $3, NOW())
        ON CONFLICT (conversation_id) DO NOTHING
        ",
    )
    .bind(conversation_id)
    .bind(selected_model.trim())
    .bind(selected_reason.trim())
    .execute(pool)
    .await
    .map_err(|err| {
        DenError::Database(format!("establish conversation default model state: {err}"))
    })?
    .rows_affected();
    Ok(inserted == 1)
}

pub async fn set_conversation_model_state(
    pool: &PgPool,
    conversation_id: Uuid,
    selection_mode: &str,
    requested_model: Option<&str>,
    selected_model: Option<&str>,
    selected_reason: Option<&str>,
) -> Result<ConversationModelState, DenError> {
    let mode = match selection_mode.trim() {
        "explicit" => "explicit",
        _ => "auto",
    };
    let requested_model = requested_model.map(str::trim).filter(|s| !s.is_empty());
    let selected_model = selected_model.map(str::trim).filter(|s| !s.is_empty());
    let selected_reason = selected_reason.map(str::trim).filter(|s| !s.is_empty());
    let row = sqlx::query(
        r"
        INSERT INTO conversation_model_state (
            conversation_id, selection_mode, requested_model, selected_model,
            selected_reason, updated_at
        ) VALUES ($1, $2, $3, $4, $5, NOW())
        ON CONFLICT (conversation_id) DO UPDATE
        SET selection_mode = EXCLUDED.selection_mode,
            requested_model = EXCLUDED.requested_model,
            selected_model = EXCLUDED.selected_model,
            selected_reason = EXCLUDED.selected_reason,
            updated_at = NOW()
        RETURNING conversation_id, selection_mode, requested_model, selected_model,
                  selected_reason, actual_last_model, actual_last_provider,
                  fallback_count, metadata_json
        ",
    )
    .bind(conversation_id)
    .bind(mode)
    .bind(requested_model)
    .bind(selected_model)
    .bind(selected_reason)
    .fetch_one(pool)
    .await
    .map_err(|err| DenError::Database(format!("set conversation model state: {err}")))?;
    decode_conversation_model_state(row)
}

pub async fn resolve_conversation_selected_model(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<String>, DenError> {
    let state = get_conversation_model_state(pool, conversation_id).await?;
    Ok(state.and_then(|state| {
        if state.selection_mode == "explicit" {
            state
                .selected_model
                .or(state.requested_model)
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        } else {
            state
                .selected_model
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        }
    }))
}

fn decode_conversation_model_state(
    row: sqlx::postgres::PgRow,
) -> Result<ConversationModelState, DenError> {
    let metadata_json: Json<serde_json::Value> = row
        .try_get("metadata_json")
        .map_err(|err| DenError::Database(format!("decode conversation model metadata: {err}")))?;
    Ok(ConversationModelState {
        conversation_id: row.try_get("conversation_id").map_err(|err| {
            DenError::Database(format!("decode conversation model conversation_id: {err}"))
        })?,
        selection_mode: row.try_get("selection_mode").map_err(|err| {
            DenError::Database(format!("decode conversation model selection_mode: {err}"))
        })?,
        requested_model: row.try_get("requested_model").map_err(|err| {
            DenError::Database(format!("decode conversation model requested_model: {err}"))
        })?,
        selected_model: row.try_get("selected_model").map_err(|err| {
            DenError::Database(format!("decode conversation model selected_model: {err}"))
        })?,
        selected_reason: row.try_get("selected_reason").map_err(|err| {
            DenError::Database(format!("decode conversation model selected_reason: {err}"))
        })?,
        actual_last_model: row.try_get("actual_last_model").map_err(|err| {
            DenError::Database(format!(
                "decode conversation model actual_last_model: {err}"
            ))
        })?,
        actual_last_provider: row.try_get("actual_last_provider").map_err(|err| {
            DenError::Database(format!(
                "decode conversation model actual_last_provider: {err}"
            ))
        })?,
        fallback_count: row.try_get("fallback_count").map_err(|err| {
            DenError::Database(format!("decode conversation model fallback_count: {err}"))
        })?,
        metadata_json: metadata_json.0,
    })
}
