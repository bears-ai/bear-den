use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

#[derive(Clone, Default)]
pub(crate) struct ToolTaskRegistry {
    tasks: Arc<TokioMutex<HashMap<String, ToolTaskRecord>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ToolTaskRecord {
    pub(crate) session_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) turn_token: Option<Uuid>,
    pub(crate) execution_ownership: ToolExecutionOwnership,
    pub(crate) phase: ToolTaskPhase,
    pub(crate) input_args: Option<Value>,
    pub(crate) display: Option<Value>,
    pub(crate) visible_summary: Option<String>,
    pub(crate) started_at: std::time::Instant,
    pub(crate) updated_at: std::time::Instant,
}

impl ToolTaskRegistry {
    fn key(session_id: &str, tool_call_id: &str) -> String {
        format!("{session_id}\n{tool_call_id}")
    }

    pub(crate) async fn try_register(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        turn_token: Option<Uuid>,
        execution_ownership: ToolExecutionOwnership,
    ) -> bool {
        let now = std::time::Instant::now();
        let mut tasks = self.tasks.lock().await;
        let key = Self::key(session_id, tool_call_id);
        if tasks.contains_key(&key) {
            return false;
        }
        tasks.insert(
            key,
            ToolTaskRecord {
                session_id: session_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                turn_token,
                execution_ownership,
                phase: ToolTaskPhase::Received,
                input_args: None,
                display: None,
                visible_summary: None,
                started_at: now,
                updated_at: now,
            },
        );
        true
    }

    pub(crate) async fn remember_presentation(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        input_args: Value,
        display: Option<Value>,
    ) {
        let mut tasks = self.tasks.lock().await;
        let Some(entry) = tasks.get_mut(&Self::key(session_id, tool_call_id)) else {
            return;
        };
        entry.tool_name = tool_name.to_string();
        entry.input_args = Some(input_args);
        entry.display = display;
        entry.updated_at = std::time::Instant::now();
    }

    pub(crate) async fn remember_visible_summary(
        &self,
        session_id: &str,
        tool_call_id: &str,
        text: &str,
    ) {
        let text = text.trim();
        if text.is_empty() || text == "Completed." || is_generic_completion(text) {
            return;
        }
        let mut tasks = self.tasks.lock().await;
        let Some(entry) = tasks.get_mut(&Self::key(session_id, tool_call_id)) else {
            return;
        };
        entry.visible_summary = Some(text.to_string());
        entry.updated_at = std::time::Instant::now();
    }
    pub(crate) async fn get(&self, session_id: &str, tool_call_id: &str) -> Option<ToolTaskRecord> {
        self.tasks
            .lock()
            .await
            .get(&Self::key(session_id, tool_call_id))
            .cloned()
    }

    pub(crate) async fn set_phase(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        phase: ToolTaskPhase,
    ) {
        let mut tasks = self.tasks.lock().await;
        let now = std::time::Instant::now();
        let Some(entry) = tasks.get_mut(&Self::key(session_id, tool_call_id)) else {
            return;
        };
        let previous_phase = entry.phase;
        let previous_elapsed_ms = now.duration_since(entry.updated_at).as_millis();
        let total_elapsed_ms = now.duration_since(entry.started_at).as_millis();
        entry.phase = phase;
        entry.updated_at = now;
        tracing::debug!(
            target: "bear_armature::lifecycle",
            session_id,
            tool_call_id,
            tool_name,
            from_phase = previous_phase.as_str(),
            to_phase = phase.as_str(),
            phase_duration_ms = previous_elapsed_ms,
            total_duration_ms = total_elapsed_ms,
            "tool task phase transitioned"
        );
    }

