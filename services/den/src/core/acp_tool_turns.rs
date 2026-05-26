use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Deserialize;
use time::OffsetDateTime;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::errors::CustomError;

const ACTIVE_TURN_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, Deserialize)]
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
    #[serde(default)]
    pub adapter_contract: Option<serde_json::Value>,
}

const SETTLED_RESULT_TTL: Duration = Duration::from_secs(5 * 60);
const SETTLED_RESULT_MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
pub struct AcpPendingToolTurn {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub status: String,
    pub registered_at: Instant,
    pub deadline_at: Instant,
}

impl AcpPendingToolTurn {
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "bear_id": self.bear_id,
            "session_id": self.acp_session_id,
            "tool_call_id": self.tool_call_id,
            "tool_name": self.tool_name,
            "approval_request_id": self.approval_request_id,
            "status": self.status,
            "age_ms": self.registered_at.elapsed().as_millis(),
            "time_to_deadline_ms": self.deadline_at.saturating_duration_since(Instant::now()).as_millis(),
            "component": "den.acp",
            "phase": "pending_tool_turn",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcpToolTurnCleanupSummary {
    pub pending_removed: usize,
    pub settled_removed: usize,
}

impl AcpToolTurnCleanupSummary {
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "pending_removed": self.pending_removed,
            "settled_removed": self.settled_removed,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AcpSettledToolResult {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub status: String,
    pub content_bytes: usize,
    pub structured_content_bytes: usize,
    pub settled_at: Instant,
}

impl AcpSettledToolResult {
    fn from_turn(turn: &AcpToolTurn, body: &AcpToolResultRequest) -> Self {
        Self {
            user_id: turn.user_id,
            bear_id: turn.bear_id,
            bear_slug: turn.bear_slug.clone(),
            acp_session_id: turn.acp_session_id.clone(),
            request_id: turn.request_id,
            tool_call_id: turn.tool_call_id.clone(),
            tool_name: turn.tool_name.clone(),
            approval_request_id: turn.approval_request_id.clone(),
            status: body.status.clone(),
            content_bytes: body.content.as_deref().map(str::len).unwrap_or(0),
            structured_content_bytes: body.structured_content.to_string().len(),
            settled_at: Instant::now(),
        }
    }

    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "bear_id": self.bear_id,
            "session_id": self.acp_session_id,
            "tool_call_id": self.tool_call_id,
            "tool_name": self.tool_name,
            "approval_request_id": self.approval_request_id,
            "status": self.status,
            "content_bytes": self.content_bytes,
            "structured_content_bytes": self.structured_content_bytes,
            "age_ms": self.settled_at.elapsed().as_millis(),
            "component": "den.acp",
            "phase": crate::core::acp_tools::acp_diag_phase::RECENTLY_SETTLED_RESULT,
        })
    }
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
    registered_at: Instant,
    deadline_at: Instant,
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
    pub timeout_ms: u64,
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
    RecentlySettled {
        turn_id: Option<String>,
        tool_call_id: String,
        cached: AcpSettledToolResult,
    },
}

#[derive(Debug, Clone)]
pub struct AcpActiveTurn {
    pub acp_session_id: String,
    pub request_id: Uuid,
    pub conversation_id: Option<String>,
    pub started_at: Instant,
    pub deadline_at: Instant,
}

impl AcpActiveTurn {
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": self.acp_session_id,
            "request_id": self.request_id,
            "conversation_id": self.conversation_id,
            "age_ms": self.started_at.elapsed().as_millis(),
            "time_to_deadline_ms": self.deadline_at.saturating_duration_since(Instant::now()).as_millis(),
            "component": "den.acp",
            "phase": "active_turn",
        })
    }
}

#[derive(Debug, Clone)]
pub struct AcpToolTurnCoordinator {
    turns: Arc<Mutex<HashMap<String, AcpToolTurn>>>,
    settled_results: Arc<Mutex<HashMap<String, AcpSettledToolResult>>>,
    active_turns: Arc<Mutex<HashMap<String, AcpActiveTurn>>>,
}

#[derive(Debug)]
pub struct AcpActiveTurnGuard {
    coordinator: AcpToolTurnCoordinator,
    session_id: String,
    request_id: Uuid,
    released: bool,
}

impl AcpActiveTurnGuard {
    pub fn release(mut self) {
        if !self.released {
            self.coordinator
                .release_active_turn(&self.session_id, self.request_id);
            self.released = true;
        }
    }
}

