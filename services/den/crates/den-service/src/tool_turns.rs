use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Deserialize;
use time::OffsetDateTime;
use tokio::sync::oneshot;
use uuid::Uuid;

use den_core::DenError;
use den_protocol::{RuntimeApprovalDecision, RuntimeContinuation, RuntimeToolResultStatus};

mod source;
pub use source::ConversationTurnLease;

const ACTIVE_TURN_TTL: Duration = Duration::from_mins(10);

fn poisoned_lock(what: &str) -> DenError {
    DenError::System(format!("{what} lock poisoned"))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolResultRequest {
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

const SETTLED_RESULT_TTL: Duration = Duration::from_mins(5);
const SETTLED_RESULT_MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
pub struct PendingToolTurn {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub client_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub status: String,
    pub registered_at: Instant,
    pub deadline_at: Instant,
}

impl From<&ToolTurn> for PendingToolTurn {
    fn from(turn: &ToolTurn) -> Self {
        Self {
            user_id: turn.user_id,
            bear_id: turn.bear_id,
            bear_slug: turn.bear_slug.clone(),
            client_session_id: turn.client_session_id.clone(),
            request_id: turn.request_id,
            tool_call_id: turn.tool_call_id.clone(),
            tool_name: turn.tool_name.clone(),
            approval_request_id: turn.approval_request_id.clone(),
            status: "pending".to_string(),
            registered_at: turn.registered_at,
            deadline_at: turn.deadline_at,
        }
    }
}

impl PendingToolTurn {
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "bear_id": self.bear_id,
            "session_id": self.client_session_id,
            "tool_call_id": self.tool_call_id,
            "tool_name": self.tool_name,
            "approval_request_id": self.approval_request_id,
            "status": self.status,
            "age_ms": self.registered_at.elapsed().as_millis(),
            "time_to_deadline_ms": self.deadline_at.saturating_duration_since(Instant::now()).as_millis(),
            "component": "den.armature",
            "phase": "pending_tool_turn",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolTurnCleanupSummary {
    pub pending_removed: usize,
    pub settled_removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareRuntimeContinuationError {
    MissingToolCallId { display_tool_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRuntimeContinuation {
    pub tool_call_id: String,
    pub display_tool_name: String,
    pub continuation: RuntimeContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSettlementSummary {
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub display_tool_name: String,
    pub status: String,
    pub removed_pending_turn: bool,
    pub completed_ok: bool,
    pub timed_out: bool,
}

impl ToolTurnCleanupSummary {
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "pending_removed": self.pending_removed,
            "settled_removed": self.settled_removed,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SettledToolResult {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub client_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub status: String,
    pub content_bytes: usize,
    pub structured_content_bytes: usize,
    pub settled_at: Instant,
}

impl From<(&ToolTurn, &ToolResultRequest)> for SettledToolResult {
    fn from((turn, body): (&ToolTurn, &ToolResultRequest)) -> Self {
        Self {
            user_id: turn.user_id,
            bear_id: turn.bear_id,
            bear_slug: turn.bear_slug.clone(),
            client_session_id: turn.client_session_id.clone(),
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
}

impl SettledToolResult {
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "bear_id": self.bear_id,
            "session_id": self.client_session_id,
            "tool_call_id": self.tool_call_id,
            "tool_name": self.tool_name,
            "approval_request_id": self.approval_request_id,
            "status": self.status,
            "content_bytes": self.content_bytes,
            "structured_content_bytes": self.structured_content_bytes,
            "age_ms": self.settled_at.elapsed().as_millis(),
            "component": "den.armature",
            "phase": "recently_settled_result",
        })
    }
}

#[derive(Debug)]
struct ToolTurn {
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    client_session_id: String,
    request_id: Uuid,
    tool_call_id: String,
    tool_name: String,
    approval_request_id: Option<String>,
    settled: bool,
    registered_at: Instant,
    deadline_at: Instant,
    result_tx: Option<oneshot::Sender<ToolResultRequest>>,
}

#[derive(Debug)]
pub struct ToolTurnRegistration {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub client_session_id: String,
    pub request_id: Uuid,
    pub tool_call_id: String,
    pub tool_name: String,
    pub approval_request_id: Option<String>,
    pub timeout_ms: u64,
    pub result_tx: oneshot::Sender<ToolResultRequest>,
}

#[derive(Debug)]
pub enum ToolResultDelivery {
    Delivered {
        body: ToolResultRequest,
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
        cached: SettledToolResult,
    },
}

#[derive(Debug, Clone)]
pub struct ActiveTurn {
    pub client_session_id: String,
    pub request_id: Uuid,
    pub conversation_id: Option<String>,
    pub started_at: Instant,
    pub deadline_at: Instant,
}

impl ActiveTurn {
    pub fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": self.client_session_id,
            "request_id": self.request_id,
            "conversation_id": self.conversation_id,
            "age_ms": self.started_at.elapsed().as_millis(),
            "time_to_deadline_ms": self.deadline_at.saturating_duration_since(Instant::now()).as_millis(),
            "component": "den.armature",
            "phase": "active_turn",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToolTurnCoordinator {
    turns: Arc<Mutex<HashMap<String, ToolTurn>>>,
    settled_results: Arc<Mutex<HashMap<String, SettledToolResult>>>,
    active_turns: Arc<Mutex<HashMap<String, ActiveTurn>>>,
    orphaned_result_txs: Arc<Mutex<HashMap<String, oneshot::Sender<ToolResultRequest>>>>,
}

#[derive(Debug)]
pub struct ActiveTurnGuard {
    coordinator: ToolTurnCoordinator,
    session_id: String,
    request_id: Uuid,
    released: bool,
}

impl ActiveTurnGuard {
    pub fn release(mut self) {
        if !self.released {
            self.coordinator
                .release_active_turn(&self.session_id, self.request_id);
            self.released = true;
        }
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if !self.released {
            self.coordinator
                .release_active_turn(&self.session_id, self.request_id);
            self.released = true;
        }
    }
}

impl Default for ToolTurnCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolTurnCoordinator {
    pub fn new() -> Self {
        Self {
            turns: Arc::new(Mutex::new(HashMap::new())),
            settled_results: Arc::new(Mutex::new(HashMap::new())),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            orphaned_result_txs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn acquire_active_turn(
        &self,
        session_id: &str,
        request_id: Uuid,
        conversation_id: Option<String>,
    ) -> Result<ActiveTurnGuard, DenError> {
        let mut active_turns = self
            .active_turns
            .lock()
            .map_err(|_| poisoned_lock("client active turn registry"))?;
        let now = Instant::now();
        active_turns.retain(|_, turn| turn.deadline_at > now);
        if let Some(existing) = active_turns.get(session_id) {
            return Err(DenError::ValidationError(format!(
                "client turn already active for this session: {}",
                existing.diagnostic()
            )));
        }
        let turn = ActiveTurn {
            client_session_id: session_id.to_string(),
            request_id,
            conversation_id,
            started_at: now,
            deadline_at: now + ACTIVE_TURN_TTL,
        };
        active_turns.insert(session_id.to_string(), turn);
        Ok(ActiveTurnGuard {
            coordinator: self.clone(),
            session_id: session_id.to_string(),
            request_id,
            released: false,
        })
    }

    pub fn cancel_active_turn(&self, session_id: &str) -> Option<ActiveTurn> {
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

    pub fn active_turn_for_session(&self, session_id: &str) -> Option<ActiveTurn> {
        let mut active_turns = self.active_turns.lock().ok()?;
        let now = Instant::now();
        active_turns.retain(|_, turn| turn.deadline_at > now);
        active_turns.get(session_id).cloned()
    }

    fn key(session_id: &str, tool_call_id: &str) -> String {
        format!("{session_id}\n{tool_call_id}")
    }

    pub fn register(&self, registration: ToolTurnRegistration) -> Result<(), DenError> {
        let key = Self::key(&registration.client_session_id, &registration.tool_call_id);
        let mut turns = self
            .turns
            .lock()
            .map_err(|_| poisoned_lock("armature tool turn registry"))?;
        let now = Instant::now();
        let client_session_id = registration.client_session_id.clone();
        let tool_call_id = registration.tool_call_id.clone();
        let tool_name = registration.tool_name.clone();
        turns.insert(
            key,
            ToolTurn {
                user_id: registration.user_id,
                bear_id: registration.bear_id,
                bear_slug: registration.bear_slug,
                client_session_id: registration.client_session_id,
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
        tracing::info!(
            client_session_id = %client_session_id,
            tool_call_id = %tool_call_id,
            tool_name = %tool_name,
            active_turn_count = turns.len(),
            "armature registered pending tool turn"
        );
        Ok(())
    }

    pub fn deliver_result(
        &self,
        user_id: i32,
        bear_slug: &str,
        session_id: &str,
        tool_call_id: &str,
        mut body: ToolResultRequest,
    ) -> Result<ToolResultDelivery, DenError> {
        let key = Self::key(session_id, tool_call_id);
        let mut turns = self
            .turns
            .lock()
            .map_err(|_| poisoned_lock("armature tool turn registry"))?;
        let Some(turn) = turns.get_mut(&key) else {
            tracing::warn!(
                client_session_id = %session_id,
                tool_call_id = %tool_call_id,
                active_turn_count = turns.len(),
                active_tool_keys = ?turns.keys().cloned().collect::<Vec<_>>(),
                "armature tool result delivery found no pending turn"
            );
            drop(turns);
            if let Some(cached) = self.recently_settled(session_id, tool_call_id) {
                if cached.user_id != user_id
                    || cached.bear_slug != bear_slug
                    || cached.client_session_id != session_id
                    || cached.tool_call_id != tool_call_id
                {
                    return Err(DenError::Authorization(
                        "tool result does not match the authenticated client session".to_string(),
                    ));
                }
                return Ok(ToolResultDelivery::RecentlySettled {
                    turn_id: body.turn_id,
                    tool_call_id: tool_call_id.to_string(),
                    cached,
                });
            }
            return Ok(ToolResultDelivery::TurnMissing {
                turn_id: body.turn_id,
                tool_call_id: tool_call_id.to_string(),
            });
        };
        if turn.user_id != user_id
            || turn.bear_slug != bear_slug
            || turn.client_session_id != session_id
            || turn.tool_call_id != tool_call_id
        {
            return Err(DenError::Authorization(
                "tool result does not match the authenticated client session".to_string(),
            ));
        }
        if let Some(body_tool_call_id) = body.tool_call_id.as_deref().filter(|s| !s.is_empty()) {
            if body_tool_call_id != turn.tool_call_id {
                return Err(DenError::ValidationError(format!(
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
                return Err(DenError::ValidationError(format!(
                    "tool result approval request id mismatch: expected {:?}, got {}",
                    turn.approval_request_id, body_approval_request_id
                )));
            }
        }
        if let Some(body_tool_name) = body.tool_name.as_deref().filter(|s| !s.is_empty()) {
            if body_tool_name != turn.tool_name {
                return Err(DenError::ValidationError(format!(
                    "tool result name mismatch: expected {}, got {}",
                    turn.tool_name, body_tool_name
                )));
            }
        }
        if turn.settled {
            return Ok(ToolResultDelivery::AlreadySettled {
                turn_id: body.turn_id,
                tool_call_id: tool_call_id.to_string(),
            });
        }
        if body.tool_call_id.as_deref().is_none_or(str::is_empty) {
            body.tool_call_id = Some(turn.tool_call_id.clone());
        }
        if body
            .approval_request_id
            .as_deref()
            .is_none_or(str::is_empty)
        {
            body.approval_request_id
                .clone_from(&turn.approval_request_id);
        }
        turn.settled = true;
        let request_id = turn.request_id;
        let bear_id = turn.bear_id;
        let tool_name = turn.tool_name.clone();
        let cached = SettledToolResult::from((&*turn, &body));
        if let Some(result_tx) = turn.result_tx.take() {
            let _ = result_tx.send(body.clone());
        }
        drop(turns);
        self.cache_settled_result(cached)?;
        Ok(ToolResultDelivery::Delivered {
            body,
            request_id,
            bear_id,
            tool_name,
        })
    }

    pub fn pending_for_session(&self, session_id: &str) -> Vec<PendingToolTurn> {
        let Ok(turns) = self.turns.lock() else {
            return Vec::new();
        };
        let prefix = format!("{session_id}\n");
        turns
            .iter()
            .filter(|(key, turn)| key.starts_with(&prefix) && !turn.settled)
            .map(|(_, turn)| PendingToolTurn::from(turn))
            .collect()
    }

    pub fn expired_pending_for_session(&self, session_id: &str) -> Vec<PendingToolTurn> {
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
    ) -> Option<ToolResultRequest> {
        let mut turns = self.turns.lock().ok()?;
        let turn = turns.get_mut(&Self::key(session_id, tool_call_id))?;
        if turn.settled {
            return None;
        }
        turn.settled = true;
        let reason = reason.into();
        let body = ToolResultRequest {
            turn_id: None,
            request_id: Some(turn.request_id.to_string()),
            tool_call_id: Some(turn.tool_call_id.clone()),
            tool_name: Some(turn.tool_name.clone()),
            approval_request_id: turn.approval_request_id.clone(),
            status: "timeout".to_string(),
            content: Some(reason),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.armature",
                "phase": "auto_timeout_denial",
                "tool_call_id": turn.tool_call_id,
                "tool_name": turn.tool_name,
                "approval_request_id": turn.approval_request_id,
            }),
            ..Default::default()
        };
        let cached = SettledToolResult::from((&*turn, &body));
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
    ) -> ToolTurnCleanupSummary {
        let prefix = format!("{session_id}\n");
        let now = Instant::now();
        let mut summary = ToolTurnCleanupSummary::default();
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
    ) -> ToolTurnCleanupSummary {
        let prefix = format!("{session_id}\n");
        let mut summary = ToolTurnCleanupSummary::default();
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

    pub fn prepare_runtime_continuation(
        result: &ToolResultRequest,
    ) -> Result<PreparedRuntimeContinuation, PrepareRuntimeContinuationError> {
        let display_tool_name = result
            .tool_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("tool")
            .to_string();
        let Some(tool_call_id) = result.tool_call_id.clone() else {
            return Err(PrepareRuntimeContinuationError::MissingToolCallId { display_tool_name });
        };
        let content = result.content.clone().unwrap_or_default();
        let continuation = if let Some(approval_request_id) = result.approval_request_id.clone() {
            RuntimeContinuation::ApprovalDecision {
                approval_request_id,
                tool_call_id: Some(tool_call_id.clone()),
                decision: if result.status == "ok" {
                    RuntimeApprovalDecision::Approve
                } else {
                    RuntimeApprovalDecision::Deny
                },
                reason: Some(content),
            }
        } else {
            RuntimeContinuation::ToolResult {
                tool_call_id: tool_call_id.clone(),
                approval_request_id: None,
                status: match result.status.as_str() {
                    "ok" => RuntimeToolResultStatus::Ok,
                    "timeout" | "timed_out" => RuntimeToolResultStatus::Timeout,
                    _ => RuntimeToolResultStatus::Error,
                },
                content,
            }
        };
        Ok(PreparedRuntimeContinuation {
            tool_call_id,
            display_tool_name,
            continuation,
        })
    }

    pub fn settle_after_result(
        &self,
        session_id: &str,
        result: &ToolResultRequest,
    ) -> ToolSettlementSummary {
        let removed_pending_turn = result
            .tool_call_id
            .as_deref()
            .map(|tool_call_id| {
                if let Ok(mut turns) = self.turns.lock() {
                    turns.remove(&Self::key(session_id, tool_call_id)).is_some()
                } else {
                    false
                }
            })
            .unwrap_or(false);
        ToolSettlementSummary {
            tool_call_id: result.tool_call_id.clone(),
            tool_name: result.tool_name.clone(),
            display_tool_name: result
                .tool_name
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or("tool")
                .to_string(),
            completed_ok: result.status == "ok",
            timed_out: result.status == "timeout",
            status: result.status.clone(),
            removed_pending_turn,
        }
    }

    pub fn remove(&self, session_id: &str, tool_call_id: &str) {
        let key = Self::key(session_id, tool_call_id);
        if let Ok(mut turns) = self.turns.lock() {
            if let Some(mut turn) = turns.remove(&key) {
                if let Some(result_tx) = turn.result_tx.take() {
                    if let Ok(mut orphaned) = self.orphaned_result_txs.lock() {
                        orphaned.insert(key.clone(), result_tx);
                    }
                }
            }
        }
    }

    fn cache_settled_result(&self, result: SettledToolResult) -> Result<(), DenError> {
        let mut settled = self
            .settled_results
            .lock()
            .map_err(|_| poisoned_lock("armature settled tool result cache"))?;
        prune_settled_results(&mut settled);
        settled.insert(
            Self::key(&result.client_session_id, &result.tool_call_id),
            result,
        );
        prune_settled_results(&mut settled);
        Ok(())
    }

    pub fn recently_settled(
        &self,
        session_id: &str,
        tool_call_id: &str,
    ) -> Option<SettledToolResult> {
        let mut settled = self.settled_results.lock().ok()?;
        prune_settled_results(&mut settled);
        settled.get(&Self::key(session_id, tool_call_id)).cloned()
    }
}

fn prune_settled_results(settled: &mut HashMap<String, SettledToolResult>) {
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
mod tests;
