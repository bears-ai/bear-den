use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub fn deserialize_optional_i64_from_value<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }))
}

pub fn deserialize_required_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(serde::de::Error::custom("expected non-empty string"));
    }
    Ok(trimmed.to_string())
}

pub fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

pub fn deserialize_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_required_string(deserializer)
}

fn default_history_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
pub struct EventPageQuery {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_i64_from_value")]
    pub after: Option<i64>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_i64_from_value")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ConversationHistoryRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub conversation_id: String,
    #[serde(
        default,
        alias = "before_sequence_no",
        deserialize_with = "deserialize_optional_i64_from_value"
    )]
    pub before: Option<i64>,
    #[serde(default = "default_history_limit")]
    pub limit: i64,
    /// Surface-only derived records are independent of the message cursor. Callers paging
    /// backwards should request them once, on the newest page, rather than duplicating them.
    #[serde(default = "default_surface_history_enrichment")]
    pub include_surface_enrichment: bool,
}

fn default_surface_history_enrichment() -> bool {
    true
}

/// Bounded, transcript-free controller evidence for one authenticated conversation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationDiagnosticsRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub conversation_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub run_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub message_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub task_id: Option<String>,
    /// Include persisted runtime checkpoint artifacts for the authorized run.
    #[serde(default)]
    pub include_checkpoints: bool,
    #[serde(default = "default_history_limit")]
    pub limit: i64,
}

#[derive(Debug, Deserialize)]
pub struct ResourceUpdateRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    /// Intentionally raw: adapter-owned resource envelopes are extensible and forwarded verbatim.
    #[serde(default)]
    pub resource: Option<Value>,
    /// Legacy alias for `resource`; intentionally raw for the same reason.
    #[serde(default)]
    pub payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RunStartRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub prompt: String,
    /// Intentionally raw: prompt context is a typed outer field carrying extensible structured payloads.
    #[serde(default)]
    pub prompt_context: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub client: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub conversation_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub cwd: Option<String>,
    #[serde(
        default,
        alias = "mode",
        deserialize_with = "deserialize_optional_string"
    )]
    pub requested_mode: Option<String>,
    /// A new user action may explicitly replace the active session run. Omitted
    /// requests are retry/reconnect-safe and reuse that run instead.
    #[serde(default)]
    pub supersede_active_run: bool,
    /// Intentionally raw: adapter session/capability context is an extensible envelope.
    #[serde(default)]
    pub client_context: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RunStateRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub run_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct RunRecoverRequest {
    #[serde(rename = "bear_slug", deserialize_with = "deserialize_required_string")]
    pub _bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub run_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RunCancelRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub run_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SessionOpenRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub client: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub conversation_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub runtime_session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub cwd: Option<String>,
    #[serde(
        default,
        alias = "requested_mode",
        deserialize_with = "deserialize_optional_string"
    )]
    pub mode: Option<String>,
    /// Intentionally raw: adapter session snapshots are open-ended capability/context envelopes.
    #[serde(default)]
    pub client_context: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct SessionIdRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SessionStateRequest {
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub bear_slug: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub include_closed: Option<bool>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_i64_from_value")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DocketJobsListRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub include_cancelled: Option<bool>,
    #[serde(default)]
    pub include_archived: Option<bool>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_i64_from_value")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DocketJobDiagnosticsRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub job_id: String,
}

/// Bounded, Bear-scoped operational failures. Raw prompts and tool payloads are never stored.
#[derive(Debug, Deserialize)]
pub struct RuntimeDiagnosticsListRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub work_run_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub runtime_run_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub docket_job_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub event_code: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub severity: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_i64_from_value")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DocketJobsExecuteRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub job_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub conversation_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub source_client_session_id: Option<String>,
}

/// Cancels the current Docket lifecycle run. This is not sandbox work-run
/// cancellation and intentionally needs no Pair-session identifier.
#[derive(Debug, Deserialize)]
pub struct DocketJobsCancelRunRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub job_id: String,
}

/// Settles the Docket task currently claimed by an execution session and
/// returns the scheduler's successor control result.
#[derive(Debug, Deserialize)]
pub struct DocketJobsSettleTaskRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub job_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub task_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub status: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub outcome_disposition: Option<String>,
    #[serde(default)]
    pub result_refs: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub result_summary: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub conversation_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub source_client_session_id: Option<String>,
}

/// Settles a Pair session-owned task without fabricating a Docket job run.
#[derive(Debug, Deserialize)]
pub struct DocketSessionTasksSettleRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub task_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub status: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub outcome_disposition: Option<String>,
    #[serde(default)]
    pub result_refs: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub result_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionModelSetRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub selection_mode: Option<String>,
    #[serde(
        default,
        alias = "requested_model",
        deserialize_with = "deserialize_optional_string"
    )]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionCurrentTaskSelectionRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub task_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SessionCurrentTaskClearRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
}

