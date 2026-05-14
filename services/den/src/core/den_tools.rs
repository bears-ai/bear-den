use std::net::{IpAddr, ToSocketAddrs};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    config::Config,
    core::{
        acp_plan_mode::{self, AcpPlanModeRequestedBy, EnterPlanModeParams, SubmitPlanModeParams},
        acp_sessions,
        bears::{db as bears_db, db::role_is_bear_admin, BearAgentRole},
        memory_manager_head::{
            fetch_memfs_role_memory_file, fetch_memfs_role_memory_status,
            fetch_memfs_role_memory_tree, fetch_memfs_role_plan_artifacts,
            search_memfs_role_memory, write_memfs_core_update, write_memfs_role_memory_entry,
            MemfsCoreUpdateRequest, MemfsWriteRoleMemoryEntryRequest,
        },
        memory_proposals::{self, CreateMemoryProposal},
        turn_state, user, web_policy,
        work_plans::{
            self, WorkPlanListFilter, WorkPlanLookup, WorkPlanStatus, WorkPlanUpdate,
            WorkPlanUpsert, WorkPlanVisibility,
        },
    },
    errors::CustomError,
};

pub(crate) fn plan_mode_workplan_payload(row: &acp_plan_mode::AcpPlanModeSessionRow) -> Value {
    turn_state::turn_state_from_sources(
        &crate::core::acp_tools::AcpResolvedSessionPolicy {
            mode_label: if row.state == "approved" { "Write" } else { "Plan" },
            tool_enablement: if row.state == "approved" {
                crate::core::acp_tools::AcpToolEnablementState::AllTools
            } else {
                crate::core::acp_tools::AcpToolEnablementState::ReadOnly
            },
            plan_mode_state: Some(row.state.clone()),
        },
        Some(row),
        None,
    )["workplan"]
        .clone()
}

pub(crate) fn no_active_workplan_payload() -> Value {
    json!({
        "domain": "workplan",
        "plan_id": Value::Null,
        "id": Value::Null,
        "state": "inactive",
        "approval_status": "inactive",
        "raw_state": Value::Null,
        "submitted_plan_present": false,
        "artifact_path": Value::Null,
        "title": Value::Null,
        "summary": Value::Null,
        "execution_unlocked": false,
    })
}

pub(crate) fn activity_payload(plan: Option<&work_plans::WorkPlanProjection>) -> Value {
    match plan {
        Some(plan) => json!({
            "domain": "activity",
            "plan_id": plan.id,
            "id": plan.id,
            "status": plan.status.clone(),
            "title": plan.title.clone(),
            "summary": plan.summary.clone(),
            "current_item": plan.current_item.clone(),
            "items": plan.items.clone(),
            "visibility": plan.visibility.clone(),
            "owner_role": plan.owner_role.clone(),
            "version": plan.version,
            "handoff_requested": plan.handoff_intent_path.is_some() || plan.handoff_task_id.is_some(),
            "handoff_intent_path": plan.handoff_intent_path.clone(),
            "handoff_task_id": plan.handoff_task_id.clone(),
            "updated_at": plan.updated_at,
        }),
        None => json!({
            "domain": "activity",
            "plan_id": Value::Null,
            "id": Value::Null,
            "status": "inactive",
            "title": Value::Null,
            "summary": Value::Null,
            "current_item": Value::Null,
            "items": [],
            "handoff_requested": false,
        }),
    }
}

// Den-executed server tools. Adding a new Den tool here and to
// `builtin_den_tool_descriptors` should not require an ACP adapter update when
// it uses existing stream/result shapes. Keep provider names semantic and
// provider-safe; accept legacy aliases only at routing boundaries.
pub const DEN_BEAR_GET_SELF: &str = "den.bear.get_self";
pub const DEN_USER_GET_CURRENT: &str = "den.user.get_current";
pub const DEN_BEAR_LIST_MEMBERS: &str = "den.bear.list_members";
pub const DEN_CAPABILITIES_LIST_SELF: &str = "den.capabilities.list_self";
pub const DEN_CHANNEL_GET_CONTEXT: &str = "den.channel.get_context";
pub const DEN_POLICY_GET_SELF: &str = "den.policy.get_self";
pub const DEN_CONVERSATION_SET_TITLE: &str = "den.conversation.set_title";
pub const DEN_CONVERSATION_SET_TITLE_PROVIDER: &str = "set_conversation_title";
pub const DEN_WEB_FETCH: &str = "den.web.fetch";
pub const DEN_WEB_FETCH_PROVIDER: &str = "web_fetch";
pub const DEN_WEB_FETCH_LEGACY_PROVIDER: &str = "den_web_fetch";
pub const DEN_WEB_SEARCH: &str = "den.web.search";
pub const DEN_WEB_SEARCH_PROVIDER: &str = "web_search";
pub const DEN_SITUATION_GET: &str = "den.session.info";
pub const DEN_SITUATION_GET_PROVIDER: &str = "session_info";
pub const DEN_SITUATION_GET_LEGACY_PROVIDER: &str = "situation_get";
pub const DEN_MEMORY_WRITE_ENTRY: &str = "den.memory.write_entry";
pub const DEN_MEMORY_WRITE_ENTRY_PROVIDER: &str = "memory_write_entry";
pub const DEN_MEMORY_STATUS: &str = "den.memory.status";
pub const DEN_MEMORY_STATUS_PROVIDER: &str = "memory_status";
pub const DEN_MEMORY_TREE: &str = "den.memory.browse";
pub const DEN_MEMORY_TREE_PROVIDER: &str = "memory_browse";
pub const DEN_MEMORY_TREE_LEGACY_PROVIDER: &str = "memory_tree";
pub const DEN_MEMORY_READ: &str = "den.memory.read";
pub const DEN_MEMORY_READ_PROVIDER: &str = "memory_read";
pub const DEN_MEMORY_SEARCH: &str = "den.memory.search";
pub const DEN_MEMORY_SEARCH_PROVIDER: &str = "memory_search";
pub const DEN_MEMORY_REQUEST_REVIEW: &str = "den.memory.request_review";
pub const DEN_MEMORY_REQUEST_REVIEW_PROVIDER: &str = "memory_request_review";
pub const DEN_MEMORY_LIST_PROPOSALS: &str = "den.memory.list_proposals";
pub const DEN_MEMORY_LIST_PROPOSALS_PROVIDER: &str = "memory_list_proposals";
pub const DEN_MEMORY_READ_PROPOSAL: &str = "den.memory.read_proposal";
pub const DEN_MEMORY_READ_PROPOSAL_PROVIDER: &str = "memory_read_proposal";
pub const DEN_MEMORY_RESOLVE_PROPOSAL: &str = "den.memory.resolve_proposal";
pub const DEN_MEMORY_RESOLVE_PROPOSAL_PROVIDER: &str = "memory_resolve_proposal";
pub const DEN_MEMORY_APPLY_CORE_UPDATE: &str = "den.memory.apply_core_update";
pub const DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER: &str = "memory_apply_core_update";
pub const DEN_SKILL_PROPOSE: &str = "den.skill.propose";
pub const DEN_SKILL_APPROVE_PROPOSAL: &str = "den.skill.approve_proposal";
pub const DEN_SKILL_REJECT_PROPOSAL: &str = "den.skill.reject_proposal";
pub const DEN_WORK_PLAN_LIST: &str = "den.work_plan.list";
pub const DEN_WORK_PLAN_LIST_PROVIDER: &str = "list_plans";
pub const DEN_WORK_PLAN_GET_STATUS: &str = "den.work_plan.get_status";
pub const DEN_WORK_PLAN_GET_STATUS_PROVIDER: &str = "get_plan_status";
pub const DEN_WORK_PLAN_UPDATE: &str = "den.work_plan.update";
pub const DEN_WORK_PLAN_UPDATE_PROVIDER: &str = "update_plan";
pub const DEN_WORK_PLAN_REQUEST_HANDOFF: &str = "den.work_plan.request_handoff";
pub const DEN_WORK_PLAN_REQUEST_HANDOFF_PROVIDER: &str = "request_work_handoff";
pub const DEN_PLAN_MODE_ENTER: &str = "den.plan_mode.enter";
pub const DEN_PLAN_MODE_ENTER_PROVIDER: &str = "enter_plan_mode";
pub const DEN_PLAN_MODE_STATUS: &str = "den.plan_mode.status";
pub const DEN_PLAN_MODE_STATUS_PROVIDER: &str = "get_plan_mode_status";
pub const DEN_PLAN_MODE_RECORD_APPROVAL: &str = "den.plan_mode.record_approval";
pub const DEN_PLAN_MODE_RECORD_APPROVAL_PROVIDER: &str = "record_plan_approval";
pub const DEN_PLAN_MODE_EXIT: &str = "den.plan_mode.exit";
pub const DEN_PLAN_MODE_EXIT_PROVIDER: &str = "exit_plan_mode";
pub const DEN_PLAN_MODE_CANCEL: &str = "den.plan_mode.cancel";
pub const DEN_PLAN_MODE_CANCEL_PROVIDER: &str = "cancel_plan_mode";
pub const DEN_TASK_WRITE_INTENT: &str = "den.task.write_intent";
pub const DEN_TASK_APPROVE_INTENT: &str = "den.task.approve_intent";
pub const DEN_TASK_REJECT_INTENT: &str = "den.task.reject_intent";
pub const DEN_CORE_WRITE_RESULT_SUMMARY: &str = "den.core.write_result_summary";
pub const DEN_OBSERVATION_WRITE: &str = "den.observation.write";
pub const DEN_RUN_WRITE_RESULT: &str = "den.run.write_result";

const ALL_ROLES: &[&str] = &["talk", "pair", "curate", "work", "watch"];
const WORK_PLAN_READ_ROLES: &[&str] = &["talk", "pair", "curate", "work"];
const WORK_PLAN_UPDATE_ROLES: &[&str] = &["talk", "pair", "work"];
const TALK_AND_PAIR_ROLES: &[&str] = &["talk", "pair"];
const PAIR_ROLES: &[&str] = &["pair"];
const PAIR_AND_CURATE_ROLES: &[&str] = &["pair", "curate"];
const CURATE_ROLES: &[&str] = &["curate"];
const WATCH_ROLES: &[&str] = &["watch"];
const WORK_ROLES: &[&str] = &["work"];

pub fn provider_safe_tool_name(name: &str) -> String {
    // Prefer concise semantic aliases (`web_search`, `session_info`) for Den
    // server tools. Do not expose `den_*` just to communicate execution
    // location; execution belongs in descriptor metadata and docs.
    match name {
        DEN_CONVERSATION_SET_TITLE => return DEN_CONVERSATION_SET_TITLE_PROVIDER.to_string(),
        DEN_WEB_FETCH => return DEN_WEB_FETCH_PROVIDER.to_string(),
        DEN_WEB_SEARCH => return DEN_WEB_SEARCH_PROVIDER.to_string(),
        DEN_SITUATION_GET => return DEN_SITUATION_GET_PROVIDER.to_string(),
        DEN_MEMORY_WRITE_ENTRY => return DEN_MEMORY_WRITE_ENTRY_PROVIDER.to_string(),
        DEN_MEMORY_STATUS => return DEN_MEMORY_STATUS_PROVIDER.to_string(),
        DEN_MEMORY_TREE => return DEN_MEMORY_TREE_PROVIDER.to_string(),
        DEN_MEMORY_READ => return DEN_MEMORY_READ_PROVIDER.to_string(),
        DEN_MEMORY_SEARCH => return DEN_MEMORY_SEARCH_PROVIDER.to_string(),
        DEN_MEMORY_REQUEST_REVIEW => return DEN_MEMORY_REQUEST_REVIEW_PROVIDER.to_string(),
        DEN_MEMORY_LIST_PROPOSALS => return DEN_MEMORY_LIST_PROPOSALS_PROVIDER.to_string(),
        DEN_MEMORY_READ_PROPOSAL => return DEN_MEMORY_READ_PROPOSAL_PROVIDER.to_string(),
        DEN_MEMORY_RESOLVE_PROPOSAL => return DEN_MEMORY_RESOLVE_PROPOSAL_PROVIDER.to_string(),
        DEN_MEMORY_APPLY_CORE_UPDATE => return DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER.to_string(),
        DEN_WORK_PLAN_LIST => return DEN_WORK_PLAN_LIST_PROVIDER.to_string(),
        DEN_WORK_PLAN_GET_STATUS => return DEN_WORK_PLAN_GET_STATUS_PROVIDER.to_string(),
        DEN_WORK_PLAN_UPDATE => return DEN_WORK_PLAN_UPDATE_PROVIDER.to_string(),
        DEN_WORK_PLAN_REQUEST_HANDOFF => return DEN_WORK_PLAN_REQUEST_HANDOFF_PROVIDER.to_string(),
        DEN_PLAN_MODE_ENTER => return DEN_PLAN_MODE_ENTER_PROVIDER.to_string(),
        DEN_PLAN_MODE_STATUS => return DEN_PLAN_MODE_STATUS_PROVIDER.to_string(),
        DEN_PLAN_MODE_RECORD_APPROVAL => return DEN_PLAN_MODE_RECORD_APPROVAL_PROVIDER.to_string(),
        DEN_PLAN_MODE_EXIT => return DEN_PLAN_MODE_EXIT_PROVIDER.to_string(),
        DEN_PLAN_MODE_CANCEL => return DEN_PLAN_MODE_CANCEL_PROVIDER.to_string(),
        _ => {}
    }
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "den_tool".to_string()
    } else {
        safe
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DenToolDescriptor {
    /// Canonical Den capability/tool name used for policy and invocation.
    pub name: &'static str,
    /// Provider/API-safe alias exposed to LLM tool registries.
    pub provider_name: String,
    /// Legacy or alternate provider aliases accepted at routing boundaries.
    pub provider_aliases: &'static [&'static str],
    pub label: &'static str,
    pub description: &'static str,
    pub kind: &'static str,
    pub provider: &'static str,
    pub execution_target: &'static str,
    pub scope: &'static str,
    pub domain: &'static str,
    pub content_class: Option<&'static str>,
    pub availability: &'static str,
    pub permissions: &'static [&'static str],
    pub allowed_roles: &'static [&'static str],
    pub approval_policy: &'static str,
    pub input_schema: Value,
}

