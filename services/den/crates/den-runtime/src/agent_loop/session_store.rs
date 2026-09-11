use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use den_core::{
    governance::Governance,
    profile::BearProfile,
    tools::capability_catalog::{CapabilityEntry, SessionCapabilityDescriptor},
};
use den_docket::TaskListProjection;
use den_protocol::ContextBudgetReport;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    agent_loop::{
        CheckpointNextAction, CheckpointState, KeyMemoryProjectionCacheKey, ObjectiveOrientation,
        ResolvedAgentLoopControl, RuntimeCheckpointRequest, StrategyProfile, ToolCallBudgetLimits,
        TurnBudgetPolicy, TurnBudgetState,
    },
    context_budget::AssembledTurnBudgetComponents,
    llm::{ChatMessage, ChatToolCall, LlmApiStyle, LlmRequestTelemetry, LlmToolDefinition},
};

const RECENT_CAPABILITY_LIMIT: usize = 6;

#[derive(Debug, Clone)]
pub struct RecentCapability {
    pub entry: CapabilityEntry,
}

pub fn retain_recent_capability_entries(
    recent: &mut Vec<RecentCapability>,
    entries: impl IntoIterator<Item = CapabilityEntry>,
) {
    for entry in entries {
        recent.retain(|current| current.entry.r#ref != entry.r#ref);
        recent.push(RecentCapability { entry });
    }
    if recent.len() > RECENT_CAPABILITY_LIMIT {
        recent.drain(..recent.len() - RECENT_CAPABILITY_LIMIT);
    }
}

pub fn discovered_capability_entries(tool_name: &str, result: &Value) -> Vec<CapabilityEntry> {
    match tool_name {
        "capability_search" => result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
            .collect(),
        "capability_describe" => result
            .get("capability")
            .cloned()
            .and_then(|entry| serde_json::from_value(entry).ok())
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

pub fn render_recently_discovered_capabilities(recent: &[RecentCapability]) -> String {
    if recent.is_empty() {
        return String::new();
    }
    let mut text =
        String::from("Recently discovered (context only; re-check authority before use):\n");
    for recent in recent {
        let entry = &recent.entry;
        text.push_str(&format!("- `{}`: {}", entry.r#ref, entry.summary));
        if entry.risk != "read_only" {
            text.push_str(&format!(" Risk: {}; scope: {}.", entry.risk, entry.surface));
        }
        if entry.descriptor_lifecycle == "session_instance" {
            text.push_str(&format!(
                " Locality: {}; surface: {}; authority: {}; lifetime: {}.",
                entry.execution_locality, entry.surface, entry.authority, entry.lifetime
            ));
        }
        text.push('\n');
    }
    text
}

#[derive(Debug, Clone)]
pub struct AgentLoopSession {
    pub session_key: String,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub user_id: Option<i32>,
    pub conversation_id: String,
    pub client_session_id: String,
    /// Concrete dispatched work run bound by `work.checkout`. This is copied
    /// into Den-hosted tool invocations; it is absent for ordinary sessions.
    pub work_run_id: Option<Uuid>,
    /// Adapter-resolved opaque IDs used exclusively for checkpoint audit persistence.
    pub checkpoint_audit_context: Option<den_protocol::CheckpointAuditContext>,
    pub workspace_roots: Vec<String>,
    pub session_capabilities: Vec<SessionCapabilityDescriptor>,
    /// Bounded, volatile projection of successful catalog lookups for the next model step.
    /// This is context only; it never grants capability authority.
    pub recently_discovered_capabilities: Vec<RecentCapability>,
    pub request_id: Option<String>,
    pub run_id: Option<String>,
    /// Sanitized adapter payload captured at turn start and persisted only at
    /// a technical-budget continuation boundary.
    pub technical_budget_recovery_start_payload: Option<Value>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<LlmToolDefinition>,
    pub budget_components: AssembledTurnBudgetComponents,
    pub model: String,
    /// Base model-task policy resolved at native turn construction. Per-step metadata is
    /// derived from this approved model reference immediately before each request.
    pub model_request_profile: den_core::ModelRequestProfile,
    pub model_context_window: Option<u32>,
    pub model_max_output_tokens: Option<u32>,
    /// Observed chars→tokens correction for this model, mirrored from the Den
    /// model registry at session build (ADR-0047 §7). `None` falls back to the
    /// chars/4 heuristic.
    pub model_token_calibration: Option<den_llm::model_registry::ModelTokenCalibration>,
    pub bifrost_virtual_key: Option<String>,
    pub api_style: Option<LlmApiStyle>,
    pub step: u32,
    pub turn_budget: TurnBudgetPolicy,
    pub turn_budget_state: TurnBudgetState,
    pub agent_loop_control: ResolvedAgentLoopControl,
    pub governance: Governance,
    pub objective_orientation: ObjectiveOrientation,
    pub checkpoint_state: CheckpointState,
    pub pending_checkpoint_request: Option<RuntimeCheckpointRequest>,
    pub pending_checkpoint_task_action: Option<CheckpointNextAction>,
    pub pending_checkpoint_recovery_attempts: u8,
    pub strategy: StrategyProfile,
    pub stream_tokens: bool,
    pub key_memory_projection_cache_key: Option<KeyMemoryProjectionCacheKey>,
    pub latest_context_budget: Option<ContextBudgetReport>,
    pub latest_projected_memory: Option<Value>,
    pub latest_recalled_memory: Option<Value>,
    /// Volatile task-list projection cache. Durable Docket execution is the
    /// source of truth for focused work; behavior decisions should resolve
    /// `RuntimeTaskContext` instead of treating this cache as authoritative.
    pub cached_activity_plan_projection: Option<TaskListProjection>,
    pub profile: BearProfile,
    pub overflow_retry_attempted: bool,
    pub overflow_compaction_recovered: bool,
}

impl AgentLoopSession {
    pub fn find_pending_tool_call(&self, tool_call_id: &str) -> Option<ChatToolCall> {
        self.messages
            .iter()
            .rev()
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .find(|call| call.id == tool_call_id)
            .cloned()
    }

    pub fn llm_telemetry(&self) -> LlmRequestTelemetry {
        LlmRequestTelemetry {
            request_id: self.request_id.clone(),
            run_id: self.run_id.clone(),
            session_id: Some(self.client_session_id.clone()),
            conversation_id: Some(self.conversation_id.clone()),
            bear_id: Some(self.bear_id.to_string()),
            stance: Some(self.profile.as_str().to_string()),
            operation: Some(den_llm::LlmOperation::AgentTurn),
            bifrost_virtual_key: self.bifrost_virtual_key.clone(),
        }
    }

    pub fn session_info_runtime_snapshot(&self) -> Value {
        let elapsed_ms = self.turn_budget_state.started_at.elapsed().as_millis() as u64;
        let remaining_wall_clock_ms = self
            .turn_budget
            .max_wall_clock_ms
            .saturating_sub(elapsed_ms);
        let next_incomplete_task = self
            .cached_activity_plan_projection
            .as_ref()
            .and_then(|plan| {
                plan.items.iter().find(|item| {
                    matches!(
                        item.status,
                        den_docket::TaskListItemStatus::Pending
                            | den_docket::TaskListItemStatus::InProgress
                    )
                })
            })
            .map(|item| item.title.clone())
            .or_else(|| orientation_active_task_title(&self.objective_orientation));
        let task_focus_active = self.cached_activity_plan_projection.is_some()
            || !matches!(
                self.objective_orientation,
                ObjectiveOrientation::Freeform { .. }
            );
        json!({
            "schema": "den.runtime_state.v1",
            "state": "active",
            "source": "native_agent_loop_session",
            "run": {
                "run_id": self.run_id.as_deref(),
                "stance": self.profile.as_str(),
                "governance": self.governance.as_str(),
                "objective_orientation_kind": self.objective_orientation.kind(),
                "docket_job_id": docket_job_id(&self.objective_orientation),
            },
            "active_turn": {
                "present": true,
                "step": self.step,
                "elapsed_ms": elapsed_ms,
                "wall_clock": {
                    "elapsed_ms": elapsed_ms,
                    "remaining_ms": remaining_wall_clock_ms,
                    "limit_ms": self.turn_budget.max_wall_clock_ms,
                },
                "pending_obligations": 0,
                "pending_adapter_tools": 0,
                "pending_den_tools": pending_tool_call_count(&self.messages),
                "pending_permissions": 0,
            },
            "agent_loop_control": serde_json::to_value(&self.agent_loop_control).unwrap_or_else(|_| json!({
                "level": self.agent_loop_control.level.as_str(),
            })),
            "objective_orientation": serde_json::to_value(&self.objective_orientation).unwrap_or_else(|_| json!({
                "kind": self.objective_orientation.kind(),
            })),
            "checkpoint_state": serde_json::to_value(&self.checkpoint_state).unwrap_or(Value::Null),
            "pending_checkpoint_request": serde_json::to_value(&self.pending_checkpoint_request).unwrap_or(Value::Null),
            "pending_checkpoint_task_action": serde_json::to_value(&self.pending_checkpoint_task_action).unwrap_or(Value::Null),
            "budgets": {
                "turn": {
                    "max_wall_clock_ms": self.turn_budget.max_wall_clock_ms,
                    "emergency_hard_steps": self.turn_budget.emergency_hard_steps,
                    "remaining_steps_before_hard_fuse": self.turn_budget.emergency_hard_steps.saturating_sub(self.step),
                    "max_consecutive_tool_failures": self.turn_budget.max_consecutive_tool_failures,
                    "max_same_tool_signature_repeats": self.turn_budget.max_same_tool_signature_repeats,
                    "budget_finalization_grace_used": self.turn_budget_state.budget_finalization_grace_used,
                },
                "tool_calls": {
                    "limits": tool_call_limits_json(self.turn_budget.tool_call_limits),
                    "usage": tool_call_usage_json(self.turn_budget_state.tool_usage),
                    "post_mutation_verification_window": self.turn_budget.post_mutation_verification_window.map(|window| json!({
                        "replenish_read": window.replenish_read,
                        "replenish_search": window.replenish_search,
                    })),
                },
            },
            "loop_guards": {
                "consecutive_tool_failures": self.turn_budget_state.consecutive_tool_failures,
                "max_consecutive_tool_failures": self.turn_budget.max_consecutive_tool_failures,
                "same_tool_signature_repeats": self.turn_budget_state.same_batch_signature_repeats,
                "max_same_tool_signature_repeats": self.turn_budget.max_same_tool_signature_repeats,
                "last_batch_signature_present": self.turn_budget_state.last_batch_signature.is_some(),
            },
            "task_focus": {
                "active": task_focus_active,
                "plan_id": self.cached_activity_plan_projection.as_ref().map(|plan| plan.id.to_string()),
                "plan_title": self.cached_activity_plan_projection.as_ref().map(|plan| plan.title.clone()),
                "plan_status": self.cached_activity_plan_projection.as_ref().map(|plan| plan.status.clone()),
                "next_incomplete_task_title": next_incomplete_task,
                "continuation_policy": if task_focus_active {
                    "continue_until_complete_or_blocked"
                } else {
                    "inactive"
                },
            },
            "docket": docket_context_json(self.cached_activity_plan_projection.as_ref(), &self.objective_orientation),
            "last_budget_advisory": last_system_advisory(&self.messages, "Budget advisory:"),
            "last_task_focus_advisory": last_system_advisory(&self.messages, "You are in autonomous implementation mode."),
        })
    }
}

fn docket_job_id(orientation: &ObjectiveOrientation) -> Option<&str> {
    match orientation {
        ObjectiveOrientation::DocketExecution { job } => Some(job.job_id.as_str()),
        ObjectiveOrientation::Freeform { .. } | ObjectiveOrientation::Oriented { .. } => None,
    }
}

fn orientation_active_task_title(orientation: &ObjectiveOrientation) -> Option<String> {
    let task_ref = match orientation {
        ObjectiveOrientation::DocketExecution { job } => job.active_task_ref.as_ref()?,
        ObjectiveOrientation::Oriented { task } => Some(&task.task_ref)?,
        ObjectiveOrientation::Freeform { .. } => return None,
    };
    match task_ref {
        crate::agent_loop::OrientationTaskRef::TaskListItem { title, .. }
        | crate::agent_loop::OrientationTaskRef::DocketTask { title, .. } => title.clone(),
    }
}

fn pending_tool_call_count(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .rev()
        .find_map(|message| message.tool_calls.as_ref().map(Vec::len))
        .unwrap_or(0)
}

fn docket_context_json(
    plan: Option<&TaskListProjection>,
    orientation: &ObjectiveOrientation,
) -> Value {
    let active_task = plan
        .and_then(|plan| plan.current_item.as_ref())
        .or_else(|| {
            plan.and_then(|plan| {
                plan.items.iter().find(|item| {
                    matches!(
                        item.status,
                        den_docket::TaskListItemStatus::Pending
                            | den_docket::TaskListItemStatus::InProgress
                    )
                })
            })
        });
    let orientation_task_ref = match orientation {
        ObjectiveOrientation::DocketExecution { job } => job.active_task_ref.as_ref(),
        ObjectiveOrientation::Oriented { task } => Some(&task.task_ref),
        ObjectiveOrientation::Freeform { .. } => None,
    };
    json!({
        "active_job_id": plan
            .and_then(|plan| plan.source_ref.docket_job_id.clone())
            .or_else(|| docket_job_id(orientation).map(str::to_string))
            .or_else(|| plan.map(|plan| plan.id.to_string())),
        "active_run_id": Value::Null,
        "active_task_id": active_task
            .and_then(|item| item.source_ref.docket_task_id.clone())
            .or_else(|| orientation_docket_task_id(orientation_task_ref)),
        "active_task_title": active_task
            .map(|item| item.title.clone())
            .or_else(|| orientation_active_task_title(orientation)),
        "source": if plan.is_some() {
            "task_focus_projection"
        } else if !matches!(orientation, ObjectiveOrientation::Freeform { .. }) {
            "objective_orientation"
        } else {
            "none"
        },
    })
}

fn orientation_docket_task_id(
    task_ref: Option<&crate::agent_loop::OrientationTaskRef>,
) -> Option<String> {
    match task_ref? {
        crate::agent_loop::OrientationTaskRef::DocketTask { task_id, .. } => Some(task_id.clone()),
        crate::agent_loop::OrientationTaskRef::TaskListItem { .. } => None,
    }
}

fn last_system_advisory(messages: &[ChatMessage], prefix: &str) -> Option<Value> {
    messages.iter().rev().find_map(|message| {
        let content = message.content.as_deref()?;
        if message.role == "system" && content.starts_with(prefix) {
            Some(json!({
                "present": true,
                "summary": content,
            }))
        } else {
            None
        }
    })
}

fn tool_call_limits_json(limits: ToolCallBudgetLimits) -> Value {
    json!({
        "total": limits.total,
        "read": limits.read,
        "search": limits.search,
        "fetch": limits.fetch,
        "execute": limits.execute,
        "write": limits.write,
        "destructive": limits.destructive,
        "other": limits.other,
    })
}

fn tool_call_usage_json(usage: crate::agent_loop::ToolCallBudgetUsage) -> Value {
    json!({
        "total": usage.total,
        "read": usage.read,
        "search": usage.search,
        "fetch": usage.fetch,
        "execute": usage.execute,
        "write": usage.write,
        "destructive": usage.destructive,
        "other": usage.other,
    })
}

#[derive(Clone, Default)]
pub struct AgentLoopSessionStore {
    inner: Arc<Mutex<HashMap<String, AgentLoopSession>>>,
}

impl AgentLoopSessionStore {
    pub fn insert(&self, session: AgentLoopSession) {
        let key = session.session_key.clone();
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, session);
    }

    pub fn get(&self, key: &str) -> Option<AgentLoopSession> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub fn update(&self, key: &str, update: impl FnOnce(&mut AgentLoopSession)) {
        if let Some(session) = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(key)
        {
            update(session);
        }
    }

    pub fn update_client_sessions(
        &self,
        conversation_id: &str,
        client_session_id: &str,
        mut update: impl FnMut(&mut AgentLoopSession),
    ) {
        let mut sessions = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for session in sessions.values_mut().filter(|session| {
            session.conversation_id == conversation_id
                && session.client_session_id == client_session_id
        }) {
            update(session);
        }
    }

    pub fn remove_client_run(&self, client_session_id: &str, run_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, session| {
                session.client_session_id != client_session_id
                    || session.run_id.as_deref() != Some(run_id)
            });
    }

    pub fn remove(&self, key: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }
}

pub fn agent_loop_session_key(
    conversation_id: &str,
    client_session_id: &str,
    run_id: &str,
) -> String {
    format!("{conversation_id}:{client_session_id}:{run_id}")
}

#[cfg(test)]
mod tests {
    use den_core::profile::BearProfile;

    use crate::agent_loop::{
        resolve_agent_loop_control, AgentLoopControlResolutionInput, DocketExecutionOrientation,
        FreeformPolicy, ObjectiveOrientation, PostMutationVerificationWindow, StrategyProfile,
        ToolCallBudgetLimits,
    };

    use super::*;

    fn test_session(objective_orientation: ObjectiveOrientation) -> AgentLoopSession {
        AgentLoopSession {
            session_key: "den-conv-test:client-test".to_string(),
            bear_id: Uuid::nil(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(7),
            conversation_id: "den-conv-test".to_string(),
            client_session_id: "client-test".to_string(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec!["/workspace".to_string()],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: Some("request-test".to_string()),
            run_id: Some("run-test".to_string()),
            technical_budget_recovery_start_payload: None,
            messages: Vec::new(),
            tools: Vec::new(),
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 0,
            turn_budget: TurnBudgetPolicy {
                max_wall_clock_ms: 60_000,
                emergency_hard_steps: 16,
                tool_call_limits: ToolCallBudgetLimits {
                    total: 8,
                    read: 6,
                    search: 4,
                    fetch: 2,
                    execute: 2,
                    write: 2,
                    destructive: 1,
                    other: 2,
                },
                max_consecutive_tool_failures: 2,
                max_same_tool_signature_repeats: 1,
                post_mutation_verification_window: Some(PostMutationVerificationWindow {
                    replenish_read: 2,
                    replenish_search: 1,
                }),
            },
            turn_budget_state: Default::default(),
            agent_loop_control: resolve_agent_loop_control(AgentLoopControlResolutionInput {
                model_handle: Some("openai/test"),
                model_default: None,
                bear_override: None,
                stance_override: None,
                task_escalation: None,
                stance: Some(BearProfile::Pair),
                objective_orientation: Some(&objective_orientation),
                pre_risk: false,
            }),
            governance: Governance::Interactive,
            objective_orientation,
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: true,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        }
    }

    #[test]
    fn runtime_snapshot_includes_run_orientation_without_focus() {
        let session = test_session(ObjectiveOrientation::Freeform {
            policy: FreeformPolicy::closed(),
        });

        let snapshot = session.session_info_runtime_snapshot();
        let run = &snapshot["run"];

        assert_eq!(run["run_id"], "run-test");
        assert_eq!(run["stance"], "pair");
        assert_eq!(run["governance"], "interactive");
        assert_eq!(run["objective_orientation_kind"], "freeform");
        assert!(run["docket_job_id"].is_null());
    }

    #[test]
    fn runtime_snapshot_uses_session_governance() {
        let mut session = test_session(ObjectiveOrientation::Freeform {
            policy: FreeformPolicy::closed(),
        });
        session.governance = Governance::AutonomousContinuation;

        let snapshot = session.session_info_runtime_snapshot();

        assert_eq!(snapshot["run"]["governance"], "autonomous_continuation");
    }

    #[test]
    fn runtime_snapshot_includes_docket_job_id() {
        let session = test_session(ObjectiveOrientation::DocketExecution {
            job: DocketExecutionOrientation {
                job_id: "job-123".to_string(),
                active_task_ref: None,
                mutable: true,
            },
        });

        let snapshot = session.session_info_runtime_snapshot();
        let run = &snapshot["run"];

        assert_eq!(run["objective_orientation_kind"], "docket_execution");
        assert_eq!(run["docket_job_id"], "job-123");
        assert_eq!(snapshot["task_focus"]["active"], true);
        assert_eq!(snapshot["docket"]["active_job_id"], "job-123");
        assert_eq!(snapshot["docket"]["source"], "objective_orientation");
    }

    #[test]
    fn recently_discovered_entries_are_bounded_deduplicated_and_rendered() {
        let entry = |ref_name: &str, lifecycle: &str| {
            serde_json::from_value(serde_json::json!({
                "ref": ref_name,
                "kind": "tool",
                "summary": "Useful capability",
                "tags": [],
                "definition_id": "definition",
                "descriptor_lifecycle": lifecycle,
                "instance_id": null,
                "provider": "provider",
                "source": "test",
                "execution_locality": "local workspace",
                "authority": "current session",
                "lifetime": "connection",
                "surface": "workspace",
                "availability": "available",
                "applicability": {"allowed_roles": [], "required_scope": "workspace", "policy": "test"},
                "risk": "mutating",
                "good_for": [],
                "not_good_for": [],
                "execution_options": [],
                "relationships": [],
                "replacement": null,
                "code_mode_compatibility": "requires_mediation"
            }))
            .expect("valid capability entry")
        };
        let mut recent = Vec::new();
        retain_recent_capability_entries(&mut recent, [entry("tool:a", "durable_definition")]);
        retain_recent_capability_entries(&mut recent, [entry("tool:a", "session_instance")]);
        for index in 0..RECENT_CAPABILITY_LIMIT {
            retain_recent_capability_entries(
                &mut recent,
                [entry(&format!("tool:{index}"), "durable_definition")],
            );
        }

        assert_eq!(recent.len(), RECENT_CAPABILITY_LIMIT);
        assert!(!recent.iter().any(|entry| entry.entry.r#ref == "tool:a"));
        let rendered = render_recently_discovered_capabilities(&[RecentCapability {
            entry: entry("capability-instance:local", "session_instance"),
        }]);
        assert!(rendered.contains("Risk: mutating; scope: workspace"));
        assert!(rendered.contains("Locality: local workspace"));
    }

    #[test]
    fn extracts_only_capability_discovery_results() {
        let result = serde_json::json!({"results": [{
            "ref": "tool:a", "kind": "tool", "summary": "A", "tags": [],
            "definition_id": "definition", "descriptor_lifecycle": "durable_definition",
            "instance_id": null,
            "provider": "den", "source": "test", "execution_locality": "den",
            "authority": "policy", "lifetime": "durable", "surface": "den",
            "availability": "available", "applicability": {"allowed_roles": [], "required_scope": "den", "policy": "test"},
            "risk": "read_only", "good_for": [], "not_good_for": [],
            "execution_options": [], "relationships": [], "replacement": null,
            "code_mode_compatibility": "supported"
        }]});
        assert_eq!(
            discovered_capability_entries("capability_search", &result).len(),
            1
        );
        assert!(discovered_capability_entries("session_info", &result).is_empty());
    }

    #[test]
    fn runtime_snapshot_uses_orientation_task_without_plan() {
        let session = test_session(ObjectiveOrientation::Oriented {
            task: crate::agent_loop::TaskOrientation {
                task_ref: crate::agent_loop::OrientationTaskRef::DocketTask {
                    job_id: Some("job-123".to_string()),
                    task_id: "task-456".to_string(),
                    title: Some("Ship the smallest slice".to_string()),
                },
                child_policy: Default::default(),
            },
        });

        let snapshot = session.session_info_runtime_snapshot();

        assert_eq!(snapshot["task_focus"]["active"], true);
        assert_eq!(
            snapshot["task_focus"]["next_incomplete_task_title"],
            "Ship the smallest slice"
        );
        assert_eq!(snapshot["docket"]["active_task_id"], "task-456");
        assert_eq!(
            snapshot["docket"]["active_task_title"],
            "Ship the smallest slice"
        );
        assert_eq!(snapshot["docket"]["source"], "objective_orientation");
    }
}
