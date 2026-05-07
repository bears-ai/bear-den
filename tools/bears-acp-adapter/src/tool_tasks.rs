use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex as TokioMutex;

#[derive(Clone, Default)]
pub(crate) struct ToolTaskRegistry {
    tasks: Arc<TokioMutex<HashMap<String, ToolTaskRecord>>>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ToolTaskRecord {
    pub(crate) session_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) phase: ToolTaskPhase,
    pub(crate) started_at: std::time::Instant,
    pub(crate) updated_at: std::time::Instant,
}

impl ToolTaskRegistry {
    fn key(session_id: &str, tool_call_id: &str) -> String {
        format!("{session_id}\n{tool_call_id}")
    }

    pub(crate) async fn register(&self, session_id: &str, tool_call_id: &str, tool_name: &str) {
        let now = std::time::Instant::now();
        self.tasks.lock().await.insert(
            Self::key(session_id, tool_call_id),
            ToolTaskRecord {
                session_id: session_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                phase: ToolTaskPhase::Received,
                started_at: now,
                updated_at: now,
            },
        );
    }

    pub(crate) async fn set_phase(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        phase: ToolTaskPhase,
    ) {
        let mut tasks = self.tasks.lock().await;
        let key = Self::key(session_id, tool_call_id);
        let now = std::time::Instant::now();
        let entry = tasks.entry(key).or_insert_with(|| ToolTaskRecord {
            session_id: session_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            phase,
            started_at: now,
            updated_at: now,
        });
        let previous_phase = entry.phase;
        let previous_elapsed_ms = now.duration_since(entry.updated_at).as_millis();
        let total_elapsed_ms = now.duration_since(entry.started_at).as_millis();
        entry.phase = phase;
        entry.updated_at = now;
        eprintln!(
            "bears-acp-adapter: tool_task transition session_id={} tool_call_id={} tool_name={} from_phase={} to_phase={} phase_duration_ms={} total_duration_ms={}",
            session_id,
            tool_call_id,
            tool_name,
            previous_phase.as_str(),
            phase.as_str(),
            previous_elapsed_ms,
            total_elapsed_ms,
        );
    }

    pub(crate) async fn remove(&self, session_id: &str, tool_call_id: &str) -> Option<ToolTaskRecord> {
        let removed = self
            .tasks
            .lock()
            .await
            .remove(&Self::key(session_id, tool_call_id));
        if let Some(record) = removed.as_ref() {
            eprintln!(
                "bears-acp-adapter: tool_task finished session_id={} tool_call_id={} tool_name={} final_phase={} total_duration_ms={}",
                record.session_id,
                record.tool_call_id,
                record.tool_name,
                record.phase.as_str(),
                record.started_at.elapsed().as_millis(),
            );
        }
        removed
    }

    #[allow(dead_code)]
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
    eprintln!(
        "bears-acp-adapter: tool_task phase={} session_id={} tool_call_id={} tool_name={}",
        phase.as_str(),
        session_id,
        tool_call_id,
        tool_name
    );
}