pub fn builtin_den_tool_descriptors() -> Vec<DenToolDescriptor> {
    // Den-executed tools are safe to add without adapter updates as long as
    // they do not introduce new required adapter-facing event/result shapes.
    // If a tool needs adapter-local execution, it belongs in acp_tools.rs and
    // must be direct-tool capability gated instead.
    vec![
        descriptor(
            DEN_BEAR_GET_SELF,
            "About this bear",
            "Return Den's trusted profile for the current bear.",
            "bear",
            &["bear.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_USER_GET_CURRENT,
            "Current user",
            "Return Den's trusted profile for the current user in this interaction.",
            "session",
            &["user.current.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_BEAR_LIST_MEMBERS,
            "Bear members",
            "List users who have access to the current bear, with policy redaction.",
            "bear",
            &["bear.members.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_CAPABILITIES_LIST_SELF,
            "Available Den capabilities",
            "List Den-managed tools available to the current bear/session.",
            "session",
            &["capabilities.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_CHANNEL_GET_CONTEXT,
            "Channel context",
            "Return trusted Den/Codepool channel and session context for this interaction.",
            "session",
            &["channel.context.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_POLICY_GET_SELF,
            "Current policy",
            "Explain current user and bear policy for this interaction.",
            "session",
            &["policy.read"],
            ALL_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_CONVERSATION_SET_TITLE,
            "Set conversation title",
            "Set the title of the current conversation. In some clients this may appear as the current chat or thread title. Does not change the conversation id, switch conversations, or write Bear memory.",
            "conversation",
            &["conversation.title.write"],
            TALK_AND_PAIR_ROLES,
            set_conversation_title_schema(),
        ),
        descriptor(
            DEN_WEB_FETCH,
            "Fetch web page",
            "Fetch an HTTP(S) URL through Den with SSRF guards and return a bounded text excerpt.",
            "web",
            &["web.fetch"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "HTTP or HTTPS URL to fetch." },
                    "max_chars": { "type": "integer", "minimum": 1, "maximum": 20000, "description": "Maximum characters of extracted text to return. Defaults to 8000." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WEB_SEARCH,
            "Search web",
            "Search the web through a configured Den search provider. Returns a clear configuration error when no provider is configured.",
            "web",
            &["web.search"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 10 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_SITUATION_GET,
            "Session info",
            "Return trusted session information for this interaction: authenticated human, bear, role, session identity, memory scopes, policy notes, and memory health hints.",
            "session",
            &["situation.read"],
            PAIR_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_WRITE_ENTRY,
            "Write memory entry",
            "Write a role-local semantic memory entry such as a note, log, decision, reflection, scratch item, or summary. Do not use for active plans or task lists; use update_plan and plan-mode tools instead. Does not write core, Cabinet, tasks, observations, or run results.",
            "bear.memory",
            &["memory.entry.write"],
            PAIR_ROLES,
            memory_write_entry_schema(),
        ),
        descriptor(
            DEN_MEMORY_STATUS,
            "Memory status",
            "Return MemFS memory health and entry counts for the current bear role.",
            "bear.memory",
            &["memory.status.read"],
            PAIR_AND_CURATE_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_TREE,
            "Browse memory",
            "Browse allowed Bear memory paths for the current role.",
            "bear.memory",
            &["memory.tree.read"],
            PAIR_AND_CURATE_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_READ,
            "Read memory file",
            "Read an allowed Bear memory file for the current role.",
            "bear.memory",
            &["memory.file.read"],
            PAIR_AND_CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Allowed memory path, for example pair/notes/mem_abc.md or core/missions.md." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_MEMORY_SEARCH,
            "Search memory",
            "Search allowed Bear memory files for the current role.",
            "bear.memory",
            &["memory.search"],
            PAIR_AND_CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_MEMORY_REQUEST_REVIEW,
            "Request memory review",
            "Request Reflection/curate review of role-local memory without writing shared memory directly.",
            "bear.memory",
            &["memory.review.request"],
            PAIR_ROLES,
            memory_request_review_schema(),
        ),
        descriptor(
            DEN_MEMORY_LIST_PROPOSALS,
            "List memory proposals",
            "List memory review proposals for this Bear.",
            "bear.memory",
            &["memory.proposal.read"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_MEMORY_READ_PROPOSAL,
            "Read memory proposal",
            "Read one memory review proposal with source pointers and status.",
            "bear.memory",
            &["memory.proposal.read"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string", "format": "uuid" }
                },
                "required": ["proposal_id"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_MEMORY_RESOLVE_PROPOSAL,
            "Resolve memory proposal",
            "Resolve a memory review proposal without applying shared-memory writes.",
            "bear.memory",
            &["memory.proposal.resolve"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string", "format": "uuid" },
                    "status": { "enum": ["rejected", "retained_local", "deferred", "superseded", "needs_human_review"] },
                    "review_notes": { "type": "string" },
                    "decision_summary": { "type": "string" }
                },
                "required": ["proposal_id", "status"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_MEMORY_APPLY_CORE_UPDATE,
            "Apply core memory update",
            "Apply a reviewed update to allowed core memory paths with provenance.",
            "bear.memory",
            &["memory.core.write"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string", "format": "uuid" },
                    "target_path": { "type": "string" },
                    "mode": { "enum": ["append_section", "create_file", "replace_text"] },
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" },
                    "review_notes": { "type": "string" }
                },
                "required": ["proposal_id", "target_path", "mode"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_SKILL_PROPOSE,
            "Propose skill",
            "Capture a durable skill proposal for curate review without installing it directly.",
            "bear.skills",
            &["skill.proposal.write"],
            ALL_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "skill_name": { "type": "string" },
                    "skill_version": { "type": "string" },
                    "rationale": { "type": "string" },
                    "proposed_content": { "type": "string" },
                    "desired_roles": {
                        "type": "array",
                        "items": { "enum": ALL_ROLES }
                    },
                    "provenance": { "type": "object" }
                },
                "required": ["skill_name", "rationale", "proposed_content"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_SKILL_APPROVE_PROPOSAL,
            "Approve skill proposal",
            "Approve a pending skill proposal, update the manifest, and queue reconciliation for affected roles.",
            "bear.skills",
            &["skill.proposal.approve"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string", "format": "uuid" },
                    "skill_name": { "type": "string" },
                    "skill_version": { "type": "string" },
                    "applies_to_roles": {
                        "type": "array",
                        "items": { "enum": ALL_ROLES },
                        "minItems": 1
                    },
                    "review_notes": { "type": "string" }
                },
                "required": ["proposal_id", "applies_to_roles"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_SKILL_REJECT_PROPOSAL,
            "Reject skill proposal",
            "Reject a pending skill proposal with reviewer metadata and a rejection reason.",
            "bear.skills",
            &["skill.proposal.reject"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string", "format": "uuid" },
                    "rejection_reason": { "type": "string" },
                    "review_notes": { "type": "string" }
                },
                "required": ["proposal_id", "rejection_reason"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WORK_PLAN_LIST,
            "List plans",
            "List visible Bear-level planning state, including live activity plans, submitted workplan gates, and saved workplan artifacts where available.",
            "bear.activity",
            &["work_plan.read"],
            WORK_PLAN_READ_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "array",
                        "items": { "enum": ["active", "blocked", "completed", "cancelled", "archived"] }
                    },
                    "owner_role": { "enum": ALL_ROLES },
                    "include_archived": { "type": "boolean" },
                    "include_completed": { "type": "boolean" },
                    "include_plan_mode": { "type": "boolean" },
                    "include_artifacts": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WORK_PLAN_GET_STATUS,
            "Get work plan status",
            "Return current status for one visible Den activity plan or this session's active plan.",
            "bear.activity",
            &["work_plan.read"],
            WORK_PLAN_READ_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_id": { "type": "string", "format": "uuid" },
                    "source_acp_session_id": { "type": "string" },
                    "source_conversation_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WORK_PLAN_UPDATE,
            "Update visible plan",
            "Create or update the current role's live visible ACP activity plan. Use this when the user asks to create, show, update, or execute a plan/task list.",
            "bear.activity",
            &["work_plan.write"],
            WORK_PLAN_UPDATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_id": { "type": "string", "format": "uuid" },
                    "expected_version": { "type": "integer", "minimum": 1 },
                    "title": { "type": "string" },
                    "summary": { "type": "string" },
                    "visibility": { "enum": ["private_to_role", "same_user", "bear_visible", "handoff_requested"] },
                    "status": { "enum": ["active", "blocked", "completed", "cancelled", "archived"] },
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "title": { "type": "string" },
                                "summary": { "type": "string" },
                                "status": { "enum": ["pending", "in_progress", "blocked", "completed", "cancelled"] },
                                "blocked_reason": { "type": "string" },
                                "source_refs": { "type": "array", "items": { "type": "string" } }
                            },
                            "required": ["id", "title", "status"],
                            "additionalProperties": false
                        }
                    },
                    "workspace_context": { "type": "object" }
                },
                "required": ["title", "visibility", "status", "items"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WORK_PLAN_REQUEST_HANDOFF,
            "Request task handoff",
            "Request conversion of selected live activity plan items into a schema-validated task intent for curate review.",
            "bear.activity",
            &["work_plan.handoff.request"],
            TALK_AND_PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_id": { "type": "string", "format": "uuid" },
                    "item_ids": { "type": "array", "items": { "type": "string" } },
                    "title": { "type": "string" },
                    "summary": { "type": "string" },
                    "requested_outcome": { "type": "string" },
                    "constraints": { "type": "array", "items": { "type": "string" } },
                    "allowed_tools_hint": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["plan_id", "item_ids", "title", "summary", "requested_outcome"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_PLAN_MODE_ENTER,
            "Enter planning mode",
            "Enter ACP pair workplan mode and reflect that mode in the ACP session UI. Use this when the user asks to enter planning mode.",
            "bear.workplan",
            &["plan_mode.enter"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string" },
                    "previous_permission_mode": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_PLAN_MODE_STATUS,
            "Get plan mode status",
            "Return the current ACP pair workplan gate for this session, if any.",
            "bear.workplan",
            &["plan_mode.read"],
            PAIR_ROLES,
            empty_schema(),
        ),
        descriptor(
            DEN_PLAN_MODE_RECORD_APPROVAL,
            "Record plan approval",
            "Record explicit approval from the authenticated human for the currently submitted implementation workplan. Use only when the user clearly approves the current plan in this conversation, for example 'go ahead', 'approved', or 'proceed'.",
            "bear.workplan",
            &["plan_mode.approve"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_mode_id": { "type": "string", "format": "uuid" },
                    "approval_text": { "type": "string", "description": "The user's approval text that prompted this tool call." }
                },
                "required": ["approval_text"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_PLAN_MODE_EXIT,
            "Submit implementation plan",
            "Submit a markdown implementation workplan artifact for user approval. This is for durable implementation workplans, not for the live visible task list; use update_plan for visible activity planning.",
            "bear.workplan",
            &["plan_mode.exit"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_mode_id": { "type": "string", "format": "uuid" },
                    "title": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["title", "body"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_PLAN_MODE_CANCEL,
            "Cancel plan mode",
            "Cancel the current ACP pair workplan gate without approving implementation.",
            "bear.workplan",
            &["plan_mode.cancel"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "plan_mode_id": { "type": "string", "format": "uuid" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_TASK_WRITE_INTENT,
            "Write task intent",
            "Write a schema-validated task intent from talk or pair for later curate review.",
            "bear.tasks",
            &["task.intent.write"],
            TALK_AND_PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "summary": { "type": "string" },
                    "requested_outcome": { "type": "string" },
                    "constraints": { "type": "array", "items": { "type": "string" } },
                    "allowed_tools_hint": { "type": "array", "items": { "type": "string" } },
                    "source_reference": { "type": "object" }
                },
                "required": ["title", "summary", "requested_outcome"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_TASK_APPROVE_INTENT,
            "Approve task intent",
            "Approve a talk/pair task intent, write the canonical core task, and update source intent audit metadata.",
            "bear.tasks",
            &["task.intent.approve"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "source_intent_path": { "type": "string" },
                    "task_id": { "type": "string" },
                    "title": { "type": "string" },
                    "approved_scope": { "type": "object" },
                    "allowed_tools": { "type": "array", "items": { "type": "string" } },
                    "expires_at": { "type": "string" },
                    "review_notes": { "type": "string" }
                },
                "required": ["source_intent_path", "task_id", "title", "approved_scope", "allowed_tools"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_TASK_REJECT_INTENT,
            "Reject task intent",
            "Reject a talk/pair task intent and update source intent audit metadata with the rejection reason.",
            "bear.tasks",
            &["task.intent.reject"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "source_intent_path": { "type": "string" },
                    "rejection_reason": { "type": "string" },
                    "review_notes": { "type": "string" }
                },
                "required": ["source_intent_path", "rejection_reason"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_CORE_WRITE_RESULT_SUMMARY,
            "Write core result summary",
            "Write a curate-reviewed summary of work results into shared core memory through Den-controlled validation.",
            "bear.core",
            &["core.result_summary.write"],
            CURATE_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "run_id": { "type": "string" },
                    "summary": { "type": "string" },
                    "durable_learnings": { "type": "array", "items": { "type": "string" } },
                    "source_result_path": { "type": "string" }
                },
                "required": ["task_id", "summary"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_OBSERVATION_WRITE,
            "Write observation",
            "Write a schema-validated inbound observation from a Den-delivered watch event.",
            "bear.observations",
            &["observation.write"],
            WATCH_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "observation_id": { "type": "string" },
                    "summary": { "type": "string" },
                    "salience": { "type": "string" },
                    "payload_ref": { "type": "string" },
                    "source": { "type": "object" }
                },
                "required": ["summary"],
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_RUN_WRITE_RESULT,
            "Write run result",
            "Write a schema-validated work run result under the active Den-issued run context.",
            "bear.runs",
            &["run.result.write"],
            WORK_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "run_id": { "type": "string" },
                    "status": { "enum": ["succeeded", "failed", "partial"] },
                    "summary": { "type": "string" },
                    "result": { "type": "object" },
                    "follow_up": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["task_id", "run_id", "status", "summary"],
                "additionalProperties": false
            }),
        ),
    ]
}

pub fn builtin_den_tool_descriptors_for_role(role: BearAgentRole) -> Vec<DenToolDescriptor> {
    builtin_den_tool_descriptors()
        .into_iter()
        .filter(|descriptor| descriptor.allows_role(role))
        .collect()
}

fn descriptor(
    name: &'static str,
    label: &'static str,
    description: &'static str,
    scope: &'static str,
    permissions: &'static [&'static str],
    allowed_roles: &'static [&'static str],
    input_schema: Value,
) -> DenToolDescriptor {
    DenToolDescriptor {
        name,
        provider_name: provider_safe_tool_name(name),
        provider_aliases: provider_aliases_for_tool(name),
        label,
        description,
        kind: "server_tool",
        provider: "den",
        execution_target: "den",
        scope,
        domain: tool_domain(name),
        content_class: tool_content_class(name),
        availability: "available",
        permissions,
        allowed_roles,
        approval_policy: "never",
        input_schema,
    }
}

fn tool_domain(name: &str) -> &'static str {
    match name {
        DEN_PLAN_MODE_ENTER
        | DEN_PLAN_MODE_STATUS
        | DEN_PLAN_MODE_RECORD_APPROVAL
        | DEN_PLAN_MODE_EXIT
        | DEN_PLAN_MODE_CANCEL => "workplan",
        DEN_WORK_PLAN_LIST
        | DEN_WORK_PLAN_GET_STATUS
        | DEN_WORK_PLAN_UPDATE
        | DEN_WORK_PLAN_REQUEST_HANDOFF => "activity",
        DEN_MEMORY_WRITE_ENTRY
        | DEN_MEMORY_STATUS
        | DEN_MEMORY_TREE
        | DEN_MEMORY_READ
        | DEN_MEMORY_SEARCH
        | DEN_MEMORY_REQUEST_REVIEW
        | DEN_MEMORY_LIST_PROPOSALS
        | DEN_MEMORY_READ_PROPOSAL
        | DEN_MEMORY_RESOLVE_PROPOSAL
        | DEN_MEMORY_APPLY_CORE_UPDATE => "memory",
        DEN_CONVERSATION_SET_TITLE | DEN_WEB_FETCH | DEN_WEB_SEARCH | DEN_SITUATION_GET => {
            "execution"
        }
        _ => "execution",
    }
}

fn tool_content_class(name: &str) -> Option<&'static str> {
    match name {
        DEN_MEMORY_WRITE_ENTRY => Some("semantic_memory"),
        DEN_PLAN_MODE_EXIT => Some("workplan_artifact"),
        DEN_WORK_PLAN_UPDATE => Some("activity_status"),
        DEN_WORK_PLAN_REQUEST_HANDOFF => Some("task_intent"),
        DEN_MEMORY_APPLY_CORE_UPDATE => Some("core_update"),
        DEN_OBSERVATION_WRITE => Some("observation"),
        DEN_RUN_WRITE_RESULT => Some("run_result"),
        _ => None,
    }
}

fn memory_request_review_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source_paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20 },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "summary": { "type": "string", "minLength": 1, "maxLength": 4000 },
            "rationale": { "type": "string", "maxLength": 4000 },
            "suggested_action": { "type": "string", "enum": ["unspecified", "summarize_into_core", "promote_to_core", "cabinet_update", "skill_review", "retain_role_local", "delete_after_review", "human_review", "archive_index", "task_context"] },
            "target_ref": { "type": "string", "maxLength": 500 },
            "refs": { "type": "object" },
            "sensitivity": { "type": "string", "enum": ["normal", "person", "secret_risk", "external_untrusted", "unknown"] },
            "requires_human": { "type": "boolean" },
            "proposed_content": { "type": "string", "maxLength": 20000 },
            "proposed_patch": { "type": "string", "maxLength": 20000 }
        },
        "required": ["source_paths", "title", "summary"],
        "additionalProperties": false
    })
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn set_conversation_title_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "minLength": 1,
                "maxLength": 120,
                "description": "New title for the current conversation."
            }
        },
        "required": ["title"],
        "additionalProperties": false
    })
}

fn memory_write_entry_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["note", "log", "decision", "reflection", "scratch", "summary", "plan"]
            },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "body": { "type": "string", "minLength": 1, "maxLength": 50000 },
            "tags": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "maxLength": 80 },
                "maxItems": 20
            },
            "refs": {
                "type": "object",
                "properties": {
                    "people": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "missions": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "knowledge": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "cabinet": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "artifacts": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "tasks": { "type": "array", "items": { "type": "string" }, "maxItems": 20 }
                },
                "additionalProperties": false
            },
            "lifecycle": {
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["role-local", "core-candidate", "cabinet-candidate"] },
                    "retention": { "type": "string", "enum": ["session", "short", "durable", "archive"] },
                    "promotion": { "type": "string", "enum": ["none", "maybe", "proposed"] },
                    "status": { "type": "string", "enum": ["active", "superseded", "stale", "archived"] }
                },
                "additionalProperties": false
            },
            "source": { "type": "object" },
            "content_class": {
                "type": "string",
                "enum": ["semantic_memory", "workplan_artifact", "activity_status", "task_intent", "run_result", "observation", "core_update", "cabinet_write"]
            },
            "domain": {
                "type": "string",
                "enum": ["workplan", "activity", "memory", "execution"]
            }
        },
        "required": ["kind", "title", "body"],
        "additionalProperties": false
    })
}