impl Drop for AcpActiveTurnGuard {
    fn drop(&mut self) {
        if !self.released {
            self.coordinator
                .release_active_turn(&self.session_id, self.request_id);
            self.released = true;
        }
    }
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
            settled_results: Arc::new(Mutex::new(HashMap::new())),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn acquire_active_turn(
        &self,
        session_id: &str,
        request_id: Uuid,
        conversation_id: Option<String>,
    ) -> Result<AcpActiveTurnGuard, CustomError> {
        let mut active_turns = self.active_turns.lock().map_err(|_| {
            CustomError::System("ACP active turn registry lock poisoned".to_string())
        })?;
        let now = Instant::now();
        active_turns.retain(|_, turn| turn.deadline_at > now);
        if let Some(existing) = active_turns.get(session_id) {
            return Err(CustomError::ValidationError(format!(
                "ACP turn already active for this session: {}",
                existing.diagnostic()
            )));
        }
        let turn = AcpActiveTurn {
            acp_session_id: session_id.to_string(),
            request_id,
            conversation_id,
            started_at: now,
            deadline_at: now + ACTIVE_TURN_TTL,
        };
        active_turns.insert(session_id.to_string(), turn.clone());
        Ok(AcpActiveTurnGuard {
            coordinator: self.clone(),
            session_id: session_id.to_string(),
            request_id,
            released: false,
        })
    }

    pub fn cancel_active_turn(&self, session_id: &str) -> Option<AcpActiveTurn> {
        self.active_turns.lock().ok()?.remove(session_id)
    }

    pub fn release_active_turn(&self, session_id: &str, request_id: Uuid) {
        if let Ok(mut active_turns) = self.active_turns.lock() {
            if active_turns
                .get(session_id)
                .is_some_and(|turn| turn.request_id == request_id)
            {
                active_turns.remove(session_id);
            }
        }
    }

    pub fn active_turn_for_session(&self, session_id: &str) -> Option<AcpActiveTurn> {
        let mut active_turns = self.active_turns.lock().ok()?;
        let now = Instant::now();
        active_turns.retain(|_, turn| turn.deadline_at > now);
        active_turns.get(session_id).cloned()
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
        let now = Instant::now();
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
                registered_at: now,
                deadline_at: now + Duration::from_millis(registration.timeout_ms.max(1)),
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
            drop(turns);
            if let Some(cached) = self.recently_settled(session_id, tool_call_id) {
                if cached.user_id != user_id
                    || cached.bear_slug != bear_slug
                    || cached.acp_session_id != session_id
                    || cached.tool_call_id != tool_call_id
                {
                    return Err(CustomError::Authorization(
                        "tool result does not match the authenticated ACP session".to_string(),
                    ));
                }
                return Ok(AcpToolResultDelivery::RecentlySettled {
                    turn_id: body.turn_id,
                    tool_call_id: tool_call_id.to_string(),
                    cached,
                });
            }
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
        let cached = AcpSettledToolResult::from_turn(turn, &body);
        if let Some(result_tx) = turn.result_tx.take() {
            let _ = result_tx.send(body.clone());
        }
        drop(turns);
        self.cache_settled_result(cached)?;
        Ok(AcpToolResultDelivery::Delivered {
            body,
            request_id,
            bear_id,
            tool_name,
        })
    }

    pub fn pending_for_session(&self, session_id: &str) -> Vec<AcpPendingToolTurn> {
        let Ok(turns) = self.turns.lock() else {
            return Vec::new();
        };
        let prefix = format!("{session_id}\n");
        turns
            .iter()
            .filter(|(key, turn)| key.starts_with(&prefix) && !turn.settled)
            .map(|(_, turn)| AcpPendingToolTurn {
                user_id: turn.user_id,
                bear_id: turn.bear_id,
                bear_slug: turn.bear_slug.clone(),
                acp_session_id: turn.acp_session_id.clone(),
                request_id: turn.request_id,
                tool_call_id: turn.tool_call_id.clone(),
                tool_name: turn.tool_name.clone(),
                approval_request_id: turn.approval_request_id.clone(),
                status: "pending".to_string(),
                registered_at: turn.registered_at,
                deadline_at: turn.deadline_at,
            })
            .collect()
    }

    pub fn expired_pending_for_session(&self, session_id: &str) -> Vec<AcpPendingToolTurn> {
        let now = Instant::now();
        self.pending_for_session(session_id)
            .into_iter()
            .filter(|turn| turn.deadline_at <= now)
            .collect()
    }

