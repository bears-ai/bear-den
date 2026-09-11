use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bearwire_protocol::lifecycle::{
    FocusedExecutionDiagnostics, FocusedExecutionTransition, FocusedExecutionTransitionRecord,
    FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

const LOCAL_TRANSITION_LIMIT: usize = 64;

#[derive(Clone, Default)]
pub(crate) struct ExecutionDiagnosticStore {
    sessions: Arc<Mutex<HashMap<String, ExecutionTimeline>>>,
}

#[derive(Clone, Default)]
struct ExecutionTimeline {
    snapshot: Option<Value>,
    transitions: VecDeque<FocusedExecutionTransitionRecord>,
    history_truncated: bool,
    version_gap: bool,
    snapshot_matches_latest_transition: bool,
    reason_counts: BTreeMap<String, u64>,
    next_refresh_at: Option<Instant>,
}

#[derive(Clone, Debug)]
pub(crate) struct ObservedExecutionTransition {
    pub(crate) record: FocusedExecutionTransitionRecord,
    pub(crate) gap_detected: bool,
}

impl ObservedExecutionTransition {
    pub(crate) fn display_line(&self) -> String {
        let transition = &self.record.transition;
        let from = transition
            .from
            .map(|state| format!("{state:?}"))
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "Execution v{}: {} → {:?} ({})\nrun={} task={} attempt={} correlation={}{}",
            transition.state_version,
            from,
            transition.to,
            transition.reason.as_str(),
            transition.run_id.as_deref().unwrap_or("none"),
            transition.task_id.as_deref().unwrap_or("none"),
            transition.attempt_id.as_deref().unwrap_or("none"),
            transition.correlation_id,
            if self.gap_detected {
                "\nDiagnostic version gap detected; refreshing the authoritative snapshot."
            } else {
                ""
            },
        )
    }
}

