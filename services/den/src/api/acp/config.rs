use std::{collections::HashMap, sync::Arc};

use tokio::sync::{oneshot, Mutex as TokioMutex};
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_tool_turns::AcpToolResultRequest,
        acp_tools::acp_tool_policy_json_for_provider,
        web_policy,
    },
};

use super::AcpStreamContext;

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn acp_stream_tokens_enabled() -> bool {
    env_flag("BEARS_ACP_STREAM_TOKENS")
}

pub(crate) fn acp_text_chunk_chars() -> usize {
    std::env::var("BEARS_ACP_TEXT_CHUNK_CHARS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(64, 2048))
        .unwrap_or(384)
}

pub(crate) fn acp_debug_ui_enabled() -> bool {
    env_flag("BEARS_ACP_DEBUG_UI")
}

pub(crate) fn acp_tool_timeout_ms_for_provider(tool_name: &str) -> u64 {
    std::env::var("BEARS_ACP_TOOL_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(1, 300_000))
        .unwrap_or_else(|| {
            acp_tool_policy_json_for_provider(tool_name)
                .get("tool_timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30_000)
        })
}

pub(crate) fn acp_debug_event_sample_chars() -> usize {
    std::env::var("ACP_DEBUG_EVENT_SAMPLE_CHARS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(128, 20_000))
        .unwrap_or(360)
}

pub(in crate::api::acp) type PendingWebFetchMap = Arc<TokioMutex<HashMap<String, PendingWebFetchApproval>>>;
pub(in crate::api::acp) static PENDING_WEB_FETCH_APPROVALS: std::sync::OnceLock<PendingWebFetchMap> =
    std::sync::OnceLock::new();

pub(in crate::api::acp) fn pending_web_fetch_approvals() -> PendingWebFetchMap {
    PENDING_WEB_FETCH_APPROVALS
        .get_or_init(|| Arc::new(TokioMutex::new(HashMap::new())))
        .clone()
}

pub(in crate::api::acp) struct PendingWebFetchApproval {
    pub(in crate::api::acp) user_id: i32,
    pub(in crate::api::acp) bear_id: Uuid,
    pub(in crate::api::acp) result_tx: oneshot::Sender<AcpToolResultRequest>,
    pub(in crate::api::acp) context: AcpStreamContext,
    pub(in crate::api::acp) provider_name: String,
    pub(in crate::api::acp) tool_call_id: String,
    pub(in crate::api::acp) approval_request_id: Option<String>,
    pub(in crate::api::acp) args: serde_json::Value,
    pub(in crate::api::acp) normalized_url: web_policy::NormalizedWebUrl,
}

#[allow(dead_code)]
pub(crate) fn _state_marker(_: &ApiState) {}
