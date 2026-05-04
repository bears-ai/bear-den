use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::Deserialize;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Deserialize)]
pub struct AcpToolResultRequest {
    pub turn_id: Option<String>,
    pub request_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub approval_request_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub structured_content: serde_json::Value,
    #[serde(default)]
    pub diagnostic: serde_json::Value,
}

#[derive(Debug)]
struct AcpToolTurn {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    acp_session_id: String,
    request_id: Uuid,
    tool_call_id: String,
    tool_name: String,
    approval_request_id: Option<String>,
    settled: bool,
    result_tx: Option<oneshot::Sender<AcpToolResultRequest>>,
}

#[derive(Debug)]
pub struct AcpToolTurnRegistration {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub result_tx: oneshot::Sender<AcpToolResultRequest>,
}

#[derive(Debug)]
pub enum AcpToolResultDelivery {
    Delivered {
        body: AcpToolResultRequest,
        request_id: Uuid,
        bear_id: Uuid,
        tool_name: String,
    },
    TurnMissing {
        turn_id: Option<String>,
        tool_call_id: String,
    },
    AlreadySettled {
        turn_id: Option<String>,
        tool_call_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct AcpToolTurnCoordinator {
    turns: Arc<Mutex<HashMap<String, AcpToolTurn>>>,
}

impl Default for AcpToolTurnCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpToolTurnCoordinator {
    pub fn new() -> Self {
        Self {
            turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn key(session_id: &str, tool_call_id: &str) -> String {
        format!("{session_id}\n{tool_call_id}")
    }

    pub fn register(&self, registration: AcpToolTurnRegistration) -> Result<(), CustomError> {
        let key = Self::key(&registration.acp_session_id, &registration.tool_call_id);
        let mut turns = self
            .turns
            .lock()
            .map_err(|_| CustomError::System("ACP tool turn registry lock poisoned".to_string()))?;
        turns.insert(
            key,
            AcpToolTurn {
                user_id: registration.user_id,
                bear_id: registration.bear_id,
                bear_slug: registration.bear_slug,
                acp_session_id: registration.acp_session_id,
                request_id: registration.request_id,
                tool_call_id: registration.tool_call_id,
                tool_name: registration.tool_name,
                approval_request_id: registration.approval_request_id,
                settled: false,
                result_tx: Some(registration.result_tx),
            },
        );
        Ok(())
    }

    pub fn deliver_result(
        &self,
        user_id: i32,
        bear_slug: &str,
        session_id: &str,
        tool_call_id: &str,
        mut body: AcpToolResultRequest,
    ) -> Result<AcpToolResultDelivery, CustomError> {
        let key = Self::key(session_id, tool_call_id);
        let mut turns = self
            .turns
            .lock()
            .map_err(|_| CustomError::System("ACP tool turn registry lock poisoned".to_string()))?;
        let Some(turn) = turns.get_mut(&key) else {
            return Ok(AcpToolResultDelivery::TurnMissing {
                turn_id: body.turn_id,
                tool_call_id: tool_call_id.to_string(),
            });
        };
        if turn.user_id != user_id
            || turn.bear_slug != bear_slug
            || turn.acp_session_id != session_id
            || turn.tool_call_id != tool_call_id
        {
            return Err(CustomError::Authorization(
                "tool result does not match the authenticated ACP session".to_string(),
            ));
        }
        if let Some(body_tool_call_id) = body.tool_call_id.as_deref().filter(|s| !s.is_empty()) {
            if body_tool_call_id != turn.tool_call_id {
                return Err(CustomError::ValidationError(format!(
                    "tool result call id mismatch: expected {}, got {}",
                    turn.tool_call_id, body_tool_call_id
                )));
            }
        }
        if let Some(body_approval_request_id) = body
            .approval_request_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            if turn.approval_request_id.as_deref() != Some(body_approval_request_id) {
                return Err(CustomError::ValidationError(format!(
                    "tool result approval request id mismatch: expected {:?}, got {}",
                    turn.approval_request_id, body_approval_request_id
                )));
            }
        }
        if let Some(body_tool_name) = body.tool_name.as_deref().filter(|s| !s.is_empty()) {
            if body_tool_name != turn.tool_name {
                return Err(CustomError::ValidationError(format!(
                    "tool result name mismatch: expected {}, got {}",
                    turn.tool_name, body_tool_name
                )));
            }
        }
        if turn.settled {
            return Ok(AcpToolResultDelivery::AlreadySettled {
                turn_id: body.turn_id,
                tool_call_id: tool_call_id.to_string(),
            });
        }
        if body
            .tool_call_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_none()
        {
            body.tool_call_id = Some(turn.tool_call_id.clone());
        }
        if body
            .approval_request_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_none()
        {
            body.approval_request_id = turn.approval_request_id.clone();
        }
        turn.settled = true;
        let request_id = turn.request_id;
        let bear_id = turn.bear_id;
        let tool_name = turn.tool_name.clone();
        if let Some(result_tx) = turn.result_tx.take() {
            let _ = result_tx.send(body.clone());
        }
        Ok(AcpToolResultDelivery::Delivered {
            body,
            request_id,
            bear_id,
            tool_name,
        })
    }

    pub fn cleanup_session(&self, session_id: &str) {
        if let Ok(mut turns) = self.turns.lock() {
            let prefix = format!("{session_id}\n");
            turns.retain(|key, _| !key.starts_with(&prefix));
        }
    }

    pub fn remove(&self, session_id: &str, tool_call_id: &str) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.remove(&Self::key(session_id, tool_call_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_body(tool_call_id: Option<&str>) -> AcpToolResultRequest {
        AcpToolResultRequest {
            turn_id: Some("turn-1".to_string()),
            request_id: Some("request-1".to_string()),
            tool_call_id: tool_call_id.map(str::to_string),
            tool_name: Some("fs_read_text_file".to_string()),
            approval_request_id: Some("approval-1".to_string()),
            status: "ok".to_string(),
            content: Some("file contents".to_string()),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({}),
        }
    }

    #[test]
    fn fills_missing_result_ids_from_registered_turn() {
        let coordinator = AcpToolTurnCoordinator::new();
        let (tx, mut rx) = oneshot::channel();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: "session-1".to_string(),
                request_id: Uuid::new_v4(),
                tool_call_id: "call-1".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-1".to_string()),
                result_tx: tx,
            })
            .unwrap();
        let delivery = coordinator
            .deliver_result(7, "meta", "session-1", "call-1", result_body(None))
            .unwrap();
        assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));
        let delivered = rx.try_recv().unwrap();
        assert_eq!(delivered.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(delivered.approval_request_id.as_deref(), Some("approval-1"));
    }

    #[test]
    fn duplicate_result_reports_already_settled() {
        let coordinator = AcpToolTurnCoordinator::new();
        let (tx, _rx) = oneshot::channel();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: "session-1".to_string(),
                request_id: Uuid::new_v4(),
                tool_call_id: "call-1".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-1".to_string()),
                result_tx: tx,
            })
            .unwrap();
        assert!(matches!(
            coordinator
                .deliver_result(
                    7,
                    "meta",
                    "session-1",
                    "call-1",
                    result_body(Some("call-1"))
                )
                .unwrap(),
            AcpToolResultDelivery::Delivered { .. }
        ));
        assert!(matches!(
            coordinator
                .deliver_result(
                    7,
                    "meta",
                    "session-1",
                    "call-1",
                    result_body(Some("call-1"))
                )
                .unwrap(),
            AcpToolResultDelivery::AlreadySettled { .. }
        ));
    }
}
