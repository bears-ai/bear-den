//! Restricted wire model checked against agent-host-protocol spec/v1.0.0.
use super::super::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

pub(super) const ROOT: &str = "ahp-root://";
pub(super) const MAX_INPUT: usize = 32 * 1024;
pub(super) const MAX_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Channel {
    Root,
    Session(Uuid),
    Chat(Uuid),
}
impl Channel {
    pub fn parse(value: &str) -> Result<Self, Error> {
        if value == ROOT {
            return Ok(Self::Root);
        }
        let (kind, id) = value
            .split_once(":/")
            .ok_or_else(|| Error::invalid("Unsupported AHP channel URI."))?;
        let id = Uuid::parse_str(id)
            .map_err(|_| Error::invalid("AHP resource must name a canonical conversation UUID."))?;
        if id.is_nil() || value != format!("{kind}:/{id}") {
            return Err(Error::invalid(
                "AHP resource must use a canonical non-nil UUID.",
            ));
        }
        match kind {
            "ahp-session" => Ok(Self::Session(id)),
            "ahp-chat" => Ok(Self::Chat(id)),
            _ => Err(Error::invalid("Unsupported AHP channel.")),
        }
    }
    pub fn uri(self) -> String {
        match self {
            Self::Root => ROOT.into(),
            Self::Session(id) => format!("ahp-session:/{id}"),
            Self::Chat(id) => format!("ahp-chat:/{id}"),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Initialize {
    pub channel: String,
    pub protocol_versions: Vec<String>,
    pub client_id: String,
    #[serde(default)]
    pub initial_subscriptions: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Reconnect {
    pub channel: String,
    pub client_id: String,
    #[serde(rename = "lastSeenServerSeq")]
    pub _last_seen_server_seq: u64,
    pub subscriptions: Vec<String>,
}
#[derive(Deserialize)]
pub(super) struct Subscribe {
    pub channel: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Dispatch {
    pub channel: String,
    pub client_seq: u64,
    pub action: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserMessage {
    pub text: String,
    pub origin: UserOrigin,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserOrigin {
    pub kind: UserKind,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) enum UserKind {
    #[serde(rename = "user")]
    User,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(super) enum InputAction {
    #[serde(rename = "chat/turnStarted", rename_all = "camelCase")]
    Start {
        turn_id: Uuid,
        started_at: String,
        message: UserMessage,
    },
    #[serde(rename = "chat/turnCancelled", rename_all = "camelCase")]
    Cancel {
        turn_id: Uuid,
        #[serde(rename = "duration")]
        _duration: u64,
    },
}
impl InputAction {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Start {
                turn_id,
                started_at,
                message,
            } => {
                if turn_id.is_nil()
                    || started_at.len() > 64
                    || time::OffsetDateTime::parse(
                        started_at,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .is_err()
                {
                    return Err(Error::invalid(
                        "turnId must be a non-nil UUID and startedAt an RFC3339 timestamp.",
                    ));
                }
                if message.text.trim().is_empty()
                    || message.text.len() > MAX_INPUT
                    || message.text.contains('\0')
                {
                    return Err(Error::invalid(
                        "Text must contain 1–32768 bytes and no NUL.",
                    ));
                }
            }
            Self::Cancel { turn_id, .. } if turn_id.is_nil() => {
                return Err(Error::invalid("turnId must be a non-nil UUID."))
            }
            Self::Cancel { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(super) enum Action {
    #[serde(rename = "chat/turnStarted", rename_all = "camelCase")]
    Started {
        turn_id: Uuid,
        started_at: String,
        message: UserMessage,
    },
    #[serde(rename = "chat/responsePart", rename_all = "camelCase")]
    Part { turn_id: Uuid, part: Part },
    #[serde(rename = "chat/delta", rename_all = "camelCase")]
    Delta {
        turn_id: Uuid,
        part_id: String,
        content: String,
    },
    #[serde(rename = "chat/turnComplete", rename_all = "camelCase")]
    Complete { turn_id: Uuid, duration: u64 },
    #[serde(rename = "chat/turnCancelled", rename_all = "camelCase")]
    Cancelled { turn_id: Uuid, duration: u64 },
    #[serde(rename = "chat/error", rename_all = "camelCase")]
    Error {
        turn_id: Uuid,
        duration: u64,
        part: Part,
    },
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub(super) enum Part {
    #[serde(rename = "markdown")]
    Markdown { id: String, content: String },
    #[serde(rename = "error")]
    Error { error: ErrorInfo, resumable: bool },
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ErrorInfo {
    pub error_type: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Origin {
    pub client_id: String,
    pub client_seq: u64,
}

pub(super) fn rpc_ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
pub(super) fn rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub(super) fn notification(
    channel: Channel,
    action: Value,
    sequence: u64,
    origin: Option<&Origin>,
    rejection: Option<&str>,
) -> Value {
    let mut params = json!({"channel":channel.uri(),"action":action,"serverSeq":sequence});
    if let Some(origin) = origin {
        params["origin"] = json!(origin);
    }
    if let Some(rejection) = rejection {
        params["rejectionReason"] = json!(rejection);
    }
    json!({"jsonrpc":"2.0","method":"action","params":params})
}
pub(super) fn feature_metadata() -> Value {
    json!({"ai.bears.native":{"textOnly":true,"permissions":false,"subagents":false,"localTools":false,"sandbox":false,"attachments":false,"sendReplay":false,"liveMultiClient":false,"reconnect":"snapshot","conversationCreation":"/v1.0/chat/conversations"}})
}