impl ExecutionDiagnosticStore {
    pub(crate) async fn observe_event(
        &self,
        session_id: &str,
        event: &Value,
    ) -> Result<Option<ObservedExecutionTransition>> {
        if event.get("type").and_then(Value::as_str)
            != Some(FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE)
        {
            return Ok(None);
        }
        let transition: FocusedExecutionTransition = serde_json::from_value(
            event
                .get("data")
                .cloned()
                .ok_or_else(|| anyhow!("focused-execution diagnostic event omitted data"))?,
        )?;
        if transition.session_id != session_id {
            return Err(anyhow!(
                "focused-execution diagnostic session mismatch: event={} active={session_id}",
                transition.session_id
            ));
        }
        let sequence = event
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("focused-execution diagnostic event omitted sequence"))?;
        let time = event
            .get("time")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("focused-execution diagnostic event omitted time"))?
            .to_string();
        let record = FocusedExecutionTransitionRecord {
            sequence,
            time,
            transition,
        };

        let mut sessions = self.sessions.lock().await;
        let timeline = sessions.entry(session_id.to_string()).or_default();
        if let Some(latest) = timeline.transitions.back() {
            if record.transition.state_version <= latest.transition.state_version {
                return Ok(None);
            }
        }
        let gap_detected = timeline.transitions.back().is_some_and(|latest| {
            record.transition.state_version != latest.transition.state_version.saturating_add(1)
        }) || (timeline.transitions.is_empty()
            && record.transition.state_version > 1);
        timeline.version_gap |= gap_detected;
        timeline.snapshot_matches_latest_transition = false;
        *timeline
            .reason_counts
            .entry(record.transition.reason.as_str().to_string())
            .or_insert(0) += 1;
        timeline.transitions.push_back(record.clone());
        if timeline.transitions.len() > LOCAL_TRANSITION_LIMIT {
            timeline.transitions.pop_front();
            timeline.history_truncated = true;
        }
        Ok(Some(ObservedExecutionTransition {
            record,
            gap_detected,
        }))
    }

    pub(crate) async fn claim_refresh(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.lock().await;
        let Some(timeline) = sessions.get_mut(session_id) else {
            return false;
        };
        if !timeline.version_gap
            || timeline
                .next_refresh_at
                .is_some_and(|next| next > Instant::now())
        {
            return false;
        }
        timeline.next_refresh_at = Some(Instant::now() + Duration::from_secs(5));
        true
    }

    pub(crate) async fn replace_authoritative(
        &self,
        session_id: &str,
        diagnostics: FocusedExecutionDiagnostics,
    ) {
        let next_refresh_at = diagnostics
            .version_gap
            .then(|| Instant::now() + Duration::from_secs(5));
        let transitions = diagnostics.transitions.into_iter().collect::<VecDeque<_>>();
        self.sessions.lock().await.insert(
            session_id.to_string(),
            ExecutionTimeline {
                snapshot: Some(serde_json::to_value(diagnostics.snapshot).unwrap_or(Value::Null)),
                transitions,
                history_truncated: diagnostics.history_truncated,
                version_gap: diagnostics.version_gap,
                snapshot_matches_latest_transition: diagnostics.snapshot_matches_latest_transition,
                reason_counts: diagnostics.reason_counts,
                next_refresh_at,
            },
        );
    }

    pub(crate) async fn remove_session(&self, session_id: &str) {
        self.sessions.lock().await.remove(session_id);
    }

    pub(crate) async fn bundle_json(&self, session_id: &str) -> String {
        let sessions = self.sessions.lock().await;
        let Some(timeline) = sessions.get(session_id) else {
            return "No focused-execution transitions observed for this client session."
                .to_string();
        };
        serde_json::to_string_pretty(&json!({
            "session_id": session_id,
            "snapshot": timeline.snapshot,
            "transitions": timeline.transitions,
            "history_truncated": timeline.history_truncated,
            "version_gap": timeline.version_gap,
            "snapshot_matches_latest_transition": timeline.snapshot_matches_latest_transition,
            "reason_counts": timeline.reason_counts,
        }))
        .unwrap_or_else(|error| format!("Could not render execution diagnostics: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bearwire_protocol::lifecycle::{FocusedExecutionState, FocusedExecutionTransitionReason};

    fn event(version: u64) -> Value {
        json!({
            "type": FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
            "sequence": version + 10,
            "time": "2026-09-11T00:00:00Z",
            "data": {
                "state_version": version,
                "from": if version == 1 { Value::Null } else { json!({"phase": "starting"}) },
                "to": {"phase": "running"},
                "reason": FocusedExecutionTransitionReason::AuthorityStarted,
                "correlation_id": "run-1",
                "session_id": "session-1",
                "task_id": "task-1",
                "run_id": "run-1",
                "attempt_id": "attempt-1",
                "fence_epoch": 1,
                "open_obligations": 0,
                "task_selection_preserved": true
            }
        })
    }

    #[tokio::test]
    async fn tracks_gaps_then_accepts_an_authoritative_replacement() {
        let store = ExecutionDiagnosticStore::default();
        store.observe_event("session-1", &event(1)).await.unwrap();
        let observed = store
            .observe_event("session-1", &event(3))
            .await
            .unwrap()
            .expect("new transition");
        assert!(observed.gap_detected);
        assert!(store.claim_refresh("session-1").await);
        assert!(!store.claim_refresh("session-1").await);

        let transitions = [1, 2, 3]
            .into_iter()
            .map(|version| {
                let event = event(version);
                FocusedExecutionTransitionRecord {
                    sequence: event["sequence"].as_u64().unwrap(),
                    time: event["time"].as_str().unwrap().to_string(),
                    transition: serde_json::from_value(event["data"].clone()).unwrap(),
                }
            })
            .collect();
        store
            .replace_authoritative(
                "session-1",
                FocusedExecutionDiagnostics {
                    snapshot: serde_json::from_value(json!({
                        "session_id": "session-1",
                        "state": {"phase": "running"},
                        "task": {"id": "task-1"},
                        "binding": {"kind": "client_session", "id": "session-1"},
                        "run": {"id": "run-1", "state": "running"},
                        "attempt": {"id": "attempt-1", "state": "running", "fence_epoch": 1},
                        "host": {"kind": "pair", "run_id": "run-1"},
                        "controller": "live",
                        "obligations": {"open": 0},
                        "launch_state": "already_running"
                    }))
                    .unwrap(),
                    transitions,
                    history_truncated: false,
                    version_gap: false,
                    snapshot_matches_latest_transition: true,
                    reason_counts: BTreeMap::from([("authority_started".to_string(), 3)]),
                },
            )
            .await;
        assert!(!store.claim_refresh("session-1").await);
        let bundle = store.bundle_json("session-1").await;
        assert!(bundle.contains("\"snapshot_matches_latest_transition\": true"));
        assert!(bundle.contains("attempt-1"));
        assert_eq!(
            observed.record.transition.to,
            FocusedExecutionState::Running
        );
    }
}