    pub fn auto_timeout_result(
        &self,
        session_id: &str,
        tool_call_id: &str,
        reason: impl Into<String>,
    ) -> Option<AcpToolResultRequest> {
        let mut turns = self.turns.lock().ok()?;
        let turn = turns.get_mut(&Self::key(session_id, tool_call_id))?;
        if turn.settled {
            return None;
        }
        turn.settled = true;
        let reason = reason.into();
        let body = AcpToolResultRequest {
            turn_id: None,
            request_id: Some(turn.request_id.to_string()),
            tool_call_id: Some(turn.tool_call_id.clone()),
            tool_name: Some(turn.tool_name.clone()),
            approval_request_id: turn.approval_request_id.clone(),
            status: "timeout".to_string(),
            content: Some(reason),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "auto_timeout_denial",
                "tool_call_id": turn.tool_call_id,
                "tool_name": turn.tool_name,
                "approval_request_id": turn.approval_request_id,
            }),
            ..Default::default()
        };
        let cached = AcpSettledToolResult::from_turn(turn, &body);
        if let Some(result_tx) = turn.result_tx.take() {
            let _ = result_tx.send(body.clone());
        }
        drop(turns);
        let _ = self.cache_settled_result(cached);
        Some(body)
    }

    pub fn diagnostic_snapshot(&self, session_id: &str) -> serde_json::Value {
        serde_json::json!({
            "session_id": session_id,
            "pending": self
                .pending_for_session(session_id)
                .into_iter()
                .map(|turn| turn.diagnostic())
                .collect::<Vec<_>>(),
            "expired": self
                .expired_pending_for_session(session_id)
                .into_iter()
                .map(|turn| turn.diagnostic())
                .collect::<Vec<_>>(),
            "observed_at": OffsetDateTime::now_utc(),
        })
    }

    pub fn cleanup_expired_tool_turns_for_session(
        &self,
        session_id: &str,
    ) -> AcpToolTurnCleanupSummary {
        let prefix = format!("{session_id}\n");
        let now = Instant::now();
        let mut summary = AcpToolTurnCleanupSummary::default();
        if let Ok(mut turns) = self.turns.lock() {
            turns.retain(|key, turn| {
                let remove = key.starts_with(&prefix) && !turn.settled && turn.deadline_at <= now;
                if remove {
                    summary.pending_removed += 1;
                }
                !remove
            });
        }
        summary
    }

    pub fn cleanup_request_tool_turns(
        &self,
        session_id: &str,
        request_id: Uuid,
    ) -> AcpToolTurnCleanupSummary {
        let prefix = format!("{session_id}\n");
        let mut summary = AcpToolTurnCleanupSummary::default();
        if let Ok(mut turns) = self.turns.lock() {
            turns.retain(|key, turn| {
                let remove = key.starts_with(&prefix) && turn.request_id == request_id;
                if remove {
                    summary.pending_removed += 1;
                }
                !remove
            });
        }
        if let Ok(mut settled) = self.settled_results.lock() {
            settled.retain(|key, result| {
                let remove = key.starts_with(&prefix) && result.request_id == request_id;
                if remove {
                    summary.settled_removed += 1;
                }
                !remove
            });
        }
        summary
    }

    pub fn cleanup_session(&self, session_id: &str) {
        if let Ok(mut turns) = self.turns.lock() {
            let prefix = format!("{session_id}\n");
            turns.retain(|key, _| !key.starts_with(&prefix));
        }
        if let Ok(mut settled) = self.settled_results.lock() {
            let prefix = format!("{session_id}\n");
            settled.retain(|key, _| !key.starts_with(&prefix));
        }
        if let Ok(mut active_turns) = self.active_turns.lock() {
            active_turns.remove(session_id);
        }
    }

    pub fn remove(&self, session_id: &str, tool_call_id: &str) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.remove(&Self::key(session_id, tool_call_id));
        }
    }

    fn cache_settled_result(&self, result: AcpSettledToolResult) -> Result<(), CustomError> {
        let mut settled = self.settled_results.lock().map_err(|_| {
            CustomError::System("ACP settled tool result cache lock poisoned".to_string())
        })?;
        prune_settled_results(&mut settled);
        settled.insert(
            Self::key(&result.acp_session_id, &result.tool_call_id),
            result,
        );
        prune_settled_results(&mut settled);
        Ok(())
    }

    pub fn recently_settled(
        &self,
        session_id: &str,
        tool_call_id: &str,
    ) -> Option<AcpSettledToolResult> {
        let mut settled = self.settled_results.lock().ok()?;
        prune_settled_results(&mut settled);
        settled.get(&Self::key(session_id, tool_call_id)).cloned()
    }
}