    pub(crate) async fn remove(
        &self,
        session_id: &str,
        tool_call_id: &str,
    ) -> Option<ToolTaskRecord> {
        let removed = self
            .tasks
            .lock()
            .await
            .remove(&Self::key(session_id, tool_call_id));
        if let Some(record) = removed.as_ref() {
            if record.phase != ToolTaskPhase::ResultPosted {
                tracing::debug!(
                    target: "bear_armature::lifecycle",
                    session_id = record.session_id,
                    tool_call_id = record.tool_call_id,
                    tool_name = record.tool_name,
                    final_phase = record.phase.as_str(),
                    total_duration_ms = record.started_at.elapsed().as_millis(),
                    "tool task finished before posting a result"
                );
            }
        }
        removed
    }

    pub(crate) async fn cancel_session(&self, session_id: &str) {
        self.cancel_matching(session_id, None).await;
    }

    pub(crate) async fn cancel_turn(&self, session_id: &str, turn_token: Uuid) {
        self.cancel_matching(session_id, Some(turn_token)).await;
    }

    async fn cancel_matching(&self, session_id: &str, turn_token: Option<Uuid>) {
        let mut tasks = self.tasks.lock().await;
        let now = std::time::Instant::now();
        tasks.retain(|_, task| {
            let matches = task.session_id == session_id
                && turn_token.is_none_or(|token| task.turn_token == Some(token));
            if !matches {
                return true;
            }
            if task.phase != ToolTaskPhase::ResultPosted {
                tracing::debug!(
                    target: "bear_armature::lifecycle",
                    session_id = task.session_id,
                    turn_token = ?task.turn_token,
                    tool_call_id = task.tool_call_id,
                    tool_name = task.tool_name,
                    from_phase = task.phase.as_str(),
                    total_duration_ms = now.duration_since(task.started_at).as_millis(),
                    "tool task cancelled"
                );
            }
            false
        });
    }

    pub(crate) async fn has_active_execution(&self, session_id: &str) -> bool {
        self.tasks.lock().await.values().any(|task| {
            task.session_id == session_id
                && task.execution_ownership == ToolExecutionOwnership::ArmatureLocal
                && matches!(
                    task.phase,
                    ToolTaskPhase::ExecutionStarted
                        | ToolTaskPhase::ExecutionSucceeded
                        | ToolTaskPhase::ExecutionFailed
                        | ToolTaskPhase::ResultPostFailed
                )
        })
    }