impl DenToolDescriptor {
    pub fn allows_role(&self, role: BearAgentRole) -> bool {
        self.allowed_roles.contains(&role.as_str())
    }

    pub fn matches_provider_name(&self, provider_name: &str) -> bool {
        self.provider_name == provider_name || self.provider_aliases.contains(&provider_name)
    }
}

pub fn provider_aliases_for_tool(name: &str) -> &'static [&'static str] {
    match name {
        DEN_WEB_FETCH => &[DEN_WEB_FETCH_LEGACY_PROVIDER],
        DEN_WEB_SEARCH => &["den_web_search"],
        DEN_CONVERSATION_SET_TITLE => &[
            "set_thread_title",
            "rename_conversation",
            "rename_thread",
            "conversation_rename",
        ],
        DEN_PLAN_MODE_RECORD_APPROVAL => &["approve_plan", "approve_current_plan"],
        DEN_SITUATION_GET => &[DEN_SITUATION_GET_LEGACY_PROVIDER, "den_situation_get"],
        DEN_MEMORY_WRITE_ENTRY => &["den_memory_write_entry"],
        DEN_MEMORY_STATUS => &["den_memory_status"],
        DEN_MEMORY_TREE => &[DEN_MEMORY_TREE_LEGACY_PROVIDER, "den_memory_tree"],
        DEN_MEMORY_READ => &["den_memory_read"],
        DEN_MEMORY_SEARCH => &["den_memory_search"],
        DEN_MEMORY_REQUEST_REVIEW => &["den_memory_request_review"],
        DEN_MEMORY_LIST_PROPOSALS => &["den_memory_list_proposals"],
        DEN_MEMORY_READ_PROPOSAL => &["den_memory_read_proposal"],
        DEN_MEMORY_RESOLVE_PROPOSAL => &["den_memory_resolve_proposal"],
        DEN_MEMORY_APPLY_CORE_UPDATE => &["den_memory_apply_core_update"],
        _ => &[],
    }
}

pub fn builtin_den_tool_descriptor_for_provider_name(
    provider_name: &str,
) -> Option<DenToolDescriptor> {
    builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.matches_provider_name(provider_name))
}

