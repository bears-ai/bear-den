use axum::response::Response;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{future::Future, pin::Pin, sync::Arc};
use time::format_description::well_known::Rfc3339;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    api::{auth::ApiError, service::ApiState},
    core::{
        acp_letta_events::AcpGatewayEvent,
        acp_tool_turns::{AcpToolResultRequest, AcpToolTurnCoordinator},
        acp_turn_controller::AcpToolExecutionRoute as ControllerToolExecutionRoute,
        role_runtime::{RoleRuntime, RoleTurnScope},
    },
    errors::CustomError,
};

use super::stream::support::AcpStreamDiagnostics;

#[derive(Debug, Serialize)]
pub(crate) struct AcpSessionHttp {
    pub(crate) acp_session_id: String,
    pub(crate) runtime_session_id: String,
    pub(crate) conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolved_conversation_id: Option<String>,
    pub(crate) client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conversation_title_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conversation_title_synced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) closed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) archived_at: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) plan_mode: Option<serde_json::Value>,
    pub(crate) session_policy: serde_json::Value,
    pub(crate) workflow_state: serde_json::Value,
}

pub(crate) fn format_acp_session_timestamp(t: time::OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct AcpResolvedTurnContext {
    pub(crate) policy: crate::core::acp_tools::AcpResolvedSessionPolicy,
    pub(crate) workflow_state: serde_json::Value,
    pub(crate) effective_mode: String,
}

#[derive(Clone)]
pub(in crate::api::acp) struct AcpStreamContext {
    pub(in crate::api::acp) pool: PgPool,
    pub(in crate::api::acp) tool_turns: AcpToolTurnCoordinator,
    pub(in crate::api::acp) user_id: i32,
    pub(in crate::api::acp) user_profile: Option<crate::core::user::User>,
    pub(in crate::api::acp) bear_id: Uuid,
    pub(in crate::api::acp) bear_slug: String,
    pub(in crate::api::acp) acp_session_id: String,
    pub(in crate::api::acp) client: String,
    pub(in crate::api::acp) conversation_selection: String,
    pub(in crate::api::acp) resolved_conversation_id: Option<String>,
    pub(in crate::api::acp) upstream_target: String,
    pub(in crate::api::acp) workspace_roots: Vec<String>,
    pub(in crate::api::acp) session_policy: Option<serde_json::Value>,
    pub(in crate::api::acp) activity: Option<serde_json::Value>,
    pub(in crate::api::acp) request_id: Uuid,
    pub(in crate::api::acp) pair_agent_id: String,
    pub(in crate::api::acp) config: Arc<crate::config::Config>,
    pub(in crate::api::acp) role_runtime: RoleRuntime,
    pub(in crate::api::acp) turn_scope: RoleTurnScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::api::acp) enum ToolExecutionRoute {
    DenServer,
    AdapterLocal,
    Unsupported,
}

impl From<ToolExecutionRoute> for ControllerToolExecutionRoute {
    fn from(route: ToolExecutionRoute) -> Self {
        match route {
            ToolExecutionRoute::DenServer => Self::DenServer,
            ToolExecutionRoute::AdapterLocal => Self::AdapterLocal,
            ToolExecutionRoute::Unsupported => Self::Unsupported,
        }
    }
}

pub(in crate::api::acp) struct PersistedToolRequestEffect {
    pub(in crate::api::acp) tool_call_id: String,
    pub(in crate::api::acp) tool_name: String,
    pub(in crate::api::acp) route: ToolExecutionRoute,
    pub(in crate::api::acp) den_server_result_rx: Option<oneshot::Receiver<AcpToolResultRequest>>,
}

pub(in crate::api::acp) type AcpFrameResult = Result<
    (
        Vec<AcpGatewayEvent>,
        Option<PersistedToolRequestEffect>,
        Option<(String, String, AcpResolvedToolResult)>,
    ),
    std::io::Error,
>;

pub(in crate::api::acp) type AcpContinueToolPrepared = Result<
    (
        crate::core::runtime_provider::RuntimeStreamContinuation,
        crate::core::runtime_provider::RuntimeEventStream,
        std::sync::Arc<std::sync::Mutex<AcpStreamDiagnostics>>,
    ),
    CustomError,
>;

pub(in crate::api::acp) enum AcpResolvedToolResult {
    Receiver(oneshot::Receiver<AcpToolResultRequest>),
}

pub(in crate::api::acp) enum AcpPendingFuture {
    Frame(Pin<Box<dyn Future<Output = (AcpFrameResult, AcpStreamDiagnostics)> + Send>>),
    Tool(Pin<Box<dyn Future<Output = Option<Box<AcpToolResultRequest>>> + Send>>),
    ContinueTool(Pin<Box<dyn Future<Output = AcpContinueToolPrepared> + Send>>),
    Cleanup(Pin<Box<dyn Future<Output = serde_json::Value> + Send>>),
}

pub(in crate::api::acp) type AcpPromptInnerResult = Result<Result<Response, CustomError>, ApiError>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdapterContract {
    pub(super) name: String,
    pub(super) version: u32,
}

pub(super) fn _state_marker(_: &ApiState) {}