    pub(crate) async fn list_for_session(&self, session_id: &str) -> Vec<ToolTaskRecord> {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|task| task.session_id == session_id)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolExecutionOwnership {
    ArmatureLocal,
    DenDisplayOnly,
}

fn is_generic_completion(text: &str) -> bool {
    let normalized = text.trim().trim_end_matches('.').to_ascii_lowercase();
    normalized == "completed" || normalized.ends_with(" completed")
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolTaskPhase {
    Received,
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    PermissionTimeout,
    ExecutionStarted,
    ExecutionSucceeded,
    ExecutionFailed,
    ResultPosted,
    ResultPostFailed,
    Cancelled,
}

impl ToolTaskPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::PermissionRequested => "permission_requested",
            Self::PermissionGranted => "permission_granted",
            Self::PermissionDenied => "permission_denied",
            Self::PermissionTimeout => "permission_timeout",
            Self::ExecutionStarted => "execution_started",
            Self::ExecutionSucceeded => "execution_succeeded",
            Self::ExecutionFailed => "execution_failed",
            Self::ResultPosted => "result_posted",
            Self::ResultPostFailed => "result_post_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(crate) fn log_tool_task_phase(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    phase: ToolTaskPhase,
) {
    tracing::debug!(
        target: "bear_armature::lifecycle",
        session_id,
        tool_call_id,
        tool_name,
        phase = phase.as_str(),
        "tool task phase reached"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn active_execution_tracks_only_command_ownership_phases() {
        let registry = ToolTaskRegistry::default();
        assert!(
            registry
                .try_register(
                    "session-a",
                    "call-a",
                    "run_command",
                    Some(Uuid::new_v4()),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            registry
                .try_register(
                    "session-a",
                    "call-display",
                    "memory_search",
                    Some(Uuid::new_v4()),
                    ToolExecutionOwnership::DenDisplayOnly,
                )
                .await
        );
        registry
            .set_phase(
                "session-a",
                "call-display",
                "memory_search",
                ToolTaskPhase::ExecutionStarted,
            )
            .await;
        assert!(
            !registry.has_active_execution("session-a").await,
            "Den-owned display cards must not count as Armature-local execution"
        );

        registry
            .set_phase(
                "session-a",
                "call-a",
                "run_command",
                ToolTaskPhase::ExecutionStarted,
            )
            .await;
        assert!(registry.has_active_execution("session-a").await);
        assert!(!registry.has_active_execution("session-b").await);

        registry.remove("session-a", "call-a").await;
        assert!(!registry.has_active_execution("session-a").await);
    }

    #[tokio::test]
    async fn cancelling_session_evicts_all_matching_task_records() {
        let registry = ToolTaskRegistry::default();
        let turn = Uuid::new_v4();
        assert!(
            registry
                .try_register(
                    "session-a",
                    "call-a",
                    "list_jobs",
                    Some(turn),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            registry
                .try_register(
                    "session-a",
                    "call-b",
                    "create_job",
                    Some(Uuid::new_v4()),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            registry
                .try_register(
                    "session-b",
                    "call-c",
                    "list_jobs",
                    Some(turn),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );

        registry.cancel_session("session-a").await;

        assert!(registry.list_for_session("session-a").await.is_empty());
        assert_eq!(registry.list_for_session("session-b").await.len(), 1);
    }

    #[tokio::test]
    async fn phase_and_input_updates_do_not_recreate_cancelled_records() {
        let registry = ToolTaskRegistry::default();
        let turn = Uuid::new_v4();
        assert!(
            registry
                .try_register(
                    "session-a",
                    "call-a",
                    "fs_read_text_file",
                    Some(turn),
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        registry.cancel_session("session-a").await;

        registry
            .remember_presentation(
                "session-a",
                "call-a",
                "fs_read_text_file",
                serde_json::json!({"path":"README.md"}),
                None,
            )
            .await;
        registry
            .set_phase(
                "session-a",
                "call-a",
                "fs_read_text_file",
                ToolTaskPhase::ExecutionStarted,
            )
            .await;

        assert!(registry.list_for_session("session-a").await.is_empty());
    }

    #[tokio::test]
    async fn registry_tracks_phase_and_session_entries() {
        let registry = ToolTaskRegistry::default();
        assert!(
            registry
                .try_register(
                    "session-1",
                    "call-1",
                    "fs_list_directory",
                    None,
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        registry
            .set_phase(
                "session-1",
                "call-1",
                "fs_list_directory",
                ToolTaskPhase::PermissionRequested,
            )
            .await;
        let items = registry.list_for_session("session-1").await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].phase, ToolTaskPhase::PermissionRequested);
        assert_eq!(items[0].tool_name, "fs_list_directory");
        assert!(items[0].updated_at >= items[0].started_at);
        let removed = registry.remove("session-1", "call-1").await.unwrap();
        assert_eq!(removed.tool_call_id, "call-1");
        assert!(registry.list_for_session("session-1").await.is_empty());
    }

    #[tokio::test]
    async fn try_register_rejects_duplicate_tool_call_for_session() {
        let registry = ToolTaskRegistry::default();

        assert!(
            registry
                .try_register(
                    "session-1",
                    "call-1",
                    "fs_read_text_file",
                    None,
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            !registry
                .try_register(
                    "session-1",
                    "call-1",
                    "fs_read_text_file",
                    None,
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            registry
                .try_register(
                    "session-1",
                    "call-2",
                    "fs_read_text_file",
                    None,
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
        assert!(
            registry
                .try_register(
                    "session-2",
                    "call-1",
                    "fs_read_text_file",
                    None,
                    ToolExecutionOwnership::ArmatureLocal,
                )
                .await
        );
    }
}