pub fn is_builtin_den_tool(name: &str) -> bool {
    matches!(
        name,
        DEN_BEAR_GET_SELF
            | DEN_USER_GET_CURRENT
            | DEN_BEAR_LIST_MEMBERS
            | DEN_CAPABILITIES_LIST_SELF
            | DEN_CHANNEL_GET_CONTEXT
            | DEN_POLICY_GET_SELF
            | DEN_CONVERSATION_SET_TITLE
            | DEN_WEB_FETCH
            | DEN_WEB_SEARCH
            | DEN_SITUATION_GET
            | DEN_MEMORY_WRITE_ENTRY
            | DEN_MEMORY_STATUS
            | DEN_MEMORY_TREE
            | DEN_MEMORY_READ
            | DEN_MEMORY_SEARCH
            | DEN_MEMORY_REQUEST_REVIEW
            | DEN_MEMORY_LIST_PROPOSALS
            | DEN_MEMORY_READ_PROPOSAL
            | DEN_MEMORY_RESOLVE_PROPOSAL
            | DEN_MEMORY_APPLY_CORE_UPDATE
            | DEN_SKILL_PROPOSE
            | DEN_SKILL_APPROVE_PROPOSAL
            | DEN_SKILL_REJECT_PROPOSAL
            | DEN_WORK_PLAN_LIST
            | DEN_WORK_PLAN_GET_STATUS
            | DEN_WORK_PLAN_UPDATE
            | DEN_WORK_PLAN_REQUEST_HANDOFF
            | DEN_PLAN_MODE_ENTER
            | DEN_PLAN_MODE_STATUS
            | DEN_PLAN_MODE_RECORD_APPROVAL
            | DEN_PLAN_MODE_EXIT
            | DEN_PLAN_MODE_CANCEL
            | DEN_TASK_WRITE_INTENT
            | DEN_TASK_APPROVE_INTENT
            | DEN_TASK_REJECT_INTENT
            | DEN_CORE_WRITE_RESULT_SUMMARY
            | DEN_OBSERVATION_WRITE
            | DEN_RUN_WRITE_RESULT
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DenToolInvocationContext {
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub role_agent_id: String,
    pub agent_role: Option<BearAgentRole>,
    pub user_id: i32,
    pub username: Option<String>,
    pub membership_role: Option<String>,
    pub conversation_id: String,
    pub session_id: String,
    #[serde(default)]
    pub acp_session_id: Option<String>,
    #[serde(default)]
    pub conversation_selection: Option<String>,
    #[serde(default)]
    pub runtime_target: Option<String>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub channel: DenToolChannelContext,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DenToolChannelContext {
    pub family: Option<String>,
    pub client: Option<String>,
    pub protocol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkPlanListArguments {
    #[serde(default, rename = "status")]
    statuses: Option<Vec<WorkPlanStatus>>,
    #[serde(default)]
    owner_role: Option<BearAgentRole>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    include_completed: bool,
    #[serde(default)]
    include_plan_mode: Option<bool>,
    #[serde(default)]
    include_artifacts: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WorkPlanGetStatusArguments {
    #[serde(default)]
    plan_id: Option<Uuid>,
    #[serde(default)]
    source_conversation_id: Option<String>,
    #[serde(default)]
    source_acp_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkPlanUpdateArguments {
    #[serde(default)]
    plan_id: Option<Uuid>,
    #[serde(default)]
    expected_version: Option<i32>,
    title: String,
    #[serde(default)]
    summary: String,
    visibility: WorkPlanVisibility,
    status: WorkPlanStatus,
    #[serde(default)]
    items: Vec<work_plans::WorkPlanItem>,
    #[serde(default = "empty_json_object")]
    workspace_context: Value,
}

#[derive(Debug, Deserialize)]
struct PlanModeEnterArguments {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    previous_permission_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlanModeRecordApprovalArguments {
    #[serde(default)]
    plan_mode_id: Option<Uuid>,
    approval_text: String,
}

#[derive(Debug, Deserialize)]
struct PlanModeExitArguments {
    #[serde(default)]
    plan_mode_id: Option<Uuid>,
    title: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct PlanModeCancelArguments {
    #[serde(default)]
    plan_mode_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct SetConversationTitleArguments {
    title: String,
}

#[derive(Debug, Deserialize)]
struct WebFetchArguments {
    url: String,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WebSearchArguments {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MemoryWriteEntryArguments {
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    refs: Option<Value>,
    #[serde(default)]
    lifecycle: Option<Value>,
    #[serde(default)]
    source: Option<Value>,
    #[serde(default)]
    content_class: Option<String>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryReadArguments {
    path: String,
}

#[derive(Debug, Deserialize)]
struct MemorySearchArguments {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryListProposalsArguments {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MemoryReadProposalArguments {
    proposal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct MemoryApplyCoreUpdateArguments {
    proposal_id: Uuid,
    target_path: String,
    mode: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    old_text: Option<String>,
    #[serde(default)]
    new_text: Option<String>,
    #[serde(default)]
    review_notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryResolveProposalArguments {
    proposal_id: Uuid,
    status: String,
    #[serde(default)]
    review_notes: Option<String>,
    #[serde(default)]
    decision_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryRequestReviewArguments {
    source_paths: Vec<String>,
    title: String,
    summary: String,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    suggested_action: Option<String>,
    #[serde(default)]
    target_ref: Option<String>,
    #[serde(default)]
    refs: Option<Value>,
    #[serde(default)]
    sensitivity: Option<String>,
    #[serde(default)]
    requires_human: bool,
    #[serde(default)]
    proposed_content: Option<String>,
    #[serde(default)]
    proposed_patch: Option<String>,
}

fn empty_json_object() -> Value {
    json!({})
}

pub async fn invoke_den_tool(
    pool: &PgPool,
    config: &Config,
    tool_name: &str,
    arguments: Value,
    context: DenToolInvocationContext,
) -> Result<Value, CustomError> {
    prevalidate_tool_arguments(tool_name, &arguments)?;
    let role = authorize_context(pool, &context).await?;
    authorize_tool_for_role(tool_name, role)?;
    match tool_name {
        DEN_BEAR_GET_SELF => get_bear_self(pool, &context).await,
        DEN_USER_GET_CURRENT => get_current_user(pool, &context).await,
        DEN_BEAR_LIST_MEMBERS => list_bear_members(pool, &context).await,
        DEN_CAPABILITIES_LIST_SELF => list_capabilities_self(pool, &context).await,
        DEN_CHANNEL_GET_CONTEXT => Ok(channel_context(&context)),
        DEN_POLICY_GET_SELF => policy_self(pool, &context).await,
        DEN_CONVERSATION_SET_TITLE => {
            set_conversation_title(pool, config, &context, arguments).await
        }
        DEN_WEB_FETCH => web_fetch(pool, &context, arguments).await,
        DEN_WEB_SEARCH => web_search(pool, config, &context, arguments).await,
        DEN_SITUATION_GET => session_info(pool, config, &context, role).await,
        DEN_MEMORY_WRITE_ENTRY => write_memory_entry(pool, config, &context, role, arguments).await,
        DEN_MEMORY_STATUS => memory_status(config, &context, role).await,
        DEN_MEMORY_TREE => memory_browse(config, &context, role).await,
        DEN_MEMORY_READ => memory_read(config, &context, role, arguments).await,
        DEN_MEMORY_SEARCH => memory_search(config, &context, role, arguments).await,
        DEN_MEMORY_REQUEST_REVIEW => request_memory_review(pool, &context, role, arguments).await,
        DEN_MEMORY_LIST_PROPOSALS => list_memory_proposals(pool, &context, role, arguments).await,
        DEN_MEMORY_READ_PROPOSAL => read_memory_proposal(pool, &context, role, arguments).await,
        DEN_MEMORY_RESOLVE_PROPOSAL => {
            resolve_memory_proposal(pool, &context, role, arguments).await
        }
        DEN_MEMORY_APPLY_CORE_UPDATE => {
            apply_core_update(pool, config, &context, role, arguments).await
        }
        DEN_WORK_PLAN_LIST => list_work_plans(pool, config, &context, role, arguments).await,
        DEN_WORK_PLAN_GET_STATUS => get_work_plan_status(pool, &context, role, arguments).await,
        DEN_WORK_PLAN_UPDATE => update_work_plan(pool, &context, role, arguments).await,
        DEN_PLAN_MODE_ENTER => enter_plan_mode(pool, &context, arguments).await,
        DEN_PLAN_MODE_STATUS => plan_mode_status(pool, &context).await,
        DEN_PLAN_MODE_RECORD_APPROVAL => record_plan_approval(pool, &context, arguments).await,
        DEN_PLAN_MODE_EXIT => exit_plan_mode(pool, config, &context, arguments).await,
        DEN_PLAN_MODE_CANCEL => cancel_plan_mode(pool, &context, arguments).await,
        DEN_SKILL_PROPOSE
        | DEN_SKILL_APPROVE_PROPOSAL
        | DEN_SKILL_REJECT_PROPOSAL
        | DEN_WORK_PLAN_REQUEST_HANDOFF
        | DEN_TASK_WRITE_INTENT
        | DEN_TASK_APPROVE_INTENT
        | DEN_TASK_REJECT_INTENT
        | DEN_CORE_WRITE_RESULT_SUMMARY
        | DEN_OBSERVATION_WRITE
        | DEN_RUN_WRITE_RESULT => Err(CustomError::System(format!(
            "Den tool `{tool_name}` is registered and role-authorized but not implemented yet"
        ))),
        _ => Err(CustomError::NotFound(format!(
            "unknown Den tool: {tool_name}"
        ))),
    }
}

fn prevalidate_tool_arguments(tool_name: &str, arguments: &Value) -> Result<(), CustomError> {
    match tool_name {
        DEN_MEMORY_WRITE_ENTRY => {
            let args: MemoryWriteEntryArguments = serde_json::from_value(arguments.clone())?;
            validate_memory_write_entry_semantics(&args)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn authorize_context(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<BearAgentRole, CustomError> {
    if !bears_db::user_may_use_bear(pool, context.user_id, context.bear_id).await? {
        return Err(CustomError::Authorization(
            "user is not a member of this bear".to_string(),
        ));
    }
    context_role(pool, context).await
}

async fn context_role(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<BearAgentRole, CustomError> {
    let agent_id = context.role_agent_id.trim();
    if agent_id.is_empty() {
        return Err(CustomError::Authorization(
            "Den tool context is missing role_agent_id".to_string(),
        ));
    }

    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role
        FROM bear_agents
        WHERE bear_id = $1
          AND letta_agent_id = $2
        "#,
    )
    .bind(context.bear_id)
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let registered_role: BearAgentRole = row
        .ok_or_else(|| {
            CustomError::Authorization("role_agent_id is not registered for this bear".to_string())
        })?
        .0
        .parse()
        .map_err(CustomError::System)?;
    if let Some(declared_role) = context.agent_role {
        if declared_role != registered_role {
            return Err(CustomError::Authorization(format!(
                "Den tool context role `{declared_role}` does not match registered role `{registered_role}` for role_agent_id"
            )));
        }
    }
    Ok(registered_role)
}

fn authorize_tool_for_role(tool_name: &str, role: BearAgentRole) -> Result<(), CustomError> {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == tool_name)
        .ok_or_else(|| CustomError::NotFound(format!("unknown Den tool: {tool_name}")))?;
    if descriptor.allows_role(role) {
        Ok(())
    } else {
        Err(CustomError::Authorization(format!(
            "Den tool `{tool_name}` is not available to the `{role}` role"
        )))
    }
}

async fn get_bear_self(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let bear = bears_db::get_bear(pool, context.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let member_count = bears_db::count_bear_members(pool, bear.id).await?;
    Ok(json!({
        "bear": {
            "bear_id": bear.id,
            "slug": bear.slug,
            "name": bear.name,
            "description": bear.description,
            "default_model": bear.default_model,
            "letta_agent_type": bear.letta_agent_type,
            "member_count": member_count,
            "created_at": format_rfc3339(bear.created_at),
            "updated_at": format_rfc3339(bear.updated_at)
        },
        "viewer": {
            "user_id": context.user_id,
            "username": context.username,
            "membership_role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref())
        }
    }))
}

async fn get_current_user(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let current = user::user_by_id(pool, context.user_id).await?;
    Ok(json!({
        "user": {
            "user_id": current.id,
            "username": current.username,
            "display_name": current.display_name,
            "email_verified": current.email_verified.unwrap_or(false),
            "created_at": format_rfc3339(current.created.assume_utc())
        },
        "bear_membership": {
            "bear_id": context.bear_id,
            "role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref())
        }
    }))
}

async fn list_bear_members(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let members = bears_db::list_members_for_bear(pool, context.bear_id).await?;
    let can_manage_members = role_is_bear_admin(context.membership_role.as_deref());
    let member_values: Vec<Value> = members
        .into_iter()
        .map(|member| {
            json!({
                "user_id": member.user_id,
                "username": member.username,
                "display_name": member.display_name,
                "role": member.role,
            })
        })
        .collect();
    Ok(json!({
        "bear_id": context.bear_id,
        "members": member_values,
        "policy": {
            "viewer_role": context.membership_role,
            "can_manage_members": can_manage_members,
            "redacted_fields": ["email"]
        }
    }))
}

async fn list_capabilities_self(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let role = context_role(pool, context).await?;
    let descriptors = builtin_den_tool_descriptors_for_role(role);
    Ok(json!({
        "bear_id": context.bear_id,
        "channel": context.channel,
        "capabilities": descriptors,
    }))
}

fn channel_context(context: &DenToolInvocationContext) -> Value {
    json!({
        "bear_id": context.bear_id,
        "role_agent_id": context.role_agent_id,
        "agent_role": context.agent_role,
        "user_id": context.user_id,
        "conversation_id": context.conversation_id,
        "session_id": context.session_id,
        "request_id": context.request_id,
        "channel": context.channel,
    })
}

async fn policy_self(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let member_count = bears_db::count_bear_members(pool, context.bear_id).await?;
    let is_bear_admin = role_is_bear_admin(context.membership_role.as_deref());
    Ok(json!({
        "bear_id": context.bear_id,
        "user_id": context.user_id,
        "membership_role": context.membership_role,
        "is_bear_admin": is_bear_admin,
        "can_chat": true,
        "can_read_bear_profile": true,
        "can_list_members": true,
        "can_manage_members": is_bear_admin,
        "can_list_capabilities": true,
        "can_read_channel_context": true,
        "member_count": member_count,
        "policy_notes": [
            "Den tool calls are scoped to the current trusted bear/user context.",
            "Emails and authentication internals are not exposed through these tools."
        ]
    }))
}

async fn set_conversation_title(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: SetConversationTitleArguments = serde_json::from_value(arguments)?;
    let title = args.title.trim().chars().take(120).collect::<String>();
    if title.is_empty() {
        return Err(CustomError::ValidationError(
            "conversation title cannot be empty".to_string(),
        ));
    }
    let conversation_id = clean_optional(&context.conversation_id).ok_or_else(|| {
        CustomError::ValidationError(
            "current conversation is not saved yet; send a message before setting its title"
                .to_string(),
        )
    })?;
    if conversation_id == "default" || conversation_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "current conversation is not saved yet; send a message before setting its title"
                .to_string(),
        ));
    }
    patch_letta_conversation_summary(config, &conversation_id, &title).await?;
    let synced_acp_sessions = acp_sessions::set_title_for_bear_conversation(
        pool,
        context.bear_id,
        &conversation_id,
        &title,
    )
    .await?;
    Ok(json!({
        "ok": true,
        "conversation_id": conversation_id,
        "title": title,
        "synced_acp_sessions": synced_acp_sessions,
        "content": format!("Conversation title set to {title:?}."),
    }))
}

async fn patch_letta_conversation_summary(
    config: &Config,
    conversation_id: &str,
    summary: &str,
) -> Result<(), CustomError> {
    let base_url = config.letta_base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL)".to_string(),
        ));
    }
    let url = format!("{base_url}/v1/conversations/{conversation_id}");
    let mut request = reqwest::Client::new()
        .patch(url)
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({ "summary": summary }));
    let key = config.letta_api_key.trim();
    if !key.is_empty() {
        request = request.header(AUTHORIZATION, format!("Bearer {key}"));
    }
    let response = request
        .send()
        .await
        .map_err(|err| CustomError::System(format!("Letta patch conversation failed: {err}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "Letta patch conversation HTTP {status}: {text}"
        )));
    }
    Ok(())
}

async fn web_fetch(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WebFetchArguments = serde_json::from_value(arguments)?;
    let max_chars = args.max_chars.unwrap_or(8_000).clamp(1, 20_000);
    let (normalized, decision) =
        web_policy::decide_web_fetch_approval(pool, context.bear_id, &args.url).await?;
    if matches!(decision, web_policy::WebApprovalDecision::Blocked) {
        web_policy::record_web_fetch_attempt(
            pool,
            context.bear_id,
            Some(context.session_id.as_str()),
            None,
            &normalized.url,
            None,
            &normalized.host,
            "den",
            decision.as_str(),
            None,
            None,
            None,
        )
        .await?;
        return Err(CustomError::Authorization(format!(
            "web_fetch host or URL is blocked by bear policy: {}",
            normalized.host
        )));
    }
    if !decision.is_approved() {
        web_policy::record_web_fetch_attempt(
            pool,
            context.bear_id,
            Some(context.session_id.as_str()),
            None,
            &normalized.url,
            None,
            &normalized.host,
            "den",
            decision.as_str(),
            None,
            None,
            None,
        )
        .await?;
        return Err(CustomError::Authorization(format!(
            "web_fetch requires approval for host {}",
            normalized.host
        )));
    }
    let url = validate_public_http_url(&normalized.url)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| CustomError::System(format!("web fetch client build failed: {e}")))?;
    let resp = client
        .get(url.as_str())
        .header(reqwest::header::USER_AGENT, "BEARS Den web_fetch/0.1")
        .send()
        .await
        .map_err(|e| CustomError::System(format!("web fetch request failed: {e}")))?;
    let final_url = resp.url().clone();
    validate_public_http_url(final_url.as_str())?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CustomError::System(format!("web fetch response read failed: {e}")))?;
    const MAX_BYTES: usize = 1_000_000;
    let bytes_truncated = bytes.len() > MAX_BYTES;
    let slice = if bytes_truncated {
        &bytes[..MAX_BYTES]
    } else {
        &bytes[..]
    };
    let raw = String::from_utf8_lossy(slice).to_string();
    let text = if content_type.to_ascii_lowercase().contains("html") {
        html_to_text_excerpt(&raw)
    } else {
        raw
    };
    let (text_excerpt, char_truncated) = truncate_chars(&text, max_chars);
    let final_normalized = web_policy::normalize_web_url(final_url.as_str())?;
    web_policy::record_web_fetch_attempt(
        pool,
        context.bear_id,
        Some(context.session_id.as_str()),
        None,
        &normalized.url,
        Some(final_url.as_str()),
        &final_normalized.host,
        "den",
        decision.as_str(),
        Some(status.as_u16() as i32),
        Some(&content_type),
        Some(bytes.len() as i64),
    )
    .await?;
    Ok(json!({
        "url": final_url.as_str(),
        "host": final_normalized.host,
        "approval": decision.as_str(),
        "status": status.as_u16(),
        "content_type": content_type,
        "text_excerpt": text_excerpt,
        "truncated": bytes_truncated || char_truncated,
    }))
}

async fn web_search(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    web_search_inner(Some(pool), config, Some(context), arguments).await
}

async fn web_search_inner(
    pool: Option<&PgPool>,
    config: &Config,
    context: Option<&DenToolInvocationContext>,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WebSearchArguments = serde_json::from_value(arguments)?;
    if args.query.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "query must not be empty".to_string(),
        ));
    }
    let max_results = args
        .max_results
        .unwrap_or(config.den_search_max_results)
        .clamp(1, 10);
    let mut value = match config.den_search_provider.as_str() {
        "brave" => brave_web_search(config, args.query.trim(), max_results).await,
        "" => Err(CustomError::System(format!(
            "den.web.search is registered but DEN_SEARCH_PROVIDER is not configured (query={}, max_results={max_results}). Set DEN_SEARCH_PROVIDER=brave and BRAVE_SEARCH_API_KEY.",
            serde_json::Value::String(args.query.trim().to_string())
        ))),
        other => Err(CustomError::System(format!(
            "unsupported DEN_SEARCH_PROVIDER={other:?}; supported providers: brave"
        ))),
    }?;
    let preferred_hosts = if let (Some(pool), Some(context)) = (pool, context) {
        web_policy::preferred_hosts_for_bear(pool, context.bear_id).await?
    } else {
        Vec::new()
    };
    if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
        for result in results.iter_mut() {
            if let Some(url) = result.get("url").and_then(Value::as_str) {
                if let Ok(normalized) = web_policy::normalize_web_url(url) {
                    let preferred = preferred_hosts.iter().any(|host| host == &normalized.host);
                    result["host"] = json!(normalized.host);
                    result["preferred_source"] = json!(preferred);
                }
            }
        }
        results.sort_by_key(|item| {
            !item
                .get("preferred_source")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    }
    value["preferred_hosts"] = json!(preferred_hosts);
    value["instruction"] = json!("Prefer results with preferred_source=true when they are relevant; otherwise use ordinary relevance judgment.");
    Ok(value)
}

fn truncate_search_detail(s: String) -> String {
    const MAX: usize = 500;
    if s.len() <= MAX {
        s
    } else {
        format!("{}…", &s[..MAX.saturating_sub(1)])
    }
}

async fn brave_web_search(
    config: &Config,
    query: &str,
    max_results: usize,
) -> Result<Value, CustomError> {
    let key = config.brave_search_api_key.trim();
    if key.is_empty() {
        return Err(CustomError::System(
            "DEN_SEARCH_PROVIDER=brave requires BRAVE_SEARCH_API_KEY".to_string(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| CustomError::System(format!("Brave search client build failed: {e}")))?;
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&[("q", query), ("count", &max_results.to_string())])
        .send()
        .await
        .map_err(|e| CustomError::System(format!("Brave search request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "Brave search HTTP {status}: {}",
            truncate_search_detail(text)
        )));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|e| CustomError::Parsing(format!("Brave search JSON: {e}")))?;
    let results = payload
        .get("web")
        .and_then(|v| v.get("results"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .take(max_results)
        .map(|item| {
            json!({
                "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "snippet": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                "source_domain": item.get("profile").and_then(|p| p.get("long_name")).and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "provider": "brave",
        "query": query,
        "max_results": max_results,
        "results": results,
        "note": "Search snippets are untrusted external content. Use web_fetch on selected URLs for bounded page content."
    }))
}

async fn session_info(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
) -> Result<Value, CustomError> {
    let member_count = bears_db::count_bear_members(pool, context.bear_id).await?;
    let current_user = user::user_by_id(pool, context.user_id).await.ok();
    let memory_status = memory_status_value(config, context, role)
        .await
        .unwrap_or_else(|err| {
            json!({
                "configured": !config.letta_memfs_service_url.trim().is_empty(),
                "available": false,
                "error": err.to_string()
            })
        });
    Ok(json!({
        "bear": {
            "bear_id": context.bear_id,
            "bear_slug": context.bear_slug,
            "member_count": member_count
        },
        "role": role.as_str(),
        "role_agent_id": context.role_agent_id,
        "human": {
            "user_id": context.user_id,
            "username": current_user.as_ref().map(|user| user.username.clone()).or_else(|| context.username.clone()),
            "display_name": current_user.as_ref().map(|user| user.display_name.clone()),
            "email_verified": current_user.as_ref().map(|user| user.email_verified.unwrap_or(false)),
            "membership_role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref()),
            "relationship": "authenticated ACP token owner; memory entries and logs should attribute work to this human"
        },
        "user": {
            "user_id": context.user_id,
            "username": current_user.as_ref().map(|user| user.username.clone()).or_else(|| context.username.clone()),
            "display_name": current_user.as_ref().map(|user| user.display_name.clone()),
            "membership_role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref())
        },
        "session": {
            "conversation_id": context.conversation_id,
            "session_id": context.session_id,
            "acp_session_id": context.acp_session_id,
            "conversation_selection": context.conversation_selection,
            "runtime_target": context.runtime_target,
            "request_id": context.request_id,
            "channel": context.channel
        },
        "memory": {
            "read_scopes": memory_read_scopes(role),
            "write_scopes": memory_write_scopes(role),
            "available_tools": [
                DEN_MEMORY_WRITE_ENTRY_PROVIDER,
                DEN_MEMORY_STATUS_PROVIDER,
                DEN_MEMORY_TREE_PROVIDER,
                DEN_MEMORY_READ_PROVIDER,
                DEN_MEMORY_SEARCH_PROVIDER
            ],
            "status": memory_status
        },
        "policy_notes": [
            "Session info is a Den-trusted briefing, not the model context window.",
            "Use memory_write_entry only for role-local notes, logs, decisions, reflections, scratch, and summaries; entries are attributed to the authenticated human in this session.",
            "Do not use memory entry tools for tasks, observations, run results, Cabinet writes, or direct core updates."
        ]
    }))
}

async fn enter_plan_mode(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: PlanModeEnterArguments = serde_json::from_value(arguments)?;
    let acp_session_id = source_acp_session_id(context).ok_or_else(|| {
        CustomError::ValidationError("ACP session id is required for plan mode".to_string())
    })?;
    let row = acp_plan_mode::enter_plan_mode(
        pool,
        EnterPlanModeParams {
            user_id: context.user_id,
            bear_id: context.bear_id,
            bear_slug: context.bear_slug.clone(),
            acp_session_id: acp_session_id.clone(),
            reason: args.reason,
            requested_by: AcpPlanModeRequestedBy::Pair,
            previous_permission_mode: args.previous_permission_mode,
        },
    )
    .await?;
    acp_sessions::set_current_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        "plan",
    )
    .await?;
    Ok(json!({
        "domain": "workplan",
        "workplan": plan_mode_workplan_payload(&row),
        "plan_mode": row,
        "workflow_state": turn_state::turn_state_json(&crate::core::acp_tools::AcpResolvedSessionPolicy {
            mode_label: "Plan",
            tool_enablement: crate::core::acp_tools::AcpToolEnablementState::ReadOnly,
            plan_mode_state: Some(row.state.clone()),
        }, None),
        "mode_update": "plan",
        "instructions": [
            "Plan mode is active for this ACP session.",
            "Inspect, read, search, and use read-only Den tools as needed.",
            "Do not mutate workspace files, run non-read-only shell commands, or perform external side effects until the submitted plan is approved.",
            "Call den.plan_mode.exit with a concise markdown implementation plan when ready for user approval."
        ]
    }))
}

async fn plan_mode_status(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<Value, CustomError> {
    let acp_session_id = source_acp_session_id(context).ok_or_else(|| {
        CustomError::ValidationError("ACP session id is required for plan mode".to_string())
    })?;
    let row =
        acp_plan_mode::active_for_session(pool, context.user_id, context.bear_id, &acp_session_id)
            .await?;
    let workplan = row
        .as_ref()
        .map(plan_mode_workplan_payload)
        .unwrap_or_else(no_active_workplan_payload);
    Ok(json!({
        "domain": "workplan",
        "bear_id": context.bear_id,
        "acp_session_id": acp_session_id,
        "workplan": workplan,
        "plan_mode": row,
        "active": row.is_some(),
    }))
}

async fn record_plan_approval(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: PlanModeRecordApprovalArguments = serde_json::from_value(arguments)?;
    let approval_text = validate_bounded_text("approval_text", &args.approval_text, 1, 1000)?;
    let acp_session_id = source_acp_session_id(context).ok_or_else(|| {
        CustomError::ValidationError("ACP session id is required for plan approval".to_string())
    })?;
    let current = acp_plan_mode::get_for_session(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        args.plan_mode_id,
    )
    .await?
    .ok_or_else(|| {
        CustomError::NotFound("submitted ACP plan mode session not found".to_string())
    })?;
    if current.state != "submitted" {
        return Err(CustomError::ValidationError(format!(
            "plan approval requires a submitted plan; current state is {}",
            current.state
        )));
    }
    let row = acp_plan_mode::approve_plan_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        current.id,
    )
    .await?;
    acp_sessions::set_current_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        "write",
    )
    .await?;
    Ok(json!({
        "domain": "workplan",
        "ok": true,
        "workplan": plan_mode_workplan_payload(&row),
        "plan_mode": row,
        "workflow_state": turn_state::turn_state_json(&crate::core::acp_tools::AcpResolvedSessionPolicy {
            mode_label: "Write",
            tool_enablement: crate::core::acp_tools::AcpToolEnablementState::AllTools,
            plan_mode_state: Some(row.state.clone()),
        }, None),
        "mode_update": "write",
        "approval_text": approval_text,
        "content": "Plan approved by the authenticated human. Write mode is now enabled; implementation may proceed subject to normal ACP tool approvals.",
    }))
}

async fn exit_plan_mode(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: PlanModeExitArguments = serde_json::from_value(arguments)?;
    let acp_session_id = source_acp_session_id(context).ok_or_else(|| {
        CustomError::ValidationError("ACP session id is required for plan mode".to_string())
    })?;
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let body = validate_bounded_text("body", &args.body, 1, 50_000)?;
    let markdown = acp_plan_mode::render_plan_artifact_markdown(&title, &body);
    let memory_request = MemfsWriteRoleMemoryEntryRequest {
        kind: "plan".to_string(),
        title: title.clone(),
        body: markdown,
        tags: vec!["plan-mode".to_string(), "implementation-plan".to_string()],
        refs: None,
        lifecycle: Some(json!({ "scope": "role-local", "retention": "durable" })),
        source: Some(json!({
            "tool": DEN_PLAN_MODE_EXIT,
            "acp_session_id": acp_session_id,
            "conversation_id": clean_optional(&context.conversation_id),
        })),
        author: context.username.clone(),
        conversation_id: clean_optional(&context.conversation_id),
        session_id: Some(acp_session_id.clone()),
        acp_session_id: Some(acp_session_id.clone()),
        conversation_selection: context.conversation_selection.clone(),
        runtime_target: context.runtime_target.clone(),
        role_agent_id: Some(context.role_agent_id.clone()),
        agent_role: Some(BearAgentRole::Pair.as_str().to_string()),
        request_id: context.request_id.clone(),
    };
    let http = memfs_http_client("MemFS plan artifact client build failed")?;
    let memfs_response = write_memfs_role_memory_entry(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        BearAgentRole::Pair.as_str(),
        &memory_request,
    )
    .await?;
    let Some(memfs_response) = memfs_response else {
        return Err(CustomError::System(
            "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)".to_string(),
        ));
    };
    let current_plan = acp_plan_mode::get_for_session(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        args.plan_mode_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("active ACP plan mode session not found".to_string()))?;
    let row = acp_plan_mode::submit_plan_artifact(
        pool,
        SubmitPlanModeParams {
            user_id: context.user_id,
            bear_id: context.bear_id,
            acp_session_id: acp_session_id.clone(),
            plan_mode_id: Some(current_plan.id),
            title,
            body,
            artifact_path: memfs_response.path.clone(),
            approval_request_id: Some(format!("plan-mode-{}", current_plan.id)),
        },
    )
    .await?;
    acp_sessions::set_current_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        "plan",
    )
    .await?;
    Ok(json!({
        "domain": "workplan",
        "workplan": plan_mode_workplan_payload(&row),
        "plan_mode": row,
        "workflow_state": turn_state::turn_state_json(&crate::core::acp_tools::AcpResolvedSessionPolicy {
            mode_label: "Plan",
            tool_enablement: crate::core::acp_tools::AcpToolEnablementState::ReadOnly,
            plan_mode_state: Some(row.state.clone()),
        }, None),
        "artifact": {
            "domain": "workplan",
            "content_class": "workplan_artifact",
            "path": memfs_response.path,
            "entry_id": memfs_response.entry_id,
            "commit": memfs_response.commit,
        },
        "approval_required": false,
        "mode_update": "plan",
        "submitted_plan": {
            "title": row.plan_title,
            "body": row.plan_body,
            "artifact_path": row.plan_artifact_path,
        },
        "instructions": [
            "Present this plan artifact to the user if useful.",
            "If the authenticated human clearly approves the plan in chat, call record_plan_approval. Tool use remains governed by Den policy and ACP client approval."
        ]
    }))
}

async fn cancel_plan_mode(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: PlanModeCancelArguments = serde_json::from_value(arguments)?;
    let acp_session_id = source_acp_session_id(context).ok_or_else(|| {
        CustomError::ValidationError("ACP session id is required for plan mode".to_string())
    })?;
    let row = acp_plan_mode::cancel_plan_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        args.plan_mode_id,
    )
    .await?;
    acp_sessions::set_current_mode(
        pool,
        context.user_id,
        context.bear_id,
        &acp_session_id,
        "ask",
    )
    .await?;
    Ok(json!({
        "domain": "workplan",
        "workplan": plan_mode_workplan_payload(&row),
        "plan_mode": row,
        "workflow_state": turn_state::turn_state_json(&crate::core::acp_tools::AcpResolvedSessionPolicy {
            mode_label: "Ask",
            tool_enablement: crate::core::acp_tools::AcpToolEnablementState::ReadOnly,
            plan_mode_state: Some(row.state.clone()),
        }, None),
        "mode_update": "ask"
    }))
}

async fn write_memory_entry(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Pair {
        return Err(CustomError::Authorization(
            "den.memory.write_entry is currently available only to the pair role".to_string(),
        ));
    }
    let args: MemoryWriteEntryArguments = serde_json::from_value(arguments)?;
    let kind = validate_memory_write_entry_semantics(&args)?;
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let body = validate_bounded_text("body", &args.body, 1, 50_000)?;
    let tags = clean_limited_strings(args.tags, 20, 80);
    validate_optional_object("refs", &args.refs)?;
    validate_optional_object("lifecycle", &args.lifecycle)?;
    validate_optional_object("source", &args.source)?;
    let current_user = user::user_by_id(pool, context.user_id).await.ok();
    let source = merge_memory_entry_source_with_human(args.source, context, current_user.as_ref());
    let request = MemfsWriteRoleMemoryEntryRequest {
        kind,
        title,
        body,
        tags,
        refs: args.refs,
        lifecycle: args.lifecycle,
        source,
        author: context.username.clone(),
        conversation_id: clean_optional(&context.conversation_id),
        session_id: source_acp_session_id(context).or_else(|| clean_optional(&context.session_id)),
        acp_session_id: context
            .acp_session_id
            .clone()
            .or_else(|| source_acp_session_id(context)),
        conversation_selection: context.conversation_selection.clone(),
        runtime_target: context.runtime_target.clone(),
        role_agent_id: Some(context.role_agent_id.clone()),
        agent_role: context.agent_role.map(|role| role.as_str().to_string()),
        request_id: context.request_id.clone(),
    };
    let http = memfs_http_client("MemFS memory entry client build failed")?;
    let response = write_memfs_role_memory_entry(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        role.as_str(),
        &request,
    )
    .await?;
    let Some(response) = response else {
        return Err(CustomError::System(
            "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)".to_string(),
        ));
    };
    Ok(json!({
        "bear_id": context.bear_id,
        "role": role.as_str(),
        "kind": response.kind,
        "entry_id": response.entry_id,
        "path": response.path,
        "commit": response.commit,
        "canonical_tip": response.canonical_tip,
        "view": response.view,
    }))
}

async fn memory_status(
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
) -> Result<Value, CustomError> {
    memory_status_value(config, context, role).await
}

async fn memory_status_value(
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
) -> Result<Value, CustomError> {
    let http = memfs_http_client("MemFS memory status client build failed")?;
    let response = fetch_memfs_role_memory_status(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        role.as_str(),
    )
    .await?;
    let Some(response) = response else {
        return Ok(json!({
            "configured": false,
            "available": false,
            "message": "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)"
        }));
    };
    Ok(json!({
        "configured": true,
        "available": response.ok,
        "bear_id": context.bear_id,
        "role": role.as_str(),
        "canonical_tip": response.canonical_tip,
        "allowed_prefixes": response.allowed_prefixes,
        "file_count": response.file_count,
        "entry_count_by_kind": response.entry_count_by_kind,
        "registered_view_count": response.registered_view_count,
    }))
}

async fn memory_browse(
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
) -> Result<Value, CustomError> {
    let http = memfs_http_client("MemFS memory browse client build failed")?;
    let response = fetch_memfs_role_memory_tree(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        role.as_str(),
    )
    .await?;
    response
        .map(|value| {
            serde_json::to_value(value)
                .map_err(|e| CustomError::Parsing(format!("memory browse JSON: {e}")))
        })
        .unwrap_or_else(|| {
            Ok(json!({
                "ok": false,
                "configured": false,
                "message": "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)"
            }))
        })
}

async fn memory_read(
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: MemoryReadArguments = serde_json::from_value(arguments)?;
    let path = args.path.trim();
    if path.is_empty() {
        return Err(CustomError::ValidationError(
            "path must not be empty".to_string(),
        ));
    }
    let http = memfs_http_client("MemFS memory read client build failed")?;
    let response = fetch_memfs_role_memory_file(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        role.as_str(),
        path,
    )
    .await?;
    response
        .map(|value| {
            serde_json::to_value(value)
                .map_err(|e| CustomError::Parsing(format!("memory file JSON: {e}")))
        })
        .unwrap_or_else(|| {
            Ok(json!({
                "ok": false,
                "configured": false,
                "message": "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)"
            }))
        })
}

async fn memory_search(
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: MemorySearchArguments = serde_json::from_value(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        return Err(CustomError::ValidationError(
            "query must not be empty".to_string(),
        ));
    }
    let http = memfs_http_client("MemFS memory search client build failed")?;
    let response = search_memfs_role_memory(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        role.as_str(),
        query,
        args.limit.map(|n| n.clamp(1, 50)),
    )
    .await?;
    response
        .map(|value| {
            serde_json::to_value(value)
                .map_err(|e| CustomError::Parsing(format!("memory search JSON: {e}")))
        })
        .unwrap_or_else(|| {
            Ok(json!({
                "ok": false,
                "configured": false,
                "message": "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)"
            }))
        })
}

async fn apply_core_update(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Curate {
        return Err(CustomError::Authorization(
            "den.memory.apply_core_update is available only to curate".to_string(),
        ));
    }
    let args: MemoryApplyCoreUpdateArguments = serde_json::from_value(arguments)?;
    let proposal = memory_proposals::get_for_bear(pool, context.bear_id, args.proposal_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("memory proposal not found".to_string()))?;
    let http = memfs_http_client("MemFS core update client build failed")?;
    let body = args.body.map(|body| {
        format!(
            "{}\n\n---\nSource proposal: `{}`\nSource role: `{}`\nSource paths: {}\n",
            body.trim(),
            proposal.id,
            proposal.source_role,
            proposal.source_paths.join(", ")
        )
    });
    let request = MemfsCoreUpdateRequest {
        target_path: args.target_path,
        mode: args.mode,
        title: args.title.or(Some(proposal.title.clone())),
        body,
        old_text: args.old_text,
        new_text: args.new_text,
        proposal_id: Some(proposal.id),
        source_paths: proposal.source_paths.clone(),
    };
    let response = write_memfs_core_update(
        &http,
        &config.letta_memfs_service_url,
        context.bear_id,
        &request,
    )
    .await?;
    let Some(response) = response else {
        return Err(CustomError::System(
            "MemFS sidecar is not configured (set LETTA_MEMFS_SERVICE_URL)".to_string(),
        ));
    };
    let resolved = memory_proposals::resolve_for_bear(
        pool,
        context.bear_id,
        proposal.id,
        role,
        Some(context.role_agent_id.as_str()),
        "approved",
        args.review_notes.as_deref(),
        Some("Applied reviewed memory proposal to core."),
    )
    .await?;
    Ok(json!({
        "bear_id": context.bear_id,
        "proposal": resolved,
        "core_update": response,
    }))
}

async fn list_memory_proposals(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Curate {
        return Err(CustomError::Authorization(
            "den.memory.list_proposals is available only to curate".to_string(),
        ));
    }
    let args: MemoryListProposalsArguments = serde_json::from_value(arguments)?;
    let status = args
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let proposals =
        memory_proposals::list_for_bear(pool, context.bear_id, status, args.limit.unwrap_or(50))
            .await?;
    Ok(json!({ "bear_id": context.bear_id, "proposals": proposals }))
}

async fn read_memory_proposal(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Curate {
        return Err(CustomError::Authorization(
            "den.memory.read_proposal is available only to curate".to_string(),
        ));
    }
    let args: MemoryReadProposalArguments = serde_json::from_value(arguments)?;
    let proposal = memory_proposals::get_for_bear(pool, context.bear_id, args.proposal_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("memory proposal not found".to_string()))?;
    Ok(json!({ "bear_id": context.bear_id, "proposal": proposal }))
}

async fn resolve_memory_proposal(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Curate {
        return Err(CustomError::Authorization(
            "den.memory.resolve_proposal is available only to curate".to_string(),
        ));
    }
    let args: MemoryResolveProposalArguments = serde_json::from_value(arguments)?;
    let status = args.status.trim();
    if !matches!(
        status,
        "rejected" | "retained_local" | "deferred" | "superseded" | "needs_human_review"
    ) {
        return Err(CustomError::ValidationError(
            "status must be rejected, retained_local, deferred, superseded, or needs_human_review"
                .to_string(),
        ));
    }
    let proposal = memory_proposals::resolve_for_bear(
        pool,
        context.bear_id,
        args.proposal_id,
        role,
        Some(context.role_agent_id.as_str()),
        status,
        args.review_notes.as_deref(),
        args.decision_summary.as_deref(),
    )
    .await?;
    Ok(json!({ "bear_id": context.bear_id, "proposal": proposal }))
}

async fn request_memory_review(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if !matches!(role, BearAgentRole::Pair) {
        return Err(CustomError::Authorization(
            "den.memory.request_review is currently available only to pair".to_string(),
        ));
    }
    let args: MemoryRequestReviewArguments = serde_json::from_value(arguments)?;
    let source_paths = args
        .source_paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    if source_paths.is_empty() {
        return Err(CustomError::ValidationError(
            "source_paths must include at least one path".to_string(),
        ));
    }
    if source_paths.len() > 20 {
        return Err(CustomError::ValidationError(
            "source_paths must include at most 20 paths".to_string(),
        ));
    }
    for path in &source_paths {
        if !path.starts_with(role.as_str()) || !path.ends_with(".md") {
            return Err(CustomError::ValidationError(format!(
                "source path must be a role-local Markdown path under {}/: {path}",
                role.as_str()
            )));
        }
    }
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let summary = validate_bounded_text("summary", &args.summary, 1, 4_000)?;
    let rationale = validate_bounded_text("rationale", &args.rationale, 0, 4_000)?;
    validate_optional_object("refs", &args.refs)?;
    let suggested_action = args
        .suggested_action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unspecified");
    let sensitivity = args
        .sensitivity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("normal");
    let proposal = memory_proposals::create(
        pool,
        CreateMemoryProposal {
            bear_id: context.bear_id,
            source_role: role,
            source_agent_id: clean_optional(&context.role_agent_id),
            source_paths,
            source_refs: serde_json::json!([]),
            suggested_action,
            target_ref: args
                .target_ref
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            title: &title,
            summary: &summary,
            rationale: &rationale,
            proposed_content: args.proposed_content.as_deref(),
            proposed_patch: args.proposed_patch.as_deref(),
            refs: args.refs.unwrap_or_else(|| serde_json::json!({})),
            sensitivity,
            requires_human: args.requires_human,
        },
    )
    .await?;
    Ok(json!({
        "bear_id": context.bear_id,
        "proposal": proposal,
        "note": "Review requested. Reflection/curate decides the final outcome; this did not write core, Cabinet, skills, tasks, observations, or run results."
    }))
}

async fn list_work_plans(
    pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkPlanListArguments = serde_json::from_value(arguments)?;
    let include_plan_mode = args.include_plan_mode.unwrap_or(true);
    let include_artifacts = args.include_artifacts.unwrap_or(true);
    let statuses = args.statuses.or_else(|| {
        (!args.include_completed).then(|| vec![WorkPlanStatus::Active, WorkPlanStatus::Blocked])
    });
    let activity_rows = work_plans::list_visible_work_plans(
        pool,
        context.bear_id,
        role,
        context.user_id,
        WorkPlanListFilter {
            statuses,
            owner_role: args.owner_role,
            include_archived: args.include_archived,
        },
    )
    .await?;
    let plan_mode_gates = if include_plan_mode {
        acp_plan_mode::list_for_bear(pool, context.bear_id, args.include_completed, 50).await?
    } else {
        Vec::new()
    };
    let plan_artifacts = if include_artifacts {
        let http = memfs_http_client("MemFS plan artifact list client build failed")?;
        match fetch_memfs_role_plan_artifacts(
            &http,
            &config.letta_memfs_service_url,
            context.bear_id,
            BearAgentRole::Pair.as_str(),
        )
        .await
        {
            Ok(Some(response)) => response.results,
            Ok(None) => json!([]),
            Err(err) => json!({ "error": err.to_string() }),
        }
    } else {
        json!([])
    };
    let linked_artifact_paths = plan_mode_gates
        .iter()
        .filter_map(|gate| gate.plan_artifact_path.as_deref())
        .collect::<Vec<_>>();
    let activity_plans = activity_rows
        .iter()
        .map(|plan| activity_payload(Some(plan)))
        .collect::<Vec<_>>();
    let workplans = plan_mode_gates
        .iter()
        .map(plan_mode_workplan_payload)
        .collect::<Vec<_>>();
    Ok(json!({
        "domain": "activity",
        "bear_id": context.bear_id,
        "viewer_role": role.as_str(),
        "planning_scope": "bear",
        "workplace": {
            "status": "unresolved",
            "note": "Workplace inference is not implemented yet; workspace/session metadata is returned as workplace reference candidates.",
            "reference_candidates": {
                "acp_session_id": context.acp_session_id,
                "session_id": context.session_id,
                "conversation_id": clean_optional(&context.conversation_id),
                "conversation_selection": context.conversation_selection,
                "runtime_target": context.runtime_target,
                "channel": context.channel,
            }
        },
        "activities": activity_plans,
        "activity_plans": activity_plans,
        "plans": activity_rows,
        "activity_rows": activity_rows,
        "workplans": workplans,
        "plan_mode_gates": plan_mode_gates,
        "plan_artifacts": plan_artifacts,
        "linked_plan_artifact_paths": linked_artifact_paths,
        "notes": [
            "list_plans is a Bear-level planning view. It includes live activity plans, submitted/active workplan gates, and saved pair workplan artifacts when available.",
            "A workplan artifact in pair/plans/ may exist even when there is no active live activity plan; this is workplan-domain state, not semantic memory.",
            "Role fields are provenance and policy hints, not product ownership. Cross-role visibility is not cross-role execution authority."
        ],
    }))
}

async fn get_work_plan_status(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkPlanGetStatusArguments = serde_json::from_value(arguments)?;
    let lookup = WorkPlanLookup {
        plan_id: args.plan_id,
        source_conversation_id: args
            .source_conversation_id
            .or_else(|| clean_optional(&context.conversation_id)),
        source_acp_session_id: args
            .source_acp_session_id
            .or_else(|| source_acp_session_id(context)),
    };
    let plan =
        work_plans::get_visible_work_plan(pool, context.bear_id, role, context.user_id, lookup)
            .await?;
    Ok(json!({
        "domain": "activity",
        "bear_id": context.bear_id,
        "activity": activity_payload(plan.as_ref()),
        "plan": plan,
    }))
}

async fn update_work_plan(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkPlanUpdateArguments = serde_json::from_value(arguments)?;
    let row = work_plans::create_or_update_work_plan(
        pool,
        WorkPlanUpsert {
            bear_id: context.bear_id,
            owner_role: role,
            owner_agent_id: clean_optional(&context.role_agent_id),
            created_by_user_id: Some(context.user_id),
            source_conversation_id: clean_optional(&context.conversation_id),
            source_acp_session_id: source_acp_session_id(context),
            source_channel: serde_json::to_value(&context.channel)?,
            plan_id: args.plan_id,
            expected_version: args.expected_version,
            update: WorkPlanUpdate {
                title: args.title,
                summary: args.summary,
                visibility: args.visibility,
                status: args.status,
                items: args.items,
                workspace_context: args.workspace_context,
            },
        },
    )
    .await?;
    let plan = row
        .project_for_role(role, context.user_id)?
        .ok_or_else(|| {
            CustomError::System("updated work plan was not visible to its owner".to_string())
        })?;
    Ok(json!({
        "domain": "activity",
        "bear_id": context.bear_id,
        "activity": activity_payload(Some(&plan)),
        "plan": plan,
    }))
}

fn merge_memory_entry_source_with_human(
    source: Option<Value>,
    context: &DenToolInvocationContext,
    current_user: Option<&user::User>,
) -> Option<Value> {
    let mut source_obj = source
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    source_obj.insert(
        "human".to_string(),
        json!({
            "user_id": context.user_id,
            "username": current_user
                .map(|user| user.username.clone())
                .or_else(|| context.username.clone()),
            "display_name": current_user.map(|user| user.display_name.clone()),
            "membership_role": context.membership_role,
            "authenticated_by": "acp_token"
        }),
    );
    source_obj.insert(
        "session".to_string(),
        json!({
            "conversation_id": clean_optional(&context.conversation_id),
            "session_id": clean_optional(&context.session_id),
            "acp_session_id": context.acp_session_id,
            "conversation_selection": context.conversation_selection,
            "runtime_target": context.runtime_target,
            "request_id": context.request_id,
        }),
    );
    Some(Value::Object(source_obj))
}

fn memfs_http_client(error_prefix: &str) -> Result<reqwest::Client, CustomError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| CustomError::System(format!("{error_prefix}: {e}")))
}

fn validate_memory_kind(value: &str) -> Result<String, CustomError> {
    let kind = value.trim().to_ascii_lowercase();
    match kind.as_str() {
        "note" | "log" | "decision" | "reflection" | "scratch" | "summary" | "plan" => Ok(kind),
        _ => Err(CustomError::ValidationError(
            "kind must be one of note, log, decision, reflection, scratch, summary, plan"
                .to_string(),
        )),
    }
}

pub(crate) fn validate_memory_write_entry_semantics(
    args: &MemoryWriteEntryArguments,
) -> Result<String, CustomError> {
    let kind = validate_memory_kind(&args.kind)?;
    reject_unlabeled_memory_misuse(args)?;
    if kind == "plan" {
        return Err(CustomError::ValidationError(
            "This content appears to be a workplan; use update_plan for visible activity plans and exit_plan_mode for submitted implementation plans instead of memory_write_entry.".to_string(),
        ));
    }
    if let Some(domain) = args.domain.as_deref() {
        match domain {
            "memory" => {}
            "workplan" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a workplan artifact; use plan-mode tools instead of memory_write_entry.".to_string(),
                ));
            }
            "activity" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be live activity state; use update_plan or related activity tools instead of memory_write_entry.".to_string(),
                ));
            }
            "execution" => {
                return Err(CustomError::ValidationError(
                    "This content appears to describe execution or run output rather than semantic memory; use the appropriate execution/result tool instead of memory_write_entry.".to_string(),
                ));
            }
            _ => {}
        }
    }
    if let Some(content_class) = args.content_class.as_deref() {
        match content_class {
            "semantic_memory" => {}
            "workplan_artifact" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a workplan artifact; use plan-mode tools instead of memory_write_entry.".to_string(),
                ));
            }
            "activity_status" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be live activity state; use update_plan instead of memory_write_entry.".to_string(),
                ));
            }
            "task_intent" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a task intent; use request_work_handoff or task-intent tools instead of memory_write_entry.".to_string(),
                ));
            }
            "run_result" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a run result; use the run-result tool instead of memory_write_entry.".to_string(),
                ));
            }
            "observation" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be an observation; use the observation tool instead of memory_write_entry.".to_string(),
                ));
            }
            "core_update" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a core update; use memory review or core-update tools instead of memory_write_entry.".to_string(),
                ));
            }
            "cabinet_write" => {
                return Err(CustomError::ValidationError(
                    "This content appears to be a Cabinet write; use the appropriate Cabinet or reviewed update path instead of memory_write_entry.".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(kind)
}

fn reject_unlabeled_memory_misuse(args: &MemoryWriteEntryArguments) -> Result<(), CustomError> {
    let title = args.title.trim();
    let body = args.body.trim();
    let haystack = format!("{}\n{}", title, body).to_ascii_lowercase();
    let lines = body.lines().map(str::trim).collect::<Vec<_>>();

    if looks_like_workplan_content(&haystack, &lines) {
        return Err(CustomError::ValidationError(
            "This content appears to be an active workplan or implementation plan; use enter_plan_mode/exit_plan_mode for approval plans or update_plan for visible activity tracking instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_activity_or_task_content(&haystack, &lines) {
        return Err(CustomError::ValidationError(
            "This content appears to be task tracking or a task intent; use update_plan or request_work_handoff instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_run_result_content(&haystack) {
        return Err(CustomError::ValidationError(
            "This content appears to be a run result or command output; use the appropriate execution/result tool instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_observation_content(&haystack) {
        return Err(CustomError::ValidationError(
            "This content appears to be an operational observation; use the observation tool instead of memory_write_entry.".to_string(),
        ));
    }
    Ok(())
}

fn looks_like_workplan_content(haystack: &str, lines: &[&str]) -> bool {
    contains_any(
        haystack,
        &[
            "implementation plan",
            "execution plan",
            "workplan",
            "work plan",
            "plan of record",
            "approval plan",
            "proposed plan",
            "submit this plan",
            "once approved",
            "awaiting approval",
        ],
    ) || (contains_any(haystack, &["plan", "steps", "phase"])
        && checkbox_or_numbered_item_count(lines) >= 3)
}

fn looks_like_activity_or_task_content(haystack: &str, lines: &[&str]) -> bool {
    contains_any(
        haystack,
        &[
            "todo:",
            "to-do:",
            "task list",
            "tasks:",
            "current task",
            "current item",
            "in progress",
            "blocked:",
            "next steps:",
            "handoff request",
            "task intent",
            "request work handoff",
        ],
    ) || checkbox_or_numbered_item_count(lines) >= 3
}

fn looks_like_run_result_content(haystack: &str) -> bool {
    contains_any(
        haystack,
        &[
            "command output",
            "run result",
            "test result",
            "test results",
            "cargo test",
            "cargo check",
            "npm test",
            "pytest",
            "exit code",
            "exit status",
            "stdout",
            "stderr",
            "stack trace",
            "failed tests",
            "test failed",
            "tests passed",
        ],
    )
}

fn looks_like_observation_content(haystack: &str) -> bool {
    contains_any(
        haystack,
        &[
            "observation:",
            "observed:",
            "i observed",
            "watch observed",
            "monitoring observed",
            "detected:",
            "incident:",
            "alert:",
            "telemetry",
            "metric spike",
        ],
    )
}

fn checkbox_or_numbered_item_count(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("- [ ]")
                || line.starts_with("- [x]")
                || line.starts_with("* [ ]")
                || line.starts_with("* [x]")
                || line.starts_with("- todo")
                || line.starts_with("- task")
                || line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && line.contains(". ")
        })
        .count()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn validate_bounded_text(
    field: &str,
    value: &str,
    min_chars: usize,
    max_chars: usize,
) -> Result<String, CustomError> {
    let trimmed = value.trim();
    let char_count = trimmed.chars().count();
    if char_count < min_chars {
        return Err(CustomError::ValidationError(format!(
            "{field} must not be empty"
        )));
    }
    if char_count > max_chars {
        return Err(CustomError::ValidationError(format!(
            "{field} must be at most {max_chars} characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn clean_limited_strings(values: Vec<String>, max_items: usize, max_chars: usize) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(max_chars).collect::<String>())
        .take(max_items)
        .collect()
}

fn validate_optional_object(field: &str, value: &Option<Value>) -> Result<(), CustomError> {
    if let Some(value) = value {
        if !value.is_object() {
            return Err(CustomError::ValidationError(format!(
                "{field} must be an object"
            )));
        }
    }
    Ok(())
}

fn memory_read_scopes(role: BearAgentRole) -> Vec<&'static str> {
    match role {
        BearAgentRole::Pair => vec!["pair/", "core/"],
        BearAgentRole::Talk => vec!["talk/", "core/"],
        BearAgentRole::Curate => vec!["talk/", "pair/", "curate/", "work/", "watch/", "core/"],
        BearAgentRole::Work => vec!["work/", "core/"],
        BearAgentRole::Watch => vec!["watch/", "core/"],
    }
}

fn memory_write_scopes(role: BearAgentRole) -> Vec<&'static str> {
    match role {
        BearAgentRole::Pair => vec![
            "pair/notes/",
            "pair/logs/",
            "pair/decisions/",
            "pair/reflections/",
            "pair/scratch/",
            "pair/summaries/",
        ],
        _ => Vec::new(),
    }
}

fn validate_public_http_url(raw: &str) -> Result<url::Url, CustomError> {
    let url = url::Url::parse(raw.trim()).map_err(|e| {
        CustomError::ValidationError(format!("url must be a valid HTTP(S) URL: {e}"))
    })?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(CustomError::ValidationError(
                "url scheme must be http or https".to_string(),
            ));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| CustomError::ValidationError("url must include a host".to_string()))?;
    let lower_host = host.trim_end_matches('.').to_ascii_lowercase();
    if lower_host == "localhost" || lower_host.ends_with(".localhost") {
        return Err(CustomError::ValidationError(
            "localhost URLs are not allowed for den.web.fetch".to_string(),
        ));
    }
    if let Ok(ip) = lower_host.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(CustomError::ValidationError(
                "private, loopback, link-local, multicast, and unspecified IP URLs are not allowed for den.web.fetch".to_string(),
            ));
        }
        return Ok(url);
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = (host, port).to_socket_addrs().map_err(|e| {
        CustomError::ValidationError(format!("url host could not be resolved safely: {e}"))
    })?;
    for addr in addrs {
        if !is_public_ip(addr.ip()) {
            return Err(CustomError::ValidationError(format!(
                "url host resolves to a non-public address: {}",
                addr.ip()
            )));
        }
    }
    Ok(url)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0)
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.segments()[0] & 0xfe00 == 0xfc00
                || ip.segments()[0] & 0xffc0 == 0xfe80)
        }
    }
}

fn html_to_text_excerpt(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len().min(64_000));
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    let mut out = String::new();
    let mut truncated = false;
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            truncated = true;
            break;
        }
        out.push(ch);
    }
    (out, truncated)
}