/// Starts native Pair execution for the already selected actionable session task.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCurrentTaskStartRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub bear_slug: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionInput {
    Approved,
    Approve,
    Granted,
    Allow,
    AllowOnce,
    AllowSiteAccount,
    AllowHost,
    Denied,
    Deny,
    Rejected,
    Reject,
    RejectOnce,
    RejectAlways,
    Timeout,
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
struct PermissionDecisionDescriptor {
    raw: &'static str,
    normalized: &'static str,
}

const PERMISSION_DECISION_DESCRIPTORS: [PermissionDecisionDescriptor; 15] = [
    PermissionDecisionDescriptor {
        raw: "approved",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "approve",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "granted",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "allow",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "allow_once",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "allow_site_account",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "allow_host",
        normalized: "granted",
    },
    PermissionDecisionDescriptor {
        raw: "denied",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "deny",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "rejected",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "reject",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "reject_once",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "reject_always",
        normalized: "denied",
    },
    PermissionDecisionDescriptor {
        raw: "timeout",
        normalized: "expired",
    },
    PermissionDecisionDescriptor {
        raw: "timed_out",
        normalized: "expired",
    },
];

impl PermissionDecisionInput {
    const fn descriptor_index(self) -> usize {
        match self {
            Self::Approved => 0,
            Self::Approve => 1,
            Self::Granted => 2,
            Self::Allow => 3,
            Self::AllowOnce => 4,
            Self::AllowSiteAccount => 5,
            Self::AllowHost => 6,
            Self::Denied => 7,
            Self::Deny => 8,
            Self::Rejected => 9,
            Self::Reject => 10,
            Self::RejectOnce => 11,
            Self::RejectAlways => 12,
            Self::Timeout => 13,
            Self::TimedOut => 14,
        }
    }

    fn descriptor(self) -> &'static PermissionDecisionDescriptor {
        &PERMISSION_DECISION_DESCRIPTORS[self.descriptor_index()]
    }

    pub fn normalized(self) -> &'static str {
        self.descriptor().normalized
    }

    pub fn raw(self) -> &'static str {
        self.descriptor().raw
    }
}

fn default_permission_decision() -> PermissionDecisionInput {
    PermissionDecisionInput::Denied
}

#[derive(Debug, Deserialize)]
pub struct ClientPermissionResultRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    pub run_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    pub permission_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub obligation_id: Option<String>,
    #[serde(default = "default_permission_decision")]
    pub decision: PermissionDecisionInput,
    /// Intentionally raw: reason may be string or structured adapter metadata.
    #[serde(default)]
    pub reason: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_task_settlement_request_trims_required_fields() {
        let request: DocketSessionTasksSettleRequest = serde_json::from_value(serde_json::json!({
            "bear_slug": " bear-1 ",
            "session_id": " session-1 ",
            "task_id": " task-1 ",
            "status": " done ",
            "result_summary": " finished "
        }))
        .unwrap();
        assert_eq!(request.bear_slug, "bear-1");
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.task_id, "task-1");
        assert_eq!(request.status, "done");
        assert_eq!(request.result_summary.as_deref(), Some("finished"));
    }

    #[test]
    fn required_strings_are_trimmed_and_empty_strings_rejected() {
        let request: RunCancelRequest = serde_json::from_value(serde_json::json!({
            "session_id": " s-1 ",
            "run_id": " r-1 "
        }))
        .unwrap();
        assert_eq!(request.session_id, "s-1");
        assert_eq!(request.run_id, "r-1");

        let error = serde_json::from_value::<RunCancelRequest>(serde_json::json!({
            "session_id": "s-1",
            "run_id": "   "
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("expected non-empty string"));
    }

    #[test]
    fn method_aliases_and_numeric_strings_decode() {
        let history: ConversationHistoryRequest = serde_json::from_value(serde_json::json!({
            "conversation_id": "conv-1",
            "before_sequence_no": "42"
        }))
        .unwrap();
        assert_eq!(history.before, Some(42));
        assert_eq!(history.limit, 50);
        assert!(history.include_surface_enrichment);

        let older_page: ConversationHistoryRequest = serde_json::from_value(serde_json::json!({
            "conversation_id": "conv-1",
            "before": 42,
            "include_surface_enrichment": false
        }))
        .unwrap();
        assert!(!older_page.include_surface_enrichment);

        let diagnostics: ConversationDiagnosticsRequest =
            serde_json::from_value(serde_json::json!({
                "bear_slug": "bear-1",
                "conversation_id": "conv-1",
                "message_id": " message-1 ",
                "task_id": " task-1 ",
                "limit": 10
            }))
            .unwrap();
        assert_eq!(diagnostics.message_id.as_deref(), Some("message-1"));
        assert_eq!(diagnostics.task_id.as_deref(), Some("task-1"));
        assert_eq!(diagnostics.limit, 10);

        assert!(
            serde_json::from_value::<ConversationDiagnosticsRequest>(serde_json::json!({
                "bear_slug": "bear-1",
                "conversation_id": "conv-1",
                "unexpected": true
            }))
            .is_err()
        );

        let run: RunStartRequest = serde_json::from_value(serde_json::json!({
            "session_id": "s-1",
            "prompt": "hi",
            "mode": " ask "
        }))
        .unwrap();
        assert_eq!(run.requested_mode.as_deref(), Some("ask"));
    }
}