fn prune_settled_results(settled: &mut HashMap<String, AcpSettledToolResult>) {
    settled.retain(|_, result| result.settled_at.elapsed() <= SETTLED_RESULT_TTL);
    if settled.len() <= SETTLED_RESULT_MAX_ENTRIES {
        return;
    }
    let mut by_age = settled
        .iter()
        .map(|(key, result)| (key.clone(), result.settled_at))
        .collect::<Vec<_>>();
    by_age.sort_by_key(|(_, settled_at)| *settled_at);
    let remove_count = settled.len().saturating_sub(SETTLED_RESULT_MAX_ENTRIES);
    for (key, _) in by_age.into_iter().take(remove_count) {
        settled.remove(&key);
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
            ..Default::default()
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
                timeout_ms: 30_000,
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
    fn duplicate_after_removal_reports_recently_settled() {
        let coordinator = AcpToolTurnCoordinator::new();
        let (tx, _rx) = oneshot::channel();
        let request_id = Uuid::new_v4();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: "session-1".to_string(),
                request_id,
                tool_call_id: "call-1".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-1".to_string()),
                timeout_ms: 30_000,
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
        coordinator.remove("session-1", "call-1");
        match coordinator
            .deliver_result(
                7,
                "meta",
                "session-1",
                "call-1",
                result_body(Some("call-1")),
            )
            .unwrap()
        {
            AcpToolResultDelivery::RecentlySettled { cached, .. } => {
                assert_eq!(cached.request_id, request_id);
                assert_eq!(cached.tool_name, "fs_read_text_file");
                assert_eq!(cached.status, "ok");
                assert_eq!(cached.content_bytes, "file contents".len());
            }
            other => panic!("unexpected delivery: {other:?}"),
        }
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
                timeout_ms: 30_000,
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

    #[test]
    fn request_scoped_cleanup_preserves_other_request_and_active_turn() {
        let coordinator = AcpToolTurnCoordinator::new();
        let session_id = "session-1";
        let stale_request_id = Uuid::new_v4();
        let active_request_id = Uuid::new_v4();
        let _guard = coordinator
            .acquire_active_turn(session_id, active_request_id, Some("conv-1".to_string()))
            .unwrap();
        let (stale_tx, _stale_rx) = oneshot::channel();
        let (active_tx, _active_rx) = oneshot::channel();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: session_id.to_string(),
                request_id: stale_request_id,
                tool_call_id: "call-stale".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-stale".to_string()),
                timeout_ms: 30_000,
                result_tx: stale_tx,
            })
            .unwrap();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: session_id.to_string(),
                request_id: active_request_id,
                tool_call_id: "call-active".to_string(),
                tool_name: "fs_edit_file".to_string(),
                approval_request_id: Some("approval-active".to_string()),
                timeout_ms: 30_000,
                result_tx: active_tx,
            })
            .unwrap();

        let summary = coordinator.cleanup_request_tool_turns(session_id, stale_request_id);

        assert_eq!(summary.pending_removed, 1);
        assert_eq!(summary.settled_removed, 0);
        let pending = coordinator.pending_for_session(session_id);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request_id, active_request_id);
        assert_eq!(pending[0].tool_call_id, "call-active");
        assert_eq!(
            coordinator
                .active_turn_for_session(session_id)
                .map(|turn| turn.request_id),
            Some(active_request_id)
        );
        assert!(matches!(
            coordinator
                .deliver_result(
                    7,
                    "meta",
                    session_id,
                    "call-active",
                    AcpToolResultRequest {
                        tool_call_id: Some("call-active".to_string()),
                        tool_name: Some("fs_edit_file".to_string()),
                        approval_request_id: Some("approval-active".to_string()),
                        status: "ok".to_string(),
                        content: Some("edited".to_string()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({}),
                        ..Default::default()
                    }
                )
                .unwrap(),
            AcpToolResultDelivery::Delivered { .. }
        ));
    }

    #[test]
    fn expired_cleanup_preserves_nonexpired_request_and_active_turn() {
        let coordinator = AcpToolTurnCoordinator::new();
        let session_id = "session-1";
        let expired_request_id = Uuid::new_v4();
        let active_request_id = Uuid::new_v4();
        let _guard = coordinator
            .acquire_active_turn(session_id, active_request_id, Some("conv-1".to_string()))
            .unwrap();
        let (expired_tx, _expired_rx) = oneshot::channel();
        let (active_tx, _active_rx) = oneshot::channel();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: session_id.to_string(),
                request_id: expired_request_id,
                tool_call_id: "call-expired".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-expired".to_string()),
                timeout_ms: 1,
                result_tx: expired_tx,
            })
            .unwrap();
        coordinator
            .register(AcpToolTurnRegistration {
                user_id: 7,
                bear_id: Uuid::new_v4(),
                bear_slug: "meta".to_string(),
                acp_session_id: session_id.to_string(),
                request_id: active_request_id,
                tool_call_id: "call-active".to_string(),
                tool_name: "fs_edit_file".to_string(),
                approval_request_id: Some("approval-active".to_string()),
                timeout_ms: 30_000,
                result_tx: active_tx,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));

        let summary = coordinator.cleanup_expired_tool_turns_for_session(session_id);

        assert_eq!(summary.pending_removed, 1);
        assert_eq!(summary.settled_removed, 0);
        let pending = coordinator.pending_for_session(session_id);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request_id, active_request_id);
        assert_eq!(pending[0].tool_call_id, "call-active");
        assert_eq!(
            coordinator
                .active_turn_for_session(session_id)
                .map(|turn| turn.request_id),
            Some(active_request_id)
        );
    }
}