fn source_acp_session_id(context: &DenToolInvocationContext) -> Option<String> {
    let is_acp = [
        context.channel.family.as_deref(),
        context.channel.client.as_deref(),
        context.channel.protocol.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_ascii_lowercase().contains("acp"));
    if is_acp {
        clean_optional(&context.session_id)
    } else {
        None
    }
}

fn clean_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_rfc3339(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn names_for_role(role: BearAgentRole) -> HashSet<&'static str> {
        builtin_den_tool_descriptors_for_role(role)
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect()
    }

    #[test]
    fn provider_names_are_safe_and_unique() {
        let descriptors = builtin_den_tool_descriptors();
        let mut provider_names = HashSet::new();
        for descriptor in descriptors {
            assert!(
                descriptor
                    .provider_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "provider name must be Letta/provider-safe: {}",
                descriptor.provider_name
            );
            assert!(!descriptor.provider_name.contains('.'));
            assert!(!descriptor.provider_name.contains('/'));
            assert!(
                provider_names.insert(descriptor.provider_name.clone()),
                "duplicate provider name: {}",
                descriptor.provider_name
            );
        }
    }

    #[test]
    fn canonical_dotted_names_map_to_provider_safe_aliases() {
        let descriptors = builtin_den_tool_descriptors();
        let task = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_TASK_WRITE_INTENT)
            .expect("task intent descriptor exists");
        assert_eq!(task.provider_name, "den_task_write_intent");

        let skill = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_SKILL_PROPOSE)
            .expect("skill proposal descriptor exists");
        assert_eq!(skill.provider_name, "den_skill_propose");

        let conversation_title = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_CONVERSATION_SET_TITLE)
            .expect("conversation title descriptor exists");
        assert_eq!(
            conversation_title.provider_name,
            DEN_CONVERSATION_SET_TITLE_PROVIDER
        );

        let web_fetch = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_WEB_FETCH)
            .expect("web fetch descriptor exists");
        assert_eq!(web_fetch.provider_name, DEN_WEB_FETCH_PROVIDER);

        let web_search = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_WEB_SEARCH)
            .expect("web search descriptor exists");
        assert_eq!(web_search.provider_name, DEN_WEB_SEARCH_PROVIDER);

        let situation = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_SITUATION_GET)
            .expect("situation descriptor exists");
        assert_eq!(situation.provider_name, DEN_SITUATION_GET_PROVIDER);
        assert_eq!(situation.provider_name, "session_info");
        assert_ne!(situation.provider_name, "situation_get");
        assert_ne!(situation.provider_name, "den_situation_get");

        let memory_browse = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_MEMORY_TREE)
            .expect("memory browse descriptor exists");
        assert_eq!(memory_browse.provider_name, DEN_MEMORY_TREE_PROVIDER);
        assert_eq!(memory_browse.provider_name, "memory_browse");
        assert_ne!(memory_browse.provider_name, "memory_tree");

        let memory = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_MEMORY_WRITE_ENTRY)
            .expect("memory write descriptor exists");
        assert_eq!(memory.provider_name, DEN_MEMORY_WRITE_ENTRY_PROVIDER);

        let update_plan = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_WORK_PLAN_UPDATE)
            .expect("work plan update descriptor exists");
        assert_eq!(update_plan.provider_name, DEN_WORK_PLAN_UPDATE_PROVIDER);
        assert_eq!(update_plan.provider_name, "update_plan");

        let enter_plan_mode = descriptors
            .iter()
            .find(|descriptor| descriptor.name == DEN_PLAN_MODE_ENTER)
            .expect("enter plan mode descriptor exists");
        assert_eq!(enter_plan_mode.provider_name, DEN_PLAN_MODE_ENTER_PROVIDER);
        assert_eq!(enter_plan_mode.provider_name, "enter_plan_mode");
    }

    #[test]
    fn den_server_tools_advertise_semantic_aliases_not_legacy_den_prefixes() {
        let provider_names = builtin_den_tool_descriptors_for_role(BearAgentRole::Pair)
            .into_iter()
            .map(|descriptor| descriptor.provider_name)
            .collect::<HashSet<_>>();
        assert!(provider_names.contains("session_info"));
        assert!(provider_names.contains("set_conversation_title"));
        assert!(provider_names.contains("web_search"));
        assert!(provider_names.contains("memory_browse"));
        assert!(provider_names.contains("memory_read"));
        assert!(provider_names.contains("update_plan"));
        assert!(provider_names.contains("enter_plan_mode"));
        assert!(provider_names.contains("record_plan_approval"));
        assert!(provider_names.contains("exit_plan_mode"));
        assert!(provider_names.contains("cancel_plan_mode"));
        assert!(!provider_names.contains("situation_get"));
        assert!(!provider_names.contains("memory_tree"));
        assert!(!provider_names.contains("den_situation_get"));
        assert!(!provider_names.contains("den_web_search"));
        assert!(!provider_names.contains("den_memory_read"));
        assert!(!provider_names.contains("den_work_plan_update"));
        assert!(!provider_names.contains("den_plan_mode_enter"));
    }

    #[test]
    fn privileged_descriptors_are_role_scoped() {
        let talk = names_for_role(BearAgentRole::Talk);
        assert!(talk.contains(DEN_TASK_WRITE_INTENT));
        assert!(talk.contains(DEN_SKILL_PROPOSE));
        assert!(!talk.contains(DEN_OBSERVATION_WRITE));
        assert!(!talk.contains(DEN_RUN_WRITE_RESULT));

        let pair = names_for_role(BearAgentRole::Pair);
        assert!(pair.contains(DEN_TASK_WRITE_INTENT));
        assert!(pair.contains(DEN_WORK_PLAN_UPDATE));
        assert!(pair.contains(DEN_WORK_PLAN_REQUEST_HANDOFF));
        assert!(pair.contains(DEN_SKILL_PROPOSE));
        assert!(!pair.contains(DEN_OBSERVATION_WRITE));
        assert!(!pair.contains(DEN_RUN_WRITE_RESULT));

        let curate = names_for_role(BearAgentRole::Curate);
        assert!(curate.contains(DEN_TASK_APPROVE_INTENT));
        assert!(curate.contains(DEN_TASK_REJECT_INTENT));
        assert!(curate.contains(DEN_CORE_WRITE_RESULT_SUMMARY));
        assert!(curate.contains(DEN_SKILL_APPROVE_PROPOSAL));
        assert!(curate.contains(DEN_SKILL_REJECT_PROPOSAL));
        assert!(curate.contains(DEN_SKILL_PROPOSE));
        assert!(!curate.contains(DEN_TASK_WRITE_INTENT));
        assert!(!curate.contains(DEN_OBSERVATION_WRITE));
        assert!(!curate.contains(DEN_RUN_WRITE_RESULT));

        let watch = names_for_role(BearAgentRole::Watch);
        assert!(watch.contains(DEN_OBSERVATION_WRITE));
        assert!(watch.contains(DEN_SKILL_PROPOSE));
        assert!(!watch.contains(DEN_WORK_PLAN_LIST));
        assert!(!watch.contains(DEN_WORK_PLAN_UPDATE));
        assert!(!watch.contains(DEN_TASK_WRITE_INTENT));
        assert!(!watch.contains(DEN_RUN_WRITE_RESULT));

        let work = names_for_role(BearAgentRole::Work);
        assert!(work.contains(DEN_RUN_WRITE_RESULT));
        assert!(work.contains(DEN_WORK_PLAN_LIST));
        assert!(work.contains(DEN_WORK_PLAN_UPDATE));
        assert!(!work.contains(DEN_WORK_PLAN_REQUEST_HANDOFF));
        assert!(work.contains(DEN_SKILL_PROPOSE));
        assert!(!work.contains(DEN_TASK_WRITE_INTENT));
        assert!(!work.contains(DEN_OBSERVATION_WRITE));
    }

    #[test]
    fn all_descriptors_are_known_tools() {
        for descriptor in builtin_den_tool_descriptors() {
            assert!(is_builtin_den_tool(descriptor.name));
        }
    }

    #[test]
    fn pair_has_web_memory_and_activity_tools() {
        let pair = names_for_role(BearAgentRole::Pair);
        assert!(pair.contains(DEN_CONVERSATION_SET_TITLE));
        assert!(pair.contains(DEN_WEB_FETCH));
        assert!(pair.contains(DEN_WEB_SEARCH));
        assert!(pair.contains(DEN_SITUATION_GET));
        assert!(pair.contains(DEN_MEMORY_WRITE_ENTRY));
        assert!(pair.contains(DEN_MEMORY_STATUS));
        assert!(pair.contains(DEN_MEMORY_TREE));
        assert!(pair.contains(DEN_MEMORY_READ));
        assert!(pair.contains(DEN_MEMORY_SEARCH));
        assert!(pair.contains(DEN_WORK_PLAN_LIST));
        assert!(pair.contains(DEN_WORK_PLAN_GET_STATUS));
        assert!(pair.contains(DEN_WORK_PLAN_UPDATE));
        assert!(pair.contains(DEN_WORK_PLAN_REQUEST_HANDOFF));
        assert!(pair.contains(DEN_PLAN_MODE_ENTER));
        assert!(pair.contains(DEN_PLAN_MODE_STATUS));
        assert!(pair.contains(DEN_PLAN_MODE_RECORD_APPROVAL));
        assert!(pair.contains(DEN_PLAN_MODE_EXIT));
        assert!(pair.contains(DEN_PLAN_MODE_CANCEL));

        let talk = names_for_role(BearAgentRole::Talk);
        assert!(talk.contains(DEN_CONVERSATION_SET_TITLE));
        assert!(!talk.contains(DEN_WEB_FETCH));
        assert!(!talk.contains(DEN_WEB_SEARCH));
        assert!(!talk.contains(DEN_MEMORY_WRITE_ENTRY));
    }

    #[tokio::test]
    async fn web_search_reports_missing_provider_config() {
        let config = Config::test_stub();
        let err = web_search_inner(None, &config, None, json!({ "query": "rust docs" }))
            .await
            .expect_err("missing provider should fail clearly");
        assert!(err.to_string().contains("DEN_SEARCH_PROVIDER"));
    }

    #[test]
    fn role_authorization_rejects_disallowed_tools() {
        assert!(authorize_tool_for_role(DEN_TASK_WRITE_INTENT, BearAgentRole::Talk).is_ok());
        assert!(authorize_tool_for_role(DEN_TASK_WRITE_INTENT, BearAgentRole::Watch).is_err());
        assert!(authorize_tool_for_role(DEN_RUN_WRITE_RESULT, BearAgentRole::Work).is_ok());
        assert!(authorize_tool_for_role(DEN_RUN_WRITE_RESULT, BearAgentRole::Talk).is_err());
        assert!(authorize_tool_for_role(DEN_TASK_APPROVE_INTENT, BearAgentRole::Curate).is_ok());
        assert!(authorize_tool_for_role(DEN_TASK_APPROVE_INTENT, BearAgentRole::Pair).is_err());
        assert!(authorize_tool_for_role(DEN_SKILL_APPROVE_PROPOSAL, BearAgentRole::Curate).is_ok());
        assert!(authorize_tool_for_role(DEN_SKILL_APPROVE_PROPOSAL, BearAgentRole::Work).is_err());
        assert!(authorize_tool_for_role(DEN_WORK_PLAN_UPDATE, BearAgentRole::Pair).is_ok());
        assert!(authorize_tool_for_role(DEN_WORK_PLAN_UPDATE, BearAgentRole::Watch).is_err());
        assert!(
            authorize_tool_for_role(DEN_WORK_PLAN_REQUEST_HANDOFF, BearAgentRole::Work).is_err()
        );
    }
}
