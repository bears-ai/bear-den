use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BearWireEventScope {
    Persistent,
    #[default]
    Ephemeral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearWireEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    pub scope: BearWireEventScope,
    pub source: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bear_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ResourceRef>,
    pub data: Value,
}

impl BearWireEvent {
    pub fn ephemeral(event_type: impl Into<String>, data: Value) -> Self {
        Self {
            event_id: None,
            sequence: None,
            scope: BearWireEventScope::Ephemeral,
            source: "den.runtime".to_string(),
            event_type: event_type.into(),
            subject: None,
            time: None,
            bear_id: None,
            role: None,
            role_agent_id: None,
            human_id: None,
            session_id: None,
            run_id: None,
            resource_refs: Vec::new(),
            data,
        }
    }

    pub fn ephemeral_typed<T: Serialize>(event_type: impl Into<String>, data: T) -> Self {
        Self::ephemeral(
            event_type,
            serde_json::to_value(data).expect("BearWire typed event data serializes"),
        )
    }

    pub fn persistent_typed<T: Serialize>(event_type: impl Into<String>, data: T) -> Self {
        let mut event = Self::ephemeral_typed(event_type, data);
        event.scope = BearWireEventScope::Persistent;
        event
    }

    pub fn tool_call_requested(data: ToolCallRequestedWire) -> Self {
        Self::ephemeral_typed("tool_call.requested", data)
    }

    pub fn tool_call_waiting(data: ToolCallWaitingWire) -> Self {
        Self::ephemeral_typed("client.waiting", data)
    }

    pub fn tool_call_finished(data: ToolCallFinishWire) -> Self {
        let event_type = match data.status {
            ToolCallFinishStatusWire::Ok => "tool_call.completed",
            ToolCallFinishStatusWire::Incomplete => "tool_call.warning",
            ToolCallFinishStatusWire::Cancelled => "tool_call.cancelled",
            ToolCallFinishStatusWire::Error => "tool_call.failed",
        };
        Self::ephemeral_typed(event_type, data)
    }

    pub fn with_run_id(mut self, run_id: Option<String>) -> Self {
        self.run_id.clone_from(&run_id);
        if let Some(run_id) = run_id {
            self.subject = Some(format!("resource/run/{run_id}"));
            self.resource_refs.push(ResourceRef::new("run", run_id));
        }
        self
    }

    pub fn with_tool_call(mut self, tool_call_id: String) -> Self {
        self.subject = Some(format!("resource/tool_call/{tool_call_id}"));
        self.resource_refs
            .push(ResourceRef::new("tool_call", tool_call_id));
        self
    }

    pub fn with_permission_request(mut self, permission_request_id: Option<String>) -> Self {
        if let Some(permission_request_id) = permission_request_id {
            self.resource_refs.push(ResourceRef::new(
                "permission_request",
                permission_request_id,
            ));
        }
        self
    }

    pub fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id.clone());
        self.subject = Some(format!("resource/session/{session_id}"));
        self.resource_refs
            .push(ResourceRef::new("session", session_id));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ResourceRef {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            uri: None,
            display_name: None,
            version: None,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcNotification<T> {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallWire {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub kind: String,
    pub arguments: Value,
    pub display: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRefWire {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallFinishStatusWire {
    Ok,
    Error,
    Incomplete,
    Cancelled,
}

impl ToolCallFinishStatusWire {
    pub fn from_wire_str(status: &str) -> Self {
        match status {
            "ok" => Self::Ok,
            "incomplete" => Self::Incomplete,
            "cancelled" => Self::Cancelled,
            _ => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallFinishWire {
    pub tool_call: ToolCallRefWire,
    pub status: ToolCallFinishStatusWire,
    pub summary: Option<String>,
    pub error_message: Option<String>,
    pub content: Option<String>,
    pub structured_content: Option<Value>,
    pub error: Option<Value>,
    pub compacted: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPermissionWire {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionTargetWire {
    Den,
    ArmatureLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequestedWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_responder_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obligation_id: Option<String>,
    pub tool_call: ToolCallWire,
    pub approval_required: bool,
    pub execution_target: ExecutionTargetWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallWaitingWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_responder_action: Option<String>,
    pub expected_client_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obligation_id: Option<String>,
    pub tool_call: ToolCallWire,
    pub permission: ToolPermissionWire,
    pub approval_required: bool,
    pub execution_target: ExecutionTargetWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_step_id: Option<String>,
}

pub fn bearwire_event_to_json_rpc_notification(
    event: BearWireEvent,
) -> JsonRpcNotification<BearWireEvent> {
    JsonRpcNotification {
        jsonrpc: "2.0",
        method: "event",
        params: event,
    }
}
