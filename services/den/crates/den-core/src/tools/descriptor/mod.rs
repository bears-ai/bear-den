use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const ALL_PROFILES: &[&str] = &["chat", "pair", "curate", "work", "watch"];
const TASK_LIST_READ_PROFILES: &[&str] = &["chat", "pair", "curate", "work"];
const TASK_LIST_UPDATE_PROFILES: &[&str] = &["chat", "pair", "work"];
const CHAT_AND_PAIR_PROFILES: &[&str] = &["chat", "pair"];
const PAIR_PROFILES: &[&str] = &["pair"];
const MEMORY_READ_PROFILES: &[&str] = &["chat", "pair", "curate", "work", "watch"];
const ENTITY_RELATION_WRITE_PROFILES: &[&str] = &["chat", "pair", "work", "watch"];
const CURATE_PROFILES: &[&str] = &["curate"];
const WATCH_PROFILES: &[&str] = &["watch"];
const WORK_PROFILES: &[&str] = &["work"];

use crate::BearProfile;

use crate::tools::{
    constants::{
        DEN_BEAR_ENVIRONMENT, DEN_BEAR_ENVIRONMENT_PROVIDER, DEN_BEAR_GET_SELF,
        DEN_BEAR_LIST_MEMBERS, DEN_CABINET_CREATE, DEN_CABINET_CREATE_PROVIDER,
        DEN_CABINET_HISTORY, DEN_CABINET_HISTORY_PROVIDER, DEN_CABINET_LIFECYCLE,
        DEN_CABINET_LIFECYCLE_PROVIDER, DEN_CABINET_READ, DEN_CABINET_READ_PROVIDER,
        DEN_CABINET_SEARCH, DEN_CABINET_SEARCH_PROVIDER, DEN_CABINET_SOURCE_LINK,
        DEN_CABINET_SOURCE_LINK_PROVIDER, DEN_CABINET_UPDATE, DEN_CABINET_UPDATE_PROVIDER,
        DEN_CAPABILITIES_LIST_SELF, DEN_CAPABILITY_DESCRIBE, DEN_CAPABILITY_DESCRIBE_PROVIDER,
        DEN_CAPABILITY_SEARCH, DEN_CAPABILITY_SEARCH_PROVIDER, DEN_CHANNEL_GET_CONTEXT,
        DEN_CONVERSATION_SET_TITLE, DEN_CONVERSATION_SET_TITLE_PROVIDER,
        DEN_CORE_WRITE_RESULT_SUMMARY, DEN_DOCKET_ENTRY_APPEND, DEN_DOCKET_ENTRY_APPEND_PROVIDER,
        DEN_DOCKET_ENTRY_LIST, DEN_DOCKET_ENTRY_LIST_PROVIDER, DEN_DOCKET_ENTRY_PROMOTE,
        DEN_DOCKET_ENTRY_PROMOTE_PROVIDER, DEN_ENTITY_BROWSE, DEN_ENTITY_BROWSE_PROVIDER,
        DEN_ENTITY_LINK_MEMORY, DEN_ENTITY_LINK_MEMORY_PROVIDER, DEN_ENTITY_MERGE,
        DEN_ENTITY_MERGE_PROVIDER, DEN_ENTITY_RESOLVE, DEN_ENTITY_RESOLVE_PROVIDER,
        DEN_ENTITY_SPLIT, DEN_ENTITY_SPLIT_PROVIDER, DEN_ENTITY_WRITE_ACCESS_RULE,
        DEN_ENTITY_WRITE_ACCESS_RULE_PROVIDER, DEN_ENTITY_WRITE_ANCHOR,
        DEN_ENTITY_WRITE_ANCHOR_PROVIDER, DEN_JOB_ARCHIVE, DEN_JOB_ARCHIVE_PROVIDER,
        DEN_JOB_CANCEL, DEN_JOB_CANCEL_PROVIDER, DEN_JOB_CANCEL_RUN, DEN_JOB_CANCEL_RUN_PROVIDER,
        DEN_JOB_CREATE, DEN_JOB_CREATE_PROVIDER, DEN_JOB_EVALUATE_CRITERION,
        DEN_JOB_EVALUATE_CRITERION_PROVIDER, DEN_JOB_EXECUTE, DEN_JOB_EXECUTE_PROVIDER,
        DEN_JOB_FIND, DEN_JOB_FIND_PROVIDER, DEN_JOB_GET, DEN_JOB_GET_PROVIDER, DEN_JOB_LIST,
        DEN_JOB_LIST_PROVIDER, DEN_JOB_RECONCILE, DEN_JOB_RECONCILE_PROVIDER, DEN_JOB_SETTLE_TASK,
        DEN_JOB_SETTLE_TASK_PROVIDER, DEN_JOB_UPDATE, DEN_JOB_UPDATE_PROVIDER,
        DEN_MEMORY_APPLY_CORE_UPDATE, DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER,
        DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD, DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD_PROVIDER,
        DEN_MEMORY_LIST_PROPOSALS, DEN_MEMORY_LIST_PROPOSALS_PROVIDER, DEN_MEMORY_MARK_LIFECYCLE,
        DEN_MEMORY_MARK_LIFECYCLE_PROVIDER, DEN_MEMORY_ORIENT_WORK_SURFACE,
        DEN_MEMORY_ORIENT_WORK_SURFACE_PROVIDER, DEN_MEMORY_READ, DEN_MEMORY_READ_PROPOSAL,
        DEN_MEMORY_READ_PROPOSAL_PROVIDER, DEN_MEMORY_READ_PROVIDER, DEN_MEMORY_REQUEST_REVIEW,
        DEN_MEMORY_REQUEST_REVIEW_PROVIDER, DEN_MEMORY_RESOLVE_PROPOSAL,
        DEN_MEMORY_RESOLVE_PROPOSAL_PROVIDER, DEN_MEMORY_SEARCH, DEN_MEMORY_SEARCH_PROVIDER,
        DEN_MEMORY_STATUS, DEN_MEMORY_STATUS_PROVIDER, DEN_MEMORY_TREE,
        DEN_MEMORY_TREE_LEGACY_PROVIDER, DEN_MEMORY_TREE_PROVIDER, DEN_MEMORY_WRITE_ENTRY,
        DEN_MEMORY_WRITE_ENTRY_PROVIDER, DEN_OBSERVATION_WRITE, DEN_PLAN_MODE_CANCEL,
        DEN_PLAN_MODE_CANCEL_PROVIDER, DEN_PLAN_MODE_ENTER, DEN_PLAN_MODE_ENTER_PROVIDER,
        DEN_PLAN_MODE_EXIT, DEN_PLAN_MODE_EXIT_PROVIDER, DEN_PLAN_MODE_RECORD_APPROVAL,
        DEN_PLAN_MODE_RECORD_APPROVAL_PROVIDER, DEN_PLAN_MODE_STATUS,
        DEN_PLAN_MODE_STATUS_PROVIDER, DEN_POLICY_GET_SELF, DEN_PROMPT_MEMORY_LIST,
        DEN_PROMPT_MEMORY_LIST_PROVIDER, DEN_PROMPT_MEMORY_PATCH, DEN_PROMPT_MEMORY_PATCH_PROVIDER,
        DEN_PROMPT_MEMORY_UPSERT, DEN_PROMPT_MEMORY_UPSERT_PROVIDER, DEN_RUNTIME_DIAGNOSTICS_LIST,
        DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER, DEN_RUN_WRITE_RESULT, DEN_SITUATION_GET,
        DEN_SITUATION_GET_LEGACY_PROVIDER, DEN_SITUATION_GET_PROVIDER, DEN_SKILL_APPROVE_PROPOSAL,
        DEN_SKILL_PROPOSE, DEN_SKILL_REJECT_PROPOSAL, DEN_TASK_APPROVE_INTENT, DEN_TASK_CREATE,
        DEN_TASK_CREATE_PROVIDER, DEN_TASK_FIND, DEN_TASK_FIND_PROVIDER, DEN_TASK_FOCUS,
        DEN_TASK_FOCUS_PROVIDER, DEN_TASK_LIST, DEN_TASK_LISTS_GET_STATUS,
        DEN_TASK_LISTS_GET_STATUS_PROVIDER, DEN_TASK_LISTS_LIST, DEN_TASK_LISTS_LIST_PROVIDER,
        DEN_TASK_LISTS_REQUEST_HANDOFF, DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER,
        DEN_TASK_LISTS_UPDATE, DEN_TASK_LISTS_UPDATE_PROVIDER, DEN_TASK_LIST_CHECKOUT,
        DEN_TASK_LIST_CHECKOUT_PROVIDER, DEN_TASK_LIST_PROVIDER, DEN_TASK_LIST_SYNC,
        DEN_TASK_LIST_SYNC_PROVIDER, DEN_TASK_REJECT_INTENT, DEN_TASK_SELECT,
        DEN_TASK_SELECT_PROVIDER, DEN_TASK_UPDATE, DEN_TASK_UPDATE_CURRENT_STATUS,
        DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER, DEN_TASK_UPDATE_PROVIDER, DEN_TASK_WRITE_INTENT,
        DEN_TOOL_OUTPUT_READ, DEN_TOOL_OUTPUT_READ_PROVIDER, DEN_USER_GET_CURRENT, DEN_WEB_FETCH,
        DEN_WEB_FETCH_LEGACY_PROVIDER, DEN_WEB_FETCH_PROVIDER, DEN_WEB_SEARCH,
        DEN_WEB_SEARCH_PROVIDER, DEN_WORK_CATALOG, DEN_WORK_CATALOG_PROVIDER, DEN_WORK_DISPATCH,
        DEN_WORK_DISPATCH_PROVIDER, DEN_WORK_PREPARE_RUST_DEPENDENCIES,
        DEN_WORK_PREPARE_RUST_DEPENDENCIES_PROVIDER, DEN_WORK_RUN_CANCEL,
        DEN_WORK_RUN_CANCEL_PROVIDER, DEN_WORK_RUN_FIND, DEN_WORK_RUN_FIND_PROVIDER,
        DEN_WORK_RUN_GET, DEN_WORK_RUN_GET_PROVIDER, DEN_WORK_RUN_LIST, DEN_WORK_RUN_LIST_PROVIDER,
        DEN_WORK_RUN_RESOLVE_STALLED, DEN_WORK_RUN_RESOLVE_STALLED_PROVIDER,
        DEN_WORK_SURFACE_CONFIRM, DEN_WORK_SURFACE_CONFIRM_PROVIDER,
    },
    display::ToolDisplayDescriptor,
    tool_descriptor_guidance::{
        render_tool_descriptor_guidance, ToolDescriptorGuidance, ToolOrientationPolicy,
        ToolScopeKind, ToolSideEffectKind,
    },
};

#[derive(Debug, Clone, Serialize)]
pub struct DenToolDescriptor {
    pub name: &'static str,
    pub provider_name: String,
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
    pub display: serde_json::Value,
    pub input_schema: Value,
}

pub fn provider_safe_tool_name(name: &str) -> String {
    match name {
        DEN_CAPABILITY_SEARCH => return DEN_CAPABILITY_SEARCH_PROVIDER.to_string(),
        DEN_CAPABILITY_DESCRIBE => return DEN_CAPABILITY_DESCRIBE_PROVIDER.to_string(),
        DEN_CONVERSATION_SET_TITLE => return DEN_CONVERSATION_SET_TITLE_PROVIDER.to_string(),
        DEN_WEB_FETCH => return DEN_WEB_FETCH_PROVIDER.to_string(),
        DEN_WEB_SEARCH => return DEN_WEB_SEARCH_PROVIDER.to_string(),
        DEN_TOOL_OUTPUT_READ => return DEN_TOOL_OUTPUT_READ_PROVIDER.to_string(),
        DEN_BEAR_ENVIRONMENT => return DEN_BEAR_ENVIRONMENT_PROVIDER.to_string(),
        DEN_SITUATION_GET => return DEN_SITUATION_GET_PROVIDER.to_string(),
        DEN_MEMORY_WRITE_ENTRY => return DEN_MEMORY_WRITE_ENTRY_PROVIDER.to_string(),
        DEN_MEMORY_STATUS => return DEN_MEMORY_STATUS_PROVIDER.to_string(),
        DEN_MEMORY_TREE => return DEN_MEMORY_TREE_PROVIDER.to_string(),
        DEN_MEMORY_READ => return DEN_MEMORY_READ_PROVIDER.to_string(),
        DEN_MEMORY_SEARCH => return DEN_MEMORY_SEARCH_PROVIDER.to_string(),
        DEN_ENTITY_BROWSE => return DEN_ENTITY_BROWSE_PROVIDER.to_string(),
        DEN_ENTITY_RESOLVE => return DEN_ENTITY_RESOLVE_PROVIDER.to_string(),
        DEN_ENTITY_LINK_MEMORY => return DEN_ENTITY_LINK_MEMORY_PROVIDER.to_string(),
        DEN_ENTITY_MERGE => return DEN_ENTITY_MERGE_PROVIDER.to_string(),
        DEN_ENTITY_SPLIT => return DEN_ENTITY_SPLIT_PROVIDER.to_string(),
        DEN_ENTITY_WRITE_ACCESS_RULE => return DEN_ENTITY_WRITE_ACCESS_RULE_PROVIDER.to_string(),
        DEN_ENTITY_WRITE_ANCHOR => return DEN_ENTITY_WRITE_ANCHOR_PROVIDER.to_string(),
        DEN_MEMORY_ORIENT_WORK_SURFACE => {
            return DEN_MEMORY_ORIENT_WORK_SURFACE_PROVIDER.to_string();
        }
        DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD => {
            return DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD_PROVIDER.to_string();
        }
        DEN_MEMORY_REQUEST_REVIEW => return DEN_MEMORY_REQUEST_REVIEW_PROVIDER.to_string(),
        DEN_PROMPT_MEMORY_UPSERT => return DEN_PROMPT_MEMORY_UPSERT_PROVIDER.to_string(),
        DEN_PROMPT_MEMORY_LIST => return DEN_PROMPT_MEMORY_LIST_PROVIDER.to_string(),
        DEN_PROMPT_MEMORY_PATCH => return DEN_PROMPT_MEMORY_PATCH_PROVIDER.to_string(),
        DEN_MEMORY_LIST_PROPOSALS => return DEN_MEMORY_LIST_PROPOSALS_PROVIDER.to_string(),
        DEN_MEMORY_READ_PROPOSAL => return DEN_MEMORY_READ_PROPOSAL_PROVIDER.to_string(),
        DEN_MEMORY_RESOLVE_PROPOSAL => return DEN_MEMORY_RESOLVE_PROPOSAL_PROVIDER.to_string(),
        DEN_MEMORY_APPLY_CORE_UPDATE => return DEN_MEMORY_APPLY_CORE_UPDATE_PROVIDER.to_string(),
        DEN_MEMORY_MARK_LIFECYCLE => return DEN_MEMORY_MARK_LIFECYCLE_PROVIDER.to_string(),
        DEN_TASK_LISTS_LIST => return DEN_TASK_LISTS_LIST_PROVIDER.to_string(),
        DEN_TASK_LISTS_GET_STATUS => return DEN_TASK_LISTS_GET_STATUS_PROVIDER.to_string(),
        DEN_TASK_LISTS_UPDATE => return DEN_TASK_LISTS_UPDATE_PROVIDER.to_string(),
        DEN_TASK_LISTS_REQUEST_HANDOFF => {
            return DEN_TASK_LISTS_REQUEST_HANDOFF_PROVIDER.to_string()
        }
        DEN_JOB_CREATE => return DEN_JOB_CREATE_PROVIDER.to_string(),
        DEN_JOB_LIST => return DEN_JOB_LIST_PROVIDER.to_string(),
        DEN_JOB_GET => return DEN_JOB_GET_PROVIDER.to_string(),
        DEN_JOB_FIND => return DEN_JOB_FIND_PROVIDER.to_string(),
        DEN_JOB_UPDATE => return DEN_JOB_UPDATE_PROVIDER.to_string(),
        DEN_JOB_CANCEL => return DEN_JOB_CANCEL_PROVIDER.to_string(),
        DEN_JOB_CANCEL_RUN => return DEN_JOB_CANCEL_RUN_PROVIDER.to_string(),
        DEN_JOB_ARCHIVE => return DEN_JOB_ARCHIVE_PROVIDER.to_string(),
        DEN_JOB_EXECUTE => return DEN_JOB_EXECUTE_PROVIDER.to_string(),
        DEN_JOB_RECONCILE => return DEN_JOB_RECONCILE_PROVIDER.to_string(),
        DEN_JOB_SETTLE_TASK => return DEN_JOB_SETTLE_TASK_PROVIDER.to_string(),
        DEN_JOB_EVALUATE_CRITERION => return DEN_JOB_EVALUATE_CRITERION_PROVIDER.to_string(),
        DEN_TASK_CREATE => return DEN_TASK_CREATE_PROVIDER.to_string(),
        DEN_TASK_LIST => return DEN_TASK_LIST_PROVIDER.to_string(),
        DEN_TASK_FIND => return DEN_TASK_FIND_PROVIDER.to_string(),
        DEN_TASK_UPDATE => return DEN_TASK_UPDATE_PROVIDER.to_string(),
        DEN_TASK_SELECT => return DEN_TASK_SELECT_PROVIDER.to_string(),
        DEN_TASK_FOCUS => return DEN_TASK_FOCUS_PROVIDER.to_string(),
        DEN_TASK_UPDATE_CURRENT_STATUS => {
            return DEN_TASK_UPDATE_CURRENT_STATUS_PROVIDER.to_string()
        }
        DEN_DOCKET_ENTRY_APPEND => return DEN_DOCKET_ENTRY_APPEND_PROVIDER.to_string(),
        DEN_DOCKET_ENTRY_PROMOTE => return DEN_DOCKET_ENTRY_PROMOTE_PROVIDER.to_string(),
        DEN_DOCKET_ENTRY_LIST => return DEN_DOCKET_ENTRY_LIST_PROVIDER.to_string(),
        DEN_RUNTIME_DIAGNOSTICS_LIST => return DEN_RUNTIME_DIAGNOSTICS_LIST_PROVIDER.to_string(),
        DEN_CABINET_SEARCH => return DEN_CABINET_SEARCH_PROVIDER.to_string(),
        DEN_CABINET_READ => return DEN_CABINET_READ_PROVIDER.to_string(),
        DEN_CABINET_CREATE => return DEN_CABINET_CREATE_PROVIDER.to_string(),
        DEN_CABINET_UPDATE => return DEN_CABINET_UPDATE_PROVIDER.to_string(),
        DEN_CABINET_HISTORY => return DEN_CABINET_HISTORY_PROVIDER.to_string(),
        DEN_CABINET_SOURCE_LINK => return DEN_CABINET_SOURCE_LINK_PROVIDER.to_string(),
        DEN_CABINET_LIFECYCLE => return DEN_CABINET_LIFECYCLE_PROVIDER.to_string(),
        DEN_TASK_LIST_SYNC => return DEN_TASK_LIST_SYNC_PROVIDER.to_string(),
        DEN_TASK_LIST_CHECKOUT => return DEN_TASK_LIST_CHECKOUT_PROVIDER.to_string(),
        DEN_WORK_DISPATCH => return DEN_WORK_DISPATCH_PROVIDER.to_string(),
        DEN_WORK_RUN_LIST => return DEN_WORK_RUN_LIST_PROVIDER.to_string(),
        DEN_WORK_RUN_GET => return DEN_WORK_RUN_GET_PROVIDER.to_string(),
        DEN_WORK_RUN_FIND => return DEN_WORK_RUN_FIND_PROVIDER.to_string(),
        DEN_WORK_RUN_CANCEL => return DEN_WORK_RUN_CANCEL_PROVIDER.to_string(),
        DEN_WORK_RUN_RESOLVE_STALLED => return DEN_WORK_RUN_RESOLVE_STALLED_PROVIDER.to_string(),
        DEN_WORK_CATALOG => return DEN_WORK_CATALOG_PROVIDER.to_string(),
        DEN_WORK_SURFACE_CONFIRM => return DEN_WORK_SURFACE_CONFIRM_PROVIDER.to_string(),
        DEN_WORK_PREPARE_RUST_DEPENDENCIES => {
            return DEN_WORK_PREPARE_RUST_DEPENDENCIES_PROVIDER.to_string()
        }
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

pub fn builtin_den_tool_descriptors() -> Vec<DenToolDescriptor> {
    vec![
        descriptor(
            DEN_BEAR_GET_SELF,
            "About this bear",
            "Return Den's trusted profile for the current bear.",
            "bear",
            &["bear.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_USER_GET_CURRENT,
            "Current user",
            "Return Den's trusted profile for the current user in this interaction.",
            "session",
            &["user.current.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_BEAR_LIST_MEMBERS,
            "Bear members",
            "List users who have access to the current bear, with policy redaction.",
            "bear",
            &["bear.members.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_CAPABILITIES_LIST_SELF,
            "Available Den capabilities",
            "List Den-managed tools available to the current bear/session.",
            "session",
            &["capabilities.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_CAPABILITY_SEARCH,
            "Search capability catalog",
            "Search the discoverable Capability Catalog using lexical query text, taxonomy tag, and kind filters. Returns compact results with locality, surface, authority, lifetime, risk, and execution-option metadata. Discovery does not grant invocation authority.",
            "session",
            &["capabilities.read"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"query":{"type":"string"},"tag":{"type":"string"},"kind":{"type":"string","enum":["tool","skill","policy","memory","surface","executor","connector","example","bundle"]},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_CAPABILITY_DESCRIBE,
            "Describe capability",
            "Describe one Capability Catalog entry by ref, canonical tool name, or provider tool name, including locality, surface, authority, lifetime, risk, execution options, and invocation references where available.",
            "session",
            &["capabilities.read"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"ref":{"type":"string","minLength":1}},"required":["ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CHANNEL_GET_CONTEXT,
            "Channel context",
            "Return trusted Den channel and session context for this interaction.",
            "session",
            &["channel.context.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_POLICY_GET_SELF,
            "Current policy",
            "Explain current user and bear policy for this interaction.",
            "session",
            &["policy.read"],
            ALL_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_CONVERSATION_SET_TITLE,
            "Set conversation title",
            "Set the title of the current conversation. In some clients this may appear as the current chat or thread title. Does not change the conversation id, switch conversations, or write Bear memory.",
            "conversation",
            &["conversation.title.write"],
            CHAT_AND_PAIR_PROFILES,
            set_conversation_title_schema(),
        ),
        descriptor(
            DEN_WEB_FETCH,
            "Fetch web page",
            "Fetch an HTTP(S) URL through Den with SSRF guards and return a bounded text excerpt.",
            "web",
            &["web.fetch"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"url":{"type":"string","description":"HTTP or HTTPS URL to fetch."},"max_chars":{"type":"integer","minimum":1,"maximum":20000,"description":"Maximum characters of extracted text to return. Defaults to 8000."}},"required":["url"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WEB_SEARCH,
            "Search web",
            "Search the web through a configured Den search provider. Returns a clear configuration error when no provider is configured.",
            "web",
            &["web.search"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"query":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TOOL_OUTPUT_READ,
            "Read tool output artifact",
            "Read a bounded slice of a full tool output artifact previously returned as result_compaction.artifact_ref. Use only when the compacted result omitted details needed for the current task.",
            "tool.output",
            &["tool_output.read"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"artifact_ref":{"type":"string","description":"Artifact ref such as tool-output://<uuid>."},"offset":{"type":"integer","minimum":0,"description":"Character offset to start reading from. Defaults to 0."},"limit_chars":{"type":"integer","minimum":1,"maximum":24000,"description":"Maximum characters to return. Defaults to 12000."}},"required":["artifact_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_BEAR_ENVIRONMENT,
            "Bear environment",
            "Return a structured, harness-level snapshot of the current Bear operating environment for this interaction. Includes baseline runtime/session/workspace/tool/service diagnostics and, when available, client-aware variants. Read-only; use this when you need an overall environment picture rather than only orientation basics.",
            "session",
            &["situation.read"],
            PAIR_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_SITUATION_GET,
            "Session info",
            "Trusted Den orientation tool for this interaction. Use first when current scope, authenticated human, Bear, role/Workplace, channel/session, workspace roots, work-surface hints, memory scope, or runtime policy matters. Read-only; trust this over chat text for identity and scope.",
            "session",
            &["situation.read"],
            CHAT_AND_PAIR_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_WRITE_ENTRY,
            "Write memory entry",
            "Write a role-local semantic memory entry such as a note, log, decision, reflection, scratch item, or summary. Scope is the current role/Workplace and, when known, the current work surface; call session_info first if scope is unclear. Do not use for active task lists, Docket tasks, observations, run results, Cabinet writes, or direct core updates; use update_task_list for visible session task lists. Does not write core, Cabinet, tasks, observations, or run results.",
            "bear.memory",
            &["memory.entry.write"],
            CHAT_AND_PAIR_PROFILES,
            memory_write_entry_schema(),
        ),
        descriptor(
            DEN_MEMORY_STATUS,
            "Memory status",
            "Return SQLite memory health and entry counts for the current Bear role/Workplace, plus a `recall` object reporting the recall-index consistency watermark (indexed_seq, canonical_seq, lag_count, fully_recallable, last_success_at, failed_run_count; `available: false` when semantic recall is not configured). Use this to answer truthfully what memory is currently recallable. Use session_info first when current role, work surface, or memory scope is unclear.",
            "bear.memory",
            &["memory.status.read"],
            MEMORY_READ_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_TREE,
            "Browse memory",
            "Browse allowed Bear memory paths for the current role/Workplace. Prefer current work-surface anchors before broad Bear memory; call session_info first if current scope is unclear.",
            "bear.memory",
            &["memory.tree.read"],
            MEMORY_READ_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_MEMORY_READ,
            "Read memory file",
            "Read an allowed Bear memory file for the current role/Workplace. Prefer current work-surface canonical anchors for local-understanding questions; call session_info first if current scope is unclear.",
            "bear.memory",
            &["memory.file.read"],
            MEMORY_READ_PROFILES,
            json!({"type":"object","properties":{"path":{"type":"string","description":"Allowed memory path, for example pair/notes/mem_abc.md or core/missions.md."}},"required":["path"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_SEARCH,
            "Search memory",
            "Search allowed Bear memory files for the current role/Workplace. For local project/repo/service questions, orient to the current work surface with session_info and memory_orient_work_surface before broad search.",
            "bear.memory",
            &["memory.search"],
            MEMORY_READ_PROFILES,
            json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_ENTITY_BROWSE,
            "Browse entities",
            "List Bear-local entities known to memory, optionally filtered by entity type. Use after session_info when you need stable people, missions, domains, work surfaces, connections, or artifacts referenced by Bear memory.",
            "bear.memory",
            &["entity.read"],
            MEMORY_READ_PROFILES,
            entity_browse_schema(),
        ),
        descriptor(
            DEN_ENTITY_RESOLVE,
            "Resolve entity",
            "Read a Bear-local entity by id, following merge pointers to the live entity and optionally including handles and memory relations. Use entity ids returned by entity_browse, memory_search, or session/work-surface context.",
            "bear.memory",
            &["entity.read"],
            MEMORY_READ_PROFILES,
            entity_resolve_schema(),
        ),
        descriptor(
            DEN_ENTITY_LINK_MEMORY,
            "Link memory to entity",
            "Add a descriptive relation from a memory record to a Bear-local entity. Use this to mark a record as about a person, mission, domain, work surface, connection, or artifact. This tool only writes descriptive relations (`subject`, `source`, `participant`, `applies_when`) and cannot write access rules such as `audience` or `confined_to`.",
            "bear.memory",
            &["entity.relation.write"],
            ENTITY_RELATION_WRITE_PROFILES,
            entity_link_memory_schema(),
        ),
        descriptor(
            DEN_ENTITY_MERGE,
            "Merge entities",
            "Curate-only identity repair: merge a duplicate or mistaken entity into a survivor. The loser is not deleted; it forwards to the survivor and active handles are re-homed.",
            "bear.memory",
            &["entity.governance.write"],
            CURATE_PROFILES,
            entity_merge_schema(),
        ),
        descriptor(
            DEN_ENTITY_SPLIT,
            "Split entity",
            "Curate-only identity repair: create a new entity and move selected handles to it after an incorrect merge or over-broad identity grouping.",
            "bear.memory",
            &["entity.governance.write"],
            CURATE_PROFILES,
            entity_split_schema(),
        ),
        descriptor(
            DEN_ENTITY_WRITE_ACCESS_RULE,
            "Write entity access rule",
            "Curate-only visibility governance: add an access-bearing relation from a memory record to a resolved entity. Supports `audience` and `confined_to`; these relations are enforced by the memory access gate.",
            "bear.memory",
            &["entity.access_rule.write"],
            CURATE_PROFILES,
            entity_write_access_rule_schema(),
        ),
        descriptor(
            DEN_ENTITY_WRITE_ANCHOR,
            "Write entity anchor",
            "Curate-only anchor maintenance: write an explicit canonical memory record for a resolved, anchor-eligible entity at its generated anchor path. This is the v1 source for projected entity anchors.",
            "bear.memory",
            &["entity.anchor.write", "memory.core.write"],
            CURATE_PROFILES,
            entity_write_anchor_schema(),
        ),
        descriptor(
            DEN_MEMORY_ORIENT_WORK_SURFACE,
            "Orient work surface",
            "Return a read-only orientation briefing for the likely current work surface using trusted session hints from session_info and canonical memory anchor paths when available. Use before broad memory search for local project/repo/service questions.",
            "bear.memory",
            &["memory.tree.read", "memory.file.read"],
            MEMORY_READ_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_WORK_SURFACE_CONFIRM,
            "Confirm work surface",
            "Record the user's explicit selection of an assigned managed work surface for this Pair session. Call only after the user has chosen; this does not create or dispatch a work job.",
            "work",
            &["work_surface.confirm"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"work_surface_id":{"type":"string","format":"uuid","description":"Managed work-surface ID selected explicitly by the user."}},"required":["work_surface_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD,
            "Create work-surface scaffold",
            "Create a minimal work-surface scaffold in Bear memory and register it in the work-surface index. Mutates memory; call session_info and memory_orient_work_surface first unless the user explicitly names the work surface.",
            "bear.memory",
            &["memory.write", "memory.core.write"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"work_surface_slug":{"type":"string","minLength":1,"maxLength":80},"work_surface_name":{"type":"string","minLength":1,"maxLength":200},"overview":{"type":"string","minLength":1,"maxLength":20000},"glossary":{"type":"string","maxLength":20000},"current_understanding":{"type":"string","maxLength":20000}},"required":["work_surface_slug", "work_surface_name", "overview"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_REQUEST_REVIEW,
            "Request memory review",
            "Request Reflection/curate review of role-local memory without writing shared memory directly. Use for role/Workplace-local material that may deserve broader Bear-global review; call session_info first if scope/provenance is unclear.",
            "bear.memory",
            &["memory.review.request"],
            PAIR_PROFILES,
            memory_request_review_schema(),
        ),
        descriptor(
            DEN_PROMPT_MEMORY_UPSERT,
            "Upsert prompt memory block",
            "Create or replace a Den-owned prompt memory block for the current bear role. Use this for editable runtime prompt memory, not semantic memory notes.",
            "bear.memory",
            &["memory.entry.write"],
            PAIR_PROFILES,
            prompt_memory_upsert_schema(),
        ),
        descriptor(
            DEN_PROMPT_MEMORY_LIST,
            "List prompt memory blocks",
            "List Den-owned prompt memory blocks for the current bear role.",
            "bear.memory",
            &["memory.status.read"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"include_archived":{"type":"boolean"},"scope":{"type":"string","enum":["bear_wide","profile_local","work_surface","session"]},"block_type":{"type":"string","enum":["profile_guidance","work_surface_context","session_focus","user_instruction"]},"work_surface":{"type":"string","maxLength":500},"session_id":{"type":"string","maxLength":200}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_PROMPT_MEMORY_PATCH,
            "Patch prompt memory block",
            "Update lifecycle/content fields for an existing Den-owned prompt memory block.",
            "bear.memory",
            &["memory.entry.write"],
            PAIR_PROFILES,
            prompt_memory_patch_schema(),
        ),
        descriptor(
            DEN_MEMORY_LIST_PROPOSALS,
            "List memory proposals",
            "List memory review proposals for this Bear.",
            "bear.memory",
            &["memory.proposal.read"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"status":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_READ_PROPOSAL,
            "Read memory proposal",
            "Read one memory review proposal with source pointers and status.",
            "bear.memory",
            &["memory.proposal.read"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"proposal_id":{"type":"string","format":"uuid"}},"required":["proposal_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_RESOLVE_PROPOSAL,
            "Resolve memory proposal",
            "Resolve a memory review proposal without applying shared-memory writes.",
            "bear.memory",
            &["memory.proposal.resolve"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"proposal_id":{"type":"string","format":"uuid"},"status":{"enum":["rejected","retained_local","deferred","superseded","needs_human_review"]},"review_notes":{"type":"string"},"decision_summary":{"type":"string"}},"required":["proposal_id","status"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_APPLY_CORE_UPDATE,
            "Apply core memory update",
            "Apply a reviewed update to allowed core memory paths with provenance.",
            "bear.memory",
            &["memory.core.write"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"proposal_id":{"type":"string","format":"uuid"},"target_path":{"type":"string"},"mode":{"enum":["append_section","create_file","replace_text"]},"title":{"type":"string"},"body":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"review_notes":{"type":"string"}},"required":["proposal_id","target_path","mode"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_MEMORY_MARK_LIFECYCLE,
            "Mark memory lifecycle",
            "Curate-only lifecycle marker for existing memory records: stale, superseded, archived, archive-candidate, or active. Does not promote or rewrite content.",
            "bear.memory",
            &["memory.lifecycle.write"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"memory_id":{"type":"string","minLength":1,"maxLength":200},"status":{"type":"string","enum":["active","stale","superseded","archived","archive-candidate"]},"reason":{"type":"string","maxLength":1000}},"required":["memory_id","status"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_SKILL_PROPOSE,
            "Propose skill",
            "Capture a durable skill proposal for curate review without installing it directly.",
            "bear.skills",
            &["skill.proposal.write"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"skill_name":{"type":"string"},"skill_version":{"type":"string"},"rationale":{"type":"string"},"proposed_content":{"type":"string"},"desired_roles":{"type":"array","items":{"enum":ALL_PROFILES}},"provenance":{"type":"object"}},"required":["skill_name","rationale","proposed_content"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_SKILL_APPROVE_PROPOSAL,
            "Approve skill proposal",
            "Approve a pending skill proposal, update the manifest, and queue reconciliation for affected roles.",
            "bear.skills",
            &["skill.proposal.approve"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"proposal_id":{"type":"string","format":"uuid"},"skill_name":{"type":"string"},"skill_version":{"type":"string"},"applies_to_profiles":{"type":"array","items":{"enum":ALL_PROFILES},"minItems":1},"review_notes":{"type":"string"}},"required":["proposal_id","applies_to_profiles"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_SKILL_REJECT_PROPOSAL,
            "Reject skill proposal",
            "Reject a pending skill proposal with reviewer metadata and a rejection reason.",
            "bear.skills",
            &["skill.proposal.reject"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"proposal_id":{"type":"string","format":"uuid"},"rejection_reason":{"type":"string"},"review_notes":{"type":"string"}},"required":["proposal_id","rejection_reason"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LISTS_LIST,
            "List task lists",
            "List visible planning and task-list context for the current Bear/conversation, including checked-out Docket task-list projections, submitted plan-mode gates, and saved plan artifacts where available. Docket-backed task lists are user-visible, durable/resumable plans, checklists, next steps, and roadmap slices for work jobs or the current Pair task tree. Call session_info first if current conversation/session/work-surface scope is unclear.",
            "bear.activity",
            &["task_list.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"status":{"type":"array","items":{"enum":["active","blocked","completed","cancelled","archived"]}},"owner_profile":{"enum":ALL_PROFILES},"include_archived":{"type":"boolean"},"include_completed":{"type":"boolean"},"include_plan_mode":{"type":"boolean"},"include_artifacts":{"type":"boolean"}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LISTS_GET_STATUS,
            "Get task list status",
            "Return the active Docket-backed task-list projection for this conversation/session when one exists. Use to recover focus before continuing, syncing, or handing off task-list work; call session_info first if conversation or session scope is unclear.",
            "bear.activity",
            &["task_list.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LISTS_UPDATE,
            "Update task list",
            "Activate a reviewed Pair session task list by selecting its actionable task as the current Pair task. This transitions the session projection from planned to active and authorizes execution; it does not start a background job or alter task definition/status. Use durable Docket task tools for task definition edits and terminal outcomes. For a distinct lifecycle, managed work surface, commit policy, or autonomous execution, create a Docket Job instead.",
            "bear.activity",
            &["task_list.write"],
            TASK_LIST_UPDATE_PROFILES,
            json!({"type":"object","properties":{"task_id":{"type":"string","format":"uuid","description":"Actionable task anchored to the current Pair session to select and activate."}},"required":["task_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LISTS_REQUEST_HANDOFF,
            "Request task-list handoff",
            "Request review, promotion, or sync of items from the current task list into durable Docket work. The runtime derives the current task list and its metadata from this session; provide only the desired outcome and, optionally, a subset of item IDs.",
            "bear.activity",
            &["task_list.handoff.request"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"item_ids":{"type":"array","items":{"type":"string"},"description":"Optional subset of item IDs from the current task list; omit to hand off the whole list."},"requested_outcome":{"type":"string","description":"What the receiving workflow should accomplish."}},"required":["requested_outcome"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_CREATE,
            "Create job",
            "Create a durable Docket work job with acceptance criteria and an optional initial task tree. A job may assign one or more managed work surfaces. The normal shorthand is work_surface_id, which creates one required mutation assignment; use work_surface_assignments only for multiple surfaces or optional/forbidden mutation policy. Every initial task requires concrete completion_criteria. Creating a Job does not execute or dispatch it. Keep returned full UUIDs for tool calls and evidence; in ordinary prose present a typed short handle such as `job e4e4797b` (extend the prefix if ambiguous). Do not invent a web URL: use a UI link only when a tool result provides one.",
            "bear.docket",
            &["docket.job.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"goal":{"type":"string","description":"Human-facing durable goal for the job."},"work_surface_id":{"type":"string","format":"uuid","description":"Simple common-case shorthand: one managed work surface with required mutation policy."},"work_surface_assignments":{"type":"array","description":"Use instead of work_surface_id for multiple surfaces or non-default mutation policy.","items":{"type":"object","properties":{"work_surface_id":{"type":"string","format":"uuid"},"mutation_policy":{"enum":["required","optional","forbidden"],"default":"required"}},"required":["work_surface_id"],"additionalProperties":false}},"commit_policy":{"enum":["none","per_task","per_job"]},"work_branch":{"type":"string","description":"Upstream branch work runs publish to when commit_policy allows; defaults to a generated den/job-<short-id> name."},"visibility":{"enum":["private_to_profile","same_user","bear_visible","handoff_requested"]},"supersedes_job_id":{"type":"string","format":"uuid","description":"Required only with overlap_resolution=supersede; the active matching job to replace."},"overlap_resolution":{"enum":["reject","independent","supersede"],"description":"For an exact active goal+surface overlap: reject (default), explicitly independent, or supersede the named predecessor."},"criteria":{"type":"array","items":{"type":"object","properties":{"kind":{"enum":["narrative","command","check_ref"]},"description":{"type":"string"},"spec":{"type":"object"},"sibling_order":{"type":"integer"}},"required":["description"],"additionalProperties":false}},"tasks":{"type":"array","items":{"type":"object","properties":{"client_key":{"type":"string"},"parent_client_key":{"type":"string"},"parent_task_id":{"type":"string","format":"uuid"},"sibling_order":{"type":"integer"},"kind":{"enum":["execution","investigation","decision"]},"scope":{"enum":["template","run"]},"title":{"type":"string"},"body":{"type":"string"},"completion_criteria":{"type":"array","items":{"type":"string"},"minItems":1,"description":"Concrete criteria that define when this task is done."},"difficulty":{"enum":["trivial","moderate","hard","unknown"]},"effort_hint":{"enum":["low","medium","high"]}},"required":["title","body","completion_criteria"],"additionalProperties":false}}},"required":["goal"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_LIST,
            "List Docket jobs",
            "List durable Docket jobs for the current Bear. Use for canonical job status.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"status":{"type":"array","items":{"enum":["draft","ready","running","blocked","completed","cancelled","archived"]}},"include_cancelled":{"type":"boolean"},"include_archived":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":200}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_GET,
            "Get Docket job",
            "Read one durable Docket job with criteria, task tree, current run, and run-scoped task state. Includes recent work runs (with queue placement) and work_attention: latest-attempt runs that ended blocked/failed and need triage, with their reasons. Treat this durable result as canonical status; keep full UUIDs for calls/evidence and use typed short handles in prose.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_FIND,
            "Find Docket job",
            "Find one Docket job by its full UUID or an unambiguous UUID prefix.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"job_ref":{"type":"string"}},"required":["job_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_UPDATE,
            "Update Docket job",
            "Update durable Docket job metadata. Operational status is derived from task, criterion, run, and lifecycle state; this tool does not execute task bodies.",
            "bear.docket",
            &["docket.job.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"goal":{"type":"string"},"work_surface_id":{"type":"string","format":"uuid"},"commit_policy":{"enum":["none","per_task","per_job",null]},"clear_commit_policy":{"type":"boolean"},"visibility":{"enum":["private_to_profile","same_user","bear_visible","handoff_requested"]}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_CANCEL,
            "Cancel Docket job",
            "Cancel a Docket job and prevent it from receiving further work. This is terminal; use only when the user requests cancellation or the job should no longer proceed.",
            "bear.docket",
            &["docket.job.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_CANCEL_RUN,
            "Cancel active Docket run",
            "Cancel the active Docket lifecycle run for a job without cancelling the job itself. This releases stale Pair execution claims so another session for the owning Bear can recover the task. It does not cancel a sandbox work run.",
            "bear.docket",
            &["docket.job.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_ARCHIVE,
            "Archive Docket job",
            "Archive a Docket job and remove it from the default active-job list. This is terminal; use only when the user requests archival or the job is no longer active.",
            "bear.docket",
            &["docket.job.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_EXECUTE,
            "Execute Docket job",
            "Start or advance a Docket job in this Pair session. This records job/run/task state; continue bounded work here when context and tools suffice. Use dispatch_work only for a ready job that should run in a background sandbox.",
            "bear.docket",
            &["docket.job.execute"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_RECONCILE,
            "Reconcile Docket execution",
            "Repair a typed stale Docket execution focus. Call only when execute_job returns next_action reconcile_execution; this is not a retry-safe replacement for execute_job.",
            "bear.docket",
            &["docket.job.execute"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_SETTLE_TASK,
            "Settle Docket execution task",
            "Terminally settle the current Docket-owned task and return the canonical successor, terminal, or blocked execution control. Use instead of update_current_task_status only for a task claimed by execute_job/reconcile_job_execution.",
            "bear.docket",
            &["docket.job.execute"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"task_id":{"type":"string","format":"uuid"},"status":{"enum":["done","blocked","cancelled"]},"outcome_disposition":{"enum":["completed","no_change","delegated","blocked","failed","cancelled"]},"result_refs":{"type":"object"},"result_summary":{"type":"string"}},"required":["job_id","task_id","status","result_summary"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_JOB_EVALUATE_CRITERION,
            "Evaluate Docket criterion",
            "Record run-scoped acceptance-criterion state for a Docket job. Use after checking narrative, command, or external evidence; job completion requires criteria to be met or waived.",
            "bear.docket",
            &["docket.criteria.write"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"run_id":{"type":"string","format":"uuid"},"criterion_id":{"type":"string","format":"uuid"},"status":{"enum":["unmet","met","waived"]},"evidence":{"type":"object"}},"required":["job_id","run_id","criterion_id","status"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_CREATE,
            "Create Docket task",
            "Create a durable Docket task under an explicit work Docket job or, when effective policy grants session-task ownership and job_id is omitted, under the authenticated current session. Use for durable/resumable plans, checklists, next steps, and roadmap slices; this records user-visible Docket state and does not execute work. A task has exactly one owner: its Job or its client session. Session-capable profiles can add planned/template tasks; work execution can add run-scoped child tasks. Every Docket task requires concrete completion_criteria so execution has a stopping condition. Terminal task outcomes are recorded atomically in the task journal; Job-run evidence is required when the Job task's execution policy requires it. Keep full UUIDs for tool calls and evidence; in prose use a typed unambiguous short handle such as `task e4e4797b` (extend the prefix if needed). Do not invent a web URL: use a UI link only when a tool result provides one.",
            "bear.docket",
            &["docket.task.write"],
            &["pair", "work"],
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"parent_task_id":{"type":"string","format":"uuid"},"placement":{"oneOf":[{"type":"object","properties":{"kind":{"const":"first"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"last"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"before"},"task_id":{"type":"string","format":"uuid"}},"required":["kind","task_id"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"after"},"task_id":{"type":"string","format":"uuid"}},"required":["kind","task_id"],"additionalProperties":false}]},"kind":{"enum":["execution","investigation","decision"]},"scope":{"enum":["template","run"]},"title":{"type":"string"},"body":{"type":"string"},"completion_criteria":{"type":"array","items":{"type":"string"},"minItems":1,"description":"Concrete criteria that define when this task is done."},"difficulty":{"enum":["trivial","moderate","hard","unknown"]},"effort_hint":{"enum":["low","medium","high"]},"created_in_run_id":{"type":"string","format":"uuid"}},"required":["title","body","completion_criteria"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LIST,
            "List Docket tasks",
            "List durable Docket task definitions for an explicit job/task subtree or, when job_id is omitted, the current session's implied Docket objective. Includes current-run state when available. Use for canonical Docket task hierarchy; use list_task_lists for conversation/job working focus. Treat returned full UUIDs as canonical identity/evidence and use typed unambiguous short task handles in prose.",
            "bear.docket",
            &["docket.task.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"pair_session_id":{"type":"string","format":"uuid"},"parent_task_id":{"type":"string","format":"uuid"},"include_descendants":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":500}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_FIND,
            "Find Docket task",
            "Find one Docket task by its full UUID or an unambiguous UUID prefix; optionally limit the lookup to a job. Keep the canonical UUID for follow-up calls and evidence; use a typed short task handle in prose.",
            "bear.docket",
            &["docket.task.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"task_ref":{"type":"string"},"job_ref":{"type":"string"}},"required":["task_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_UPDATE,
            "Update Docket task definition",
            "Update durable Docket task definition fields only: title/body/completion_criteria/hierarchy/kind/scope/difficulty/effort. Do not use for status or result changes; use update_current_task_status to settle a task and record its durable journal outcome.",
            "bear.docket",
            &["docket.task.write"],
            &["pair", "work"],
            json!({"type":"object","properties":{"task_id":{"type":"string","format":"uuid"},"title":{"type":"string"},"body":{"type":"string"},"completion_criteria":{"type":"array","items":{"type":"string"},"description":"Replacement concrete criteria that define when this task is done."},"parent_task_id":{"type":["string","null"],"format":"uuid"},"clear_parent_task_id":{"type":"boolean"},"sibling_order":{"type":"integer"},"kind":{"enum":["execution","investigation","decision"]},"scope":{"enum":["template","run"]},"difficulty":{"enum":["trivial","moderate","hard","unknown",null]},"effort_hint":{"enum":["low","medium","high",null]}} ,"required":["task_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_SELECT,
            "Select current session task",
            "Select an actionable task anchored to this client session as its canonical current task. Omit task_id to clear the selection. This changes session context only; it does not execute or settle work and cannot affect Job-scoped work runs. Do not call this merely because the conversational topic appears to change: first ask the user to confirm the proposed task switch. If several eligible tasks could match, ask which one to select. If none matches, ask whether to create a new session task or continue with no selected task. Never silently select, clear, replace, complete, or create a session task in response to redirection.",
            "bear.docket",
            &["docket.task.write"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"task_id":{"type":["string","null"],"format":"uuid"}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_FOCUS,
            "Focus current session task",
            "Start or resume focused execution for this client session's already-selected executable task. This does not select, create, replace, or settle a task. Success means Den acquired execution control and durably queued or started the first loop slice; a typed failure leaves the selection unchanged. Use this when the selected task should proceed without requiring a second user message.",
            "bear.docket",
            &["docket.task.write"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_UPDATE_CURRENT_STATUS,
            "Update current task status",
            "Update a session-owned task's status/results. Omit job_id and run_id only for a task owned by this client session. Never use this for a task returned or claimed by execute_job/reconcile_job_execution: use settle_execution_task with its job_id and task_id; it settles the active execution run without a run_id. Every terminal status (done, blocked, or cancelled) requires a non-empty result_summary; Den records it atomically as the durable task outcome. Use outcome_disposition when the default does not describe the result: done accepts completed, no_change, or delegated; blocked accepts blocked or failed; cancelled accepts only cancelled. For report-only work, result_summary is sufficient and result_refs may be omitted. If the task has a verified primary output, provide result_refs.primary_output {kind: git_commit|den_artifact, artifact_ref, immutable_identity} and result_refs.validation {primary_output_ref, immutable_identity, command, result: passed, execution_provenance}; validation must match the primary output exactly. Do not invent primary-output evidence for work that did not produce it. Does not edit durable task definitions or execute task bodies.",
            "bear.docket",
            &["docket.task.write"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"run_id":{"type":"string","format":"uuid"},"task_id":{"type":"string","format":"uuid"},"status":{"enum":["pending","done","blocked","cancelled"]},"outcome_disposition":{"enum":["completed","no_change","delegated","blocked","failed","cancelled"],"description":"Optional typed terminal outcome. done accepts completed, no_change, or delegated; blocked accepts blocked or failed; cancelled accepts cancelled. Omit to use the status default. Not allowed for pending."},"result_refs":{"type":"object","description":"Optional. Omit for report-only completion. When reporting a verified output, provide primary_output {kind: git_commit|den_artifact, artifact_ref, immutable_identity} and validation {primary_output_ref, immutable_identity, command, result: passed, execution_provenance}."},"result_summary":{"type":"string","description":"Required for terminal status done, blocked, or cancelled. Den records it atomically as the durable task outcome; describe what actually occurred."}},"required":["task_id","status"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_DOCKET_ENTRY_APPEND,
            "Append Docket entry",
            "Append a durable finding, decision, obstacle, follow-up, milestone, or question to a task journal or job notebook. Outcomes are settlement-owned and cannot be appended manually. Questions may be recorded only by Pair. A task-journal entry without task_id uses this client session's selected current task; otherwise it is rejected before persistence.",
            "bear.docket",
            &["docket.task.write"],
            &["pair", "work"],
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"task_id":{"type":"string","format":"uuid"},"run_id":{"type":"string","format":"uuid"},"scope":{"enum":["task_journal","job_notebook"]},"kind":{"enum":["finding","decision","obstacle","follow_up","milestone","question"]},"summary":{"type":"string"},"body":{"type":"string"},"evidence_refs":{"type":"array","items":{}},"related_task_ids":{"type":"array","items":{"type":"string","format":"uuid"}},"tags":{"type":"array","items":{"type":"string"}}},"required":["scope","kind","summary"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_DOCKET_ENTRY_PROMOTE,
            "Promote Docket entry",
            "Promote one non-outcome task-journal entry into its job notebook by reference. The operation is idempotent and preserves the original entry and provenance rather than copying model-authored content.",
            "bear.docket",
            &["docket.task.write"],
            &["pair", "work"],
            json!({"type":"object","properties":{"entry_id":{"type":"string","format":"uuid"}},"required":["entry_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_DOCKET_ENTRY_LIST,
            "List Docket entries",
            "List durable task-journal or job-notebook entries for this Bear, filtered by job or task. Includes settlement outcomes and explicitly recorded findings, decisions, obstacles, follow-ups, milestones, and questions.",
            "bear.docket",
            &["docket.task.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"task_id":{"type":"string","format":"uuid"},"limit":{"type":"integer","minimum":1,"maximum":500}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_SEARCH,
            "Search Cabinet",
            "Search Cabinet, the Den-wide shared knowledge wiki that humans and Bears read and edit together. Matches item titles and current content; returns item summaries with cabinet_ref and current version. Cabinet is shared durable knowledge, not your private memory: use memory tools for Bear-local notes and this for knowledge meant for people and other Bears.",
            "bear.cabinet",
            &["cabinet.read"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"query":{"type":"string","description":"Substring match over titles and current content. Empty lists recent items."},"lifecycle":{"enum":["active","archived"],"description":"Defaults to active items."}},"required":["query"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_READ,
            "Read Cabinet item",
            "Read one Cabinet shared-knowledge item: title, Markdown content, revision, current version ref, provenance, and source links. Pass version_ref to read an older immutable revision. Always read an item before updating it - the returned current version ref is the base_version a later cabinet_update requires.",
            "bear.cabinet",
            &["cabinet.read"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"cabinet_ref":{"type":"string","description":"Item ref from cabinet_search or a citation, e.g. cabinet_item_..."},"version_ref":{"type":"string","description":"Optional immutable revision to read instead of the current version."}},"required":["cabinet_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_CREATE,
            "Create Cabinet item",
            "Create a new Cabinet shared-knowledge document with Markdown content. The item publishes immediately and is visible and editable by humans and other authorized Bears; write knowledge worth sharing, not session scratch or private Bear memory. Optionally attach source links recording where the knowledge came from. Search first to avoid duplicating an existing item.",
            "bear.cabinet",
            &["cabinet.write"],
            &["chat", "pair", "curate"],
            json!({"type":"object","properties":{"title":{"type":"string"},"content":{"type":"string","description":"Markdown document body."},"source_links":{"type":"array","items":{"type":"object","properties":{"source_kind":{"enum":["url","offline","artifact","conversation","external_record"]},"locator":{"type":"string","description":"https URL, synthetic scheme like book://isbn/..., artifact_... ref, or conversation id, matching source_kind."},"role":{"enum":["origin","citation","related"]}},"required":["source_kind","locator","role"],"additionalProperties":false}}},"required":["title","content"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_UPDATE,
            "Update Cabinet item",
            "Publish a new revision of a Cabinet item with the full replacement Markdown content. Requires base_version: the current version ref from a fresh cabinet_read. If someone else published a newer revision first, the update fails with the new current version ref - re-read, merge your change into the latest content, and retry. Every revision is immutable and kept in history; nothing is overwritten.",
            "bear.cabinet",
            &["cabinet.write"],
            &["chat", "pair", "curate"],
            json!({"type":"object","properties":{"cabinet_ref":{"type":"string"},"content":{"type":"string","description":"Full replacement Markdown body (not a diff)."},"base_version":{"type":"string","description":"The current version ref this edit is based on, from cabinet_read."},"title":{"type":"string","description":"Optional new title."}},"required":["cabinet_ref","content","base_version"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_HISTORY,
            "Read Cabinet history",
            "List the immutable revisions of a Cabinet item, newest first: version ref, revision number, who authored it (person or Bear), when, and the content hash. Use this after a cabinet_update conflict to see what changed and who changed it, or to cite or read a specific earlier revision with cabinet_read.",
            "bear.cabinet",
            &["cabinet.read"],
            ALL_PROFILES,
            json!({"type":"object","properties":{"cabinet_ref":{"type":"string"}},"required":["cabinet_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_SOURCE_LINK,
            "Link Cabinet source",
            "Attach or detach provenance on a Cabinet item: where its knowledge came from. Add a link when you learn the origin of material already written down; source links are provenance only, so Cabinet never stores the linked content itself. Detaching removes the link and never alters any revision.",
            "bear.cabinet",
            &["cabinet.write"],
            &["chat", "pair", "curate"],
            json!({"type":"object","properties":{"cabinet_ref":{"type":"string"},"action":{"enum":["add","remove"],"description":"Defaults to add."},"source_kind":{"enum":["url","offline","artifact","conversation","external_record"],"description":"Required when adding."},"locator":{"type":"string","description":"Required when adding: https URL, synthetic scheme like book://isbn/..., artifact_... ref, or conversation id, matching source_kind."},"role":{"enum":["origin","citation","related"],"description":"Required when adding."},"source_ref":{"type":"string","description":"Required when removing: the cabinet_source_... ref from cabinet_read."}},"required":["cabinet_ref"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CABINET_LIFECYCLE,
            "Set Cabinet item lifecycle",
            "Archive a Cabinet item so it drops out of default search, or restore an archived one. Archiving is reversible and keeps every revision readable; it does not delete anything. Prefer archiving superseded knowledge over rewriting it away. Only people can delete a Cabinet item.",
            "bear.cabinet",
            &["cabinet.write"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"cabinet_ref":{"type":"string"},"lifecycle":{"enum":["archived","active"],"description":"archived hides the item from default search; active restores it."}},"required":["cabinet_ref","lifecycle"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_RUNTIME_DIAGNOSTICS_LIST,
            "List runtime diagnostics",
            "List bounded, sanitized warning and error evidence for this Bear. Filter by Work run, runtime run, session, Docket job, event code, or severity when investigating a user-visible failure. This is a searchable exception record, not raw server logs; it never returns prompts, tool payloads, credentials, or cross-Bear events.",
            "bear.runtime",
            &["runtime.diagnostics.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"work_run_id":{"type":"string","format":"uuid"},"runtime_run_id":{"type":"string"},"session_id":{"type":"string"},"docket_job_id":{"type":"string","format":"uuid"},"event_code":{"type":"string"},"severity":{"enum":["warning","error"]},"limit":{"type":"integer","minimum":1,"maximum":200}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LIST_CHECKOUT,
            "Checkout task list",
            "Create a task-list projection from an explicit Docket job/root task subtree or, in pair conversation scope when job_id is omitted, the current conversation's implied Docket objective. Use this when you want to work Docket tasks through the current conversation/task-list focus. Checkout records focus/projection state only; it does not execute tasks or change task definitions by itself.",
            "bear.docket",
            &["docket.task.checkout"],
            &["pair", "work"],
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"parent_task_id":{"type":"string","format":"uuid"}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_DISPATCH,
            "Dispatch work job",
            "Queue a ready Docket job for isolated background execution in a sandbox. `root` identifies the managed source or sandbox-provider root; `image` selects a sandbox toolchain image. Docket dispatch never modifies Pair's attached checkout. Keep full UUIDs for calls and evidence; use typed short handles only in prose.",
            "bear.docket",
            &["docket.job.execute"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"root":{"type":"string","description":"Managed work surface or sandbox provider root."},"git_ref":{"type":"string"},"image":{"type":"string","description":"Catalog image name for sandbox execution."}},"required":["job_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_RUN_LIST,
            "List work runs",
            "List autonomous work runs (sandbox executions of work-assigned Docket tasks) for this Bear, optionally filtered by job, task, or state. Queued runs include a queue object (position within the job's queue and the in-flight run they are waiting behind) — runs serialize per job. Keep full UUIDs for evidence/follow-up calls and present typed unambiguous short work-run handles in prose.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"job_id":{"type":"string","format":"uuid"},"task_id":{"type":"string","format":"uuid"},"state":{"enum":["queued","claimed","provisioning","running","reporting","succeeded","blocked","failed","cancelled","timed_out"]},"limit":{"type":"integer","minimum":1,"maximum":200}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_RUN_GET,
            "Get work run",
            "Read one autonomous work run: state, attempt, sandbox type and strength, recognized work surface, result summary (including published branch/commit in result_refs), changed files, bounded log tail, and error/blockage reason. Queued runs include a queue object (position within the job's queue and the in-flight run they are waiting behind). Use its terminal result and durable evidence for claims about changes, tests, or commits; a failed/cancelled run proves only the recorded partial progress. Its work surface is not implicitly accessible to Pair. Keep the full UUID for tool calls/evidence and present `work run e4e4797b`-style handles in prose.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"work_run_id":{"type":"string","format":"uuid"}},"required":["work_run_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_RUN_FIND,
            "Find work run",
            "Find one work run by full UUID or an unambiguous UUID prefix, or list runs for a job UUID or prefix. Keep canonical UUIDs for calls/evidence and use typed unambiguous short work-run handles in prose.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{"run_ref":{"type":"string"},"job_ref":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_RUN_CANCEL,
            "Cancel work run",
            "Request cancellation of an active work run. The dispatch worker tears the sandbox down and records the task as blocked; this tool only sets the cancel flag and never touches the sandbox host directly. After requesting cancellation, read the canonical work-run result before describing the outcome; its separate worktree may contain partial changes. Keep the full UUID for the call and use a typed short handle in prose.",
            "bear.docket",
            &["docket.job.execute"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"work_run_id":{"type":"string","format":"uuid"}},"required":["work_run_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_RUN_RESOLVE_STALLED,
            "Resolve stalled work run",
            "Record the operator's resolution of a stalled work run without changing its terminal outcome or diagnostic evidence. Use only after inspecting the stalled run and deciding how it was handled; retrying work creates a new attempt. A repeated request does not overwrite the original resolution.",
            "bear.docket",
            &["docket.job.execute"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"work_run_id":{"type":"string","format":"uuid"},"reason":{"type":"string","minLength":1,"maxLength":2000}},"required":["work_run_id"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_CATALOG,
            "Get work catalog",
            "Read what work can run on: managed work surfaces assigned to this bear (preferred for create_job.work_surface_id / dispatch_work.root), the sandbox provider's roots, and the container images selectable for dispatch_work. Use this before dispatching when the surface or toolchain image is not obvious.",
            "bear.docket",
            &["docket.job.read"],
            TASK_LIST_READ_PROFILES,
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_LIST_SYNC,
            "Sync task list to Docket",
            "Apply authorized changes from a checked-out Docket-backed task-list projection back to Docket-backed tasks. Docket-backed items update task definitions and run-scoped status; local-only items in a Docket checkout become new child tasks. Conflicts are reported instead of overwritten.",
            "bear.docket",
            &["docket.task.sync"],
            &["pair", "work"],
            json!({"type":"object","properties":{"task_list":{"type":"object","description":"TaskListProjection returned by get_job, get_task_list_status, update_task_list, or checkout."}},"required":["task_list"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_PLAN_MODE_ENTER,
            "Enter planning mode",
            "Enter client pair workplan mode and reflect that mode in the client session UI. Use this when the user asks to enter planning mode.",
            "bear.workplan",
            &["plan_mode.enter"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"reason":{"type":"string"},"previous_permission_mode":{"type":"string"}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_PLAN_MODE_STATUS,
            "Get plan mode status",
            "Return the current client pair workplan gate for this session, if any.",
            "bear.workplan",
            &["plan_mode.read"],
            PAIR_PROFILES,
            empty_schema(),
        ),
        descriptor(
            DEN_PLAN_MODE_RECORD_APPROVAL,
            "Record plan approval",
            "Record explicit approval from the authenticated human for the currently submitted implementation workplan. Use only when the user clearly approves the current plan in this conversation, for example 'go ahead', 'approved', or 'proceed'.",
            "bear.workplan",
            &["plan_mode.approve"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"plan_mode_id":{"type":"string","format":"uuid"},"approval_text":{"type":"string","description":"The user's approval text that prompted this tool call."}},"required":["approval_text"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_PLAN_MODE_EXIT,
            "Submit implementation plan",
            "Submit a markdown implementation workplan artifact for user approval. This is for durable implementation workplans, not for the live visible task list; use Docket task-list projections for visible conversation/task-list tracking.",
            "bear.workplan",
            &["plan_mode.exit"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"plan_mode_id":{"type":"string","format":"uuid"},"title":{"type":"string"},"body":{"type":"string"}},"required":["title","body"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_PLAN_MODE_CANCEL,
            "Cancel plan mode",
            "Cancel the current client pair workplan gate without approving implementation.",
            "bear.workplan",
            &["plan_mode.cancel"],
            PAIR_PROFILES,
            json!({"type":"object","properties":{"plan_mode_id":{"type":"string","format":"uuid"}},"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_WRITE_INTENT,
            "Write task intent",
            "Write a schema-validated task intent from chat or pair for later curate review.",
            "bear.tasks",
            &["task.intent.write"],
            CHAT_AND_PAIR_PROFILES,
            json!({"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"requested_outcome":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"allowed_tools_hint":{"type":"array","items":{"type":"string"}},"source_reference":{"type":"object"}},"required":["title","summary","requested_outcome"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_APPROVE_INTENT,
            "Approve task intent",
            "Approve a chat/pair task intent, write the canonical core task, and update source intent audit metadata.",
            "bear.tasks",
            &["task.intent.approve"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"source_intent_path":{"type":"string"},"task_id":{"type":"string"},"title":{"type":"string"},"approved_scope":{"type":"object"},"allowed_tools":{"type":"array","items":{"type":"string"}},"expires_at":{"type":"string"},"review_notes":{"type":"string"}},"required":["source_intent_path","task_id","title","approved_scope","allowed_tools"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_TASK_REJECT_INTENT,
            "Reject task intent",
            "Reject a chat/pair task intent and update source intent audit metadata with the rejection reason.",
            "bear.tasks",
            &["task.intent.reject"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"source_intent_path":{"type":"string"},"rejection_reason":{"type":"string"},"review_notes":{"type":"string"}},"required":["source_intent_path","rejection_reason"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_CORE_WRITE_RESULT_SUMMARY,
            "Write core result summary",
            "Write a curate-reviewed summary of work results into shared core memory through Den-controlled validation.",
            "bear.core",
            &["core.result_summary.write"],
            CURATE_PROFILES,
            json!({"type":"object","properties":{"task_id":{"type":"string"},"run_id":{"type":"string"},"summary":{"type":"string"},"durable_learnings":{"type":"array","items":{"type":"string"}},"source_result_path":{"type":"string"}},"required":["task_id","summary"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_OBSERVATION_WRITE,
            "Write observation",
            "Write a schema-validated inbound observation from a Den-delivered watch event.",
            "bear.observations",
            &["observation.write"],
            WATCH_PROFILES,
            json!({"type":"object","properties":{"observation_id":{"type":"string"},"summary":{"type":"string"},"salience":{"type":"string"},"payload_ref":{"type":"string"},"source":{"type":"object"}},"required":["summary"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_WORK_PREPARE_RUST_DEPENDENCIES,
            "Prepare Rust dependencies",
            "Prepare dependencies for one Rust package outside the restricted work sandbox. Use after changing Cargo.toml; update_lockfile may modify the applicable Cargo.lock. The sandbox remains offline.",
            "work.run",
            &["work.rust_dependencies.prepare"],
            WORK_PROFILES,
            json!({"type":"object","properties":{
                "manifest_path":{"type":"string","minLength":1},
                "package":{"type":"string","minLength":1},
                "resolution":{"enum":["locked","update_lockfile"]},
                "preparation":{"enum":["check","test_no_run"]}
            },"required":["manifest_path","package","resolution","preparation"],"additionalProperties":false}),
        ),
        descriptor(
            DEN_RUN_WRITE_RESULT,
            "Write run result",
            "Write a schema-validated work run result under the active Den-issued run context.",
            "bear.runs",
            &["run.result.write"],
            WORK_PROFILES,
            json!({"type":"object","properties":{"task_id":{"type":"string"},"run_id":{"type":"string"},"status":{"enum":["succeeded","failed","partial"]},"summary":{"type":"string"},"result":{"type":"object"},"follow_up":{"type":"array","items":{"type":"string"}}},"required":["task_id","run_id","status","summary"],"additionalProperties":false}),
        ),
    ]
}

pub fn builtin_den_tool_descriptors_for_profile(role: BearProfile) -> Vec<DenToolDescriptor> {
    builtin_den_tool_descriptors()
        .into_iter()
        .filter(|descriptor| descriptor.allows_profile(role))
        .collect()
}

/// Provider names for bear.memory tools exposed to the given trust profile.
pub fn memory_tool_provider_names_for_profile(role: BearProfile) -> Vec<String> {
    builtin_den_tool_descriptors_for_profile(role)
        .into_iter()
        .filter(|descriptor| descriptor.domain == "memory")
        .map(|descriptor| descriptor.provider_name)
        .collect()
}

/// Compact manifest for browser chat system context so meta questions need no tool round-trip.
pub fn render_profile_tool_surface_blurb(role: BearProfile) -> String {
    let descriptors = builtin_den_tool_descriptors_for_profile(role);
    let mut lines = vec![
        "Available Den tools for this browser chat session (answer from this list when asked about capabilities; call a tool only when needed):".to_string(),
    ];
    for descriptor in descriptors {
        lines.push(format!(
            "- {} ({}): {}",
            descriptor.label, descriptor.provider_name, descriptor.description
        ));
    }
    lines.join("\n")
}

/// Den tools on the client pair surface (adapter environment + native pair LLM turns).
/// Keeps session/memory/plan tools without session-admin or plan-mode control noise.
pub fn pair_acp_surface_den_tool_names() -> &'static [&'static str] {
    &[
        DEN_CONVERSATION_SET_TITLE,
        DEN_CAPABILITY_SEARCH,
        DEN_CAPABILITY_DESCRIBE,
        DEN_WEB_FETCH,
        DEN_WEB_SEARCH,
        DEN_SITUATION_GET,
        DEN_MEMORY_WRITE_ENTRY,
        DEN_MEMORY_STATUS,
        DEN_MEMORY_TREE,
        DEN_MEMORY_READ,
        DEN_MEMORY_SEARCH,
        DEN_MEMORY_REQUEST_REVIEW,
        DEN_TASK_LISTS_LIST,
        DEN_TASK_LISTS_GET_STATUS,
        DEN_TASK_LISTS_UPDATE,
        DEN_TASK_LISTS_REQUEST_HANDOFF,
        DEN_JOB_CREATE,
        DEN_JOB_LIST,
        DEN_JOB_GET,
        DEN_JOB_UPDATE,
        DEN_JOB_CANCEL,
        DEN_JOB_CANCEL_RUN,
        DEN_JOB_ARCHIVE,
        DEN_JOB_EXECUTE,
        DEN_JOB_RECONCILE,
        DEN_JOB_SETTLE_TASK,
        DEN_JOB_EVALUATE_CRITERION,
        DEN_TASK_CREATE,
        DEN_TASK_LIST,
        DEN_TASK_UPDATE,
        // ACP has no client-owned task-picker UI. The Pair agent selects only on
        // explicit user instruction; task-list visibility is not task authority.
        DEN_TASK_SELECT,
        DEN_TASK_FOCUS,
        DEN_TASK_UPDATE_CURRENT_STATUS,
        DEN_DOCKET_ENTRY_APPEND,
        DEN_DOCKET_ENTRY_LIST,
        DEN_CABINET_SEARCH,
        DEN_CABINET_READ,
        DEN_CABINET_CREATE,
        DEN_CABINET_UPDATE,
        DEN_CABINET_HISTORY,
        DEN_CABINET_SOURCE_LINK,
        DEN_RUNTIME_DIAGNOSTICS_LIST,
        DEN_TASK_LIST_SYNC,
        DEN_TASK_LIST_CHECKOUT,
        DEN_WORK_DISPATCH,
        DEN_WORK_RUN_LIST,
        DEN_WORK_RUN_GET,
        DEN_WORK_RUN_CANCEL,
        DEN_WORK_CATALOG,
    ]
}

pub fn builtin_den_tool_descriptors_for_pair_acp_surface() -> Vec<DenToolDescriptor> {
    let allowed: std::collections::HashSet<&str> =
        pair_acp_surface_den_tool_names().iter().copied().collect();
    builtin_den_tool_descriptors()
        .into_iter()
        .filter(|descriptor| allowed.contains(descriptor.name))
        .collect()
}

pub fn builtin_den_tool_descriptor_for_provider_name(
    provider_name: &str,
) -> Option<DenToolDescriptor> {
    builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| {
            descriptor.provider_name == provider_name
                || descriptor.provider_aliases.contains(&provider_name)
                || descriptor.name == provider_name
        })
}

pub fn provider_aliases_for_tool(name: &str) -> &'static [&'static str] {
    match name {
        DEN_WEB_FETCH => &[DEN_WEB_FETCH_LEGACY_PROVIDER],
        DEN_SITUATION_GET => &[DEN_SITUATION_GET_LEGACY_PROVIDER],
        DEN_MEMORY_TREE => &[DEN_MEMORY_TREE_LEGACY_PROVIDER],

        _ => &[],
    }
}

fn den_tool_description(name: &'static str, description: &'static str) -> &'static str {
    let guidance = match name {
        DEN_CAPABILITIES_LIST_SELF | DEN_CAPABILITY_SEARCH | DEN_CAPABILITY_DESCRIBE => None,
        DEN_SITUATION_GET => None,
        DEN_CONVERSATION_SET_TITLE => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::Conversation,
            side_effect: ToolSideEffectKind::ConversationMetadata,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_WEB_FETCH | DEN_WEB_SEARCH => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::ExternalWeb,
            side_effect: ToolSideEffectKind::ExternalNetwork,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_BEAR_ENVIRONMENT => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::Conversation,
            side_effect: ToolSideEffectKind::ReadOnly,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_MEMORY_WRITE_ENTRY => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::BearRoleMemory,
            side_effect: ToolSideEffectKind::WritesMemory,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_MEMORY_STATUS
        | DEN_MEMORY_TREE
        | DEN_MEMORY_READ
        | DEN_MEMORY_SEARCH
        | DEN_MEMORY_ORIENT_WORK_SURFACE => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::BearRoleMemory,
            side_effect: ToolSideEffectKind::ReadOnly,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::BearRoleMemory,
            side_effect: ToolSideEffectKind::WritesMemory,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_MEMORY_REQUEST_REVIEW => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::BearRoleMemory,
            side_effect: ToolSideEffectKind::WritesMemory,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_TASK_LISTS_LIST | DEN_TASK_LISTS_GET_STATUS => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ReadOnly,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_TASK_LISTS_UPDATE | DEN_TASK_LISTS_REQUEST_HANDOFF => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ActiveWorkState,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_JOB_LIST | DEN_JOB_GET | DEN_DOCKET_ENTRY_LIST => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ReadOnly,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_JOB_CREATE
        | DEN_JOB_UPDATE
        | DEN_JOB_CANCEL_RUN
        | DEN_JOB_EXECUTE
        | DEN_JOB_EVALUATE_CRITERION
        | DEN_TASK_CREATE
        | DEN_TASK_UPDATE
        | DEN_TASK_UPDATE_CURRENT_STATUS
        | DEN_DOCKET_ENTRY_APPEND
        | DEN_DOCKET_ENTRY_PROMOTE
        | DEN_TASK_LIST_SYNC
        | DEN_TASK_LIST_CHECKOUT => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ActiveWorkState,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_TASK_LIST => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ReadOnly,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_PLAN_MODE_ENTER
        | DEN_PLAN_MODE_STATUS
        | DEN_PLAN_MODE_RECORD_APPROVAL
        | DEN_PLAN_MODE_EXIT
        | DEN_PLAN_MODE_CANCEL => Some(ToolDescriptorGuidance {
            scope: ToolScopeKind::CurrentSession,
            side_effect: ToolSideEffectKind::ActiveWorkState,
            orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
        }),
        DEN_SKILL_PROPOSE | DEN_SKILL_APPROVE_PROPOSAL | DEN_SKILL_REJECT_PROPOSAL => {
            Some(ToolDescriptorGuidance {
                scope: ToolScopeKind::CurrentSession,
                side_effect: ToolSideEffectKind::SkillReview,
                orientation: ToolOrientationPolicy::UseSessionInfoIfScopeUnclear,
            })
        }
        _ => None,
    };
    let Some(guidance) = guidance else {
        return description;
    };
    Box::leak(
        format!(
            "{} {}",
            description,
            render_tool_descriptor_guidance(guidance)
        )
        .into_boxed_str(),
    )
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
        description: den_tool_description(name, description),
        kind: "server_tool",
        provider: "den",
        execution_target: "den",
        scope,
        domain: tool_domain(name),
        content_class: tool_content_class(name),
        availability: "available",
        permissions,
        allowed_roles,
        approval_policy: if name == DEN_WEB_FETCH {
            "always"
        } else {
            "never"
        },
        display: den_tool_display(name, label).to_json(),
        input_schema,
    }
}

pub fn den_tool_display_json_for_provider(provider_name: &str, args: &Value) -> Option<Value> {
    let descriptor = builtin_den_tool_descriptor_for_provider_name(provider_name)?;
    let display = den_tool_display(descriptor.name, descriptor.label);
    let target = crate::tools::display::tool_target_summary(display.target_arg_keys, args);
    Some(json!({
        "label": display.label,
        "title": target.as_ref()
            .map(|target| format!("{} {}", display.progress_verb, target))
            .unwrap_or_else(|| display.label.to_string()),
        "subtitle": target,
        "category": display.category,
        "status": "requested",
        "progress": display.progress_verb,
        "complete": display.complete_verb,
        "approval_summary": display.approval_summary,
    }))
}

pub fn den_tool_policy_json_for_provider(provider_name: &str) -> Option<Value> {
    let descriptor = builtin_den_tool_descriptor_for_provider_name(provider_name)?;
    Some(json!({
        "execution_target": descriptor.execution_target,
        "scope_basis": descriptor.scope,
        "risk": if descriptor.approval_policy == "never" {
            "read_only"
        } else {
            "mutating"
        },
        "approval_required": descriptor.approval_policy != "never",
        "canonical_tool": descriptor.name,
        "provider_tool": descriptor.provider_name,
    }))
}

pub fn den_tool_completion_status_text(provider_name: &str) -> Option<String> {
    let descriptor = builtin_den_tool_descriptor_for_provider_name(provider_name)?;
    let display = den_tool_display(descriptor.name, descriptor.label);
    Some(format!("{}.", display.complete_verb))
}

pub fn den_tool_display(name: &'static str, label: &'static str) -> ToolDisplayDescriptor {
    match name {
        DEN_CABINET_SEARCH => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Searching Cabinet for",
            complete_verb: "Searched Cabinet for",
            target_arg_keys: &["query"],
            sensitive_arg_keys: &[],
            approval_summary: "Search shared Cabinet knowledge.",
        },
        DEN_CABINET_READ => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Reading Cabinet item",
            complete_verb: "Read Cabinet item",
            target_arg_keys: &["cabinet_ref"],
            sensitive_arg_keys: &[],
            approval_summary: "Read one shared Cabinet item.",
        },
        DEN_CABINET_CREATE => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Creating Cabinet item",
            complete_verb: "Created Cabinet item",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &[],
            approval_summary: "Publish a new shared Cabinet item visible to humans and Bears.",
        },
        DEN_CABINET_HISTORY => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Reading Cabinet history for",
            complete_verb: "Read Cabinet history for",
            target_arg_keys: &["cabinet_ref"],
            sensitive_arg_keys: &[],
            approval_summary: "List revisions of a Cabinet item.",
        },
        DEN_CABINET_SOURCE_LINK => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Updating Cabinet sources for",
            complete_verb: "Updated Cabinet sources for",
            target_arg_keys: &["cabinet_ref"],
            sensitive_arg_keys: &[],
            approval_summary: "Attach or detach provenance on a Cabinet item.",
        },
        DEN_CABINET_LIFECYCLE => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Updating Cabinet lifecycle for",
            complete_verb: "Updated Cabinet lifecycle for",
            target_arg_keys: &["cabinet_ref"],
            sensitive_arg_keys: &[],
            approval_summary: "Archive or restore a Cabinet item.",
        },
        DEN_CABINET_UPDATE => ToolDisplayDescriptor {
            label,
            category: "cabinet",
            progress_verb: "Updating Cabinet item",
            complete_verb: "Updated Cabinet item",
            target_arg_keys: &["cabinet_ref", "title"],
            sensitive_arg_keys: &[],
            approval_summary: "Publish a new revision of a shared Cabinet item.",
        },
        DEN_CONVERSATION_SET_TITLE => ToolDisplayDescriptor {
            label,
            category: "conversation",
            progress_verb: "Setting conversation title",
            complete_verb: "Set conversation title",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &[],
            approval_summary: "Update the visible conversation title.",
        },
        DEN_WEB_FETCH => ToolDisplayDescriptor {
            label,
            category: "web",
            progress_verb: "Fetching",
            complete_verb: "Fetched",
            target_arg_keys: &["url"],
            sensitive_arg_keys: &[],
            approval_summary: "Fetch this URL with Den web safeguards.",
        },
        DEN_WEB_SEARCH => ToolDisplayDescriptor {
            label,
            category: "web",
            progress_verb: "Searching web for",
            complete_verb: "Searched web for",
            target_arg_keys: &["query"],
            sensitive_arg_keys: &[],
            approval_summary: "Search the web through the configured Den provider.",
        },
        DEN_BEAR_ENVIRONMENT => ToolDisplayDescriptor {
            label,
            category: "orientation",
            progress_verb: "Inspecting bear environment",
            complete_verb: "Inspected bear environment",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read a structured snapshot of the current Bear runtime environment.",
        },
        DEN_SITUATION_GET => ToolDisplayDescriptor {
            label,
            category: "orientation",
            progress_verb: "Checking session info",
            complete_verb: "Checked session info",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary:
                "Read trusted session, Bear, human, policy, and workspace orientation.",
        },
        DEN_MEMORY_WRITE_ENTRY => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Writing memory entry",
            complete_verb: "Wrote memory entry",
            target_arg_keys: &["title", "path"],
            sensitive_arg_keys: &["body", "content"],
            approval_summary: "Write a role-local memory entry with provenance.",
        },
        DEN_MEMORY_STATUS => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Checking memory status",
            complete_verb: "Checked memory status",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read memory health and counts.",
        },
        DEN_MEMORY_TREE => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Browsing memory",
            complete_verb: "Browsed memory",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Browse allowed memory paths.",
        },
        DEN_MEMORY_READ => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Reading memory",
            complete_verb: "Read memory",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Read this allowed memory file.",
        },
        DEN_MEMORY_SEARCH => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Searching memory for",
            complete_verb: "Searched memory for",
            target_arg_keys: &["query"],
            sensitive_arg_keys: &[],
            approval_summary: "Search allowed Bear memory.",
        },
        DEN_MEMORY_ORIENT_WORK_SURFACE => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Orienting work surface",
            complete_verb: "Oriented work surface",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read work-surface memory anchors and orientation.",
        },
        DEN_WORK_SURFACE_CONFIRM => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Confirming work surface",
            complete_verb: "Confirmed work surface",
            target_arg_keys: &["work_surface_id"],
            sensitive_arg_keys: &[],
            approval_summary:
                "Record the user's selected managed work surface for this Pair session.",
        },
        DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Creating work-surface scaffold",
            complete_verb: "Created work-surface scaffold",
            target_arg_keys: &["work_surface_slug", "work_surface_name"],
            sensitive_arg_keys: &["overview", "glossary", "current_understanding"],
            approval_summary: "Create canonical memory scaffold for this work surface.",
        },
        DEN_MEMORY_REQUEST_REVIEW => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Requesting memory review",
            complete_verb: "Requested memory review",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &["summary", "rationale", "proposed_content", "proposed_patch"],
            approval_summary: "Ask curate to review role-local memory.",
        },
        DEN_MEMORY_LIST_PROPOSALS => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Listing memory proposals",
            complete_verb: "Listed memory proposals",
            target_arg_keys: &["status"],
            sensitive_arg_keys: &[],
            approval_summary: "List memory review proposals.",
        },
        DEN_MEMORY_READ_PROPOSAL => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Reading memory proposal",
            complete_verb: "Read memory proposal",
            target_arg_keys: &["proposal_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Read this memory review proposal.",
        },
        DEN_MEMORY_RESOLVE_PROPOSAL => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Resolving memory proposal",
            complete_verb: "Resolved memory proposal",
            target_arg_keys: &["proposal_id", "status"],
            sensitive_arg_keys: &["review_notes", "decision_summary"],
            approval_summary: "Record a curate decision for this memory proposal.",
        },
        DEN_MEMORY_APPLY_CORE_UPDATE => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Applying core memory update",
            complete_verb: "Applied core memory update",
            target_arg_keys: &["target_path", "mode"],
            sensitive_arg_keys: &["body", "old_text", "new_text", "review_notes"],
            approval_summary: "Apply a reviewed update to core memory.",
        },
        DEN_MEMORY_MARK_LIFECYCLE => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Marking memory lifecycle",
            complete_verb: "Marked memory lifecycle",
            target_arg_keys: &["memory_id", "status"],
            sensitive_arg_keys: &["reason"],
            approval_summary: "Mark an existing memory record's lifecycle status.",
        },
        DEN_SKILL_PROPOSE => ToolDisplayDescriptor {
            label,
            category: "skills",
            progress_verb: "Proposing skill",
            complete_verb: "Proposed skill",
            target_arg_keys: &["skill_name", "skill_version"],
            sensitive_arg_keys: &["proposed_content"],
            approval_summary: "Create a skill proposal for curate review.",
        },
        DEN_SKILL_APPROVE_PROPOSAL => ToolDisplayDescriptor {
            label,
            category: "skills",
            progress_verb: "Approving skill proposal",
            complete_verb: "Approved skill proposal",
            target_arg_keys: &["proposal_id", "skill_name"],
            sensitive_arg_keys: &["review_notes"],
            approval_summary: "Approve this skill proposal.",
        },
        DEN_SKILL_REJECT_PROPOSAL => ToolDisplayDescriptor {
            label,
            category: "skills",
            progress_verb: "Rejecting skill proposal",
            complete_verb: "Rejected skill proposal",
            target_arg_keys: &["proposal_id"],
            sensitive_arg_keys: &["rejection_reason", "review_notes"],
            approval_summary: "Reject this skill proposal.",
        },
        DEN_TASK_LISTS_LIST => ToolDisplayDescriptor {
            label,
            category: "task-list",
            progress_verb: "Listing task lists",
            complete_verb: "Listed task lists",
            target_arg_keys: &["owner_profile"],
            sensitive_arg_keys: &[],
            approval_summary: "Read visible task-list and planning context.",
        },
        DEN_TASK_LISTS_GET_STATUS => ToolDisplayDescriptor {
            label,
            category: "task-list",
            progress_verb: "Checking task-list status",
            complete_verb: "Checked task-list status",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read visible task-list status.",
        },
        DEN_TASK_LISTS_UPDATE => ToolDisplayDescriptor {
            label,
            category: "task-list",
            progress_verb: "Updating task list",
            complete_verb: "Updated task list",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &["summary", "items", "workspace_context"],
            approval_summary: "Update active task-list work state.",
        },
        DEN_TASK_LISTS_REQUEST_HANDOFF => ToolDisplayDescriptor {
            label,
            category: "task-list",
            progress_verb: "Requesting task-list handoff",
            complete_verb: "Requested task-list handoff",
            target_arg_keys: &["title", "task_list_id"],
            sensitive_arg_keys: &["summary", "requested_outcome", "constraints"],
            approval_summary: "Request reviewed promotion or reconciliation of task-list items.",
        },
        DEN_JOB_CREATE => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Creating job",
            complete_verb: "Created job",
            target_arg_keys: &["goal"],
            sensitive_arg_keys: &["criteria", "tasks"],
            approval_summary: "Create a durable job without executing it.",
        },
        DEN_JOB_LIST => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Listing Docket jobs",
            complete_verb: "Listed Docket jobs",
            target_arg_keys: &["status"],
            sensitive_arg_keys: &[],
            approval_summary: "Read Docket jobs for this Bear.",
        },
        DEN_JOB_GET => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Reading Docket job",
            complete_verb: "Read Docket job",
            target_arg_keys: &["job_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Read this Docket job and task tree.",
        },
        DEN_WORK_DISPATCH => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Dispatching work task",
            complete_verb: "Dispatched work task",
            target_arg_keys: &["task_id", "root"],
            sensitive_arg_keys: &[],
            approval_summary: "Queue this task for autonomous sandbox execution.",
        },
        DEN_WORK_RUN_LIST => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Listing work runs",
            complete_verb: "Listed work runs",
            target_arg_keys: &["job_id", "task_id", "state"],
            sensitive_arg_keys: &[],
            approval_summary: "Read work runs for this Bear.",
        },
        DEN_WORK_RUN_GET => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Reading work run",
            complete_verb: "Read work run",
            target_arg_keys: &["work_run_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Read this work run's status and results.",
        },
        DEN_WORK_RUN_CANCEL => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Cancelling work run",
            complete_verb: "Requested work run cancellation",
            target_arg_keys: &["work_run_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Request cancellation of this work run.",
        },
        DEN_WORK_RUN_RESOLVE_STALLED => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Recording stalled work-run resolution",
            complete_verb: "Recorded stalled work-run resolution",
            target_arg_keys: &["work_run_id"],
            sensitive_arg_keys: &["reason"],
            approval_summary: "Record how this stalled work run was resolved.",
        },
        DEN_WORK_CATALOG => ToolDisplayDescriptor {
            label,
            category: "work",
            progress_verb: "Reading work catalog",
            complete_verb: "Read work catalog",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read the sandbox provider's roots and image catalog.",
        },
        DEN_TASK_CREATE => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Creating Docket task",
            complete_verb: "Created Docket task",
            target_arg_keys: &["title", "job_id", "parent_task_id"],
            sensitive_arg_keys: &["body"],
            approval_summary: "Create a durable Docket task definition.",
        },
        DEN_TASK_LIST => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Listing Docket tasks",
            complete_verb: "Listed Docket tasks",
            target_arg_keys: &["job_id", "parent_task_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Read Docket task definitions and current run state.",
        },
        DEN_TASK_UPDATE_CURRENT_STATUS => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Marking task status",
            complete_verb: "Marked task status",
            target_arg_keys: &["status", "task_id", "result_summary"],
            sensitive_arg_keys: &["result_refs", "result_summary"],
            approval_summary: "Record this task's run-scoped status and result.",
        },
        DEN_DOCKET_ENTRY_APPEND => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Recording Docket entry",
            complete_verb: "Recorded Docket entry",
            target_arg_keys: &["kind", "summary", "task_id", "job_id"],
            sensitive_arg_keys: &["body", "evidence_refs"],
            approval_summary: "Append a durable Docket journal or notebook entry.",
        },
        DEN_DOCKET_ENTRY_PROMOTE => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Promoting Docket entry",
            complete_verb: "Promoted Docket entry",
            target_arg_keys: &["entry_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Reference a task-journal entry from its job notebook.",
        },
        DEN_DOCKET_ENTRY_LIST => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Listing Docket entries",
            complete_verb: "Listed Docket entries",
            target_arg_keys: &["task_id", "job_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Read durable Docket journal and notebook entries.",
        },
        DEN_TASK_UPDATE => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Updating Docket task",
            complete_verb: "Updated Docket task",
            target_arg_keys: &["task_id", "status"],
            sensitive_arg_keys: &[
                "body",
                "completion_criteria",
                "result_refs",
                "result_summary",
            ],
            approval_summary: "Update a Docket task definition or run-scoped state.",
        },
        DEN_TASK_LIST_CHECKOUT => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Checking out task list",
            complete_verb: "Checked out task list",
            target_arg_keys: &["job_id", "parent_task_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Create a Docket-backed task-list projection.",
        },
        DEN_TASK_LIST_SYNC => ToolDisplayDescriptor {
            label,
            category: "docket",
            progress_verb: "Syncing task list",
            complete_verb: "Synced task list",
            target_arg_keys: &[],
            sensitive_arg_keys: &["task_list"],
            approval_summary: "Sync checked-out Docket-backed task-list changes to Docket.",
        },
        DEN_PLAN_MODE_ENTER => ToolDisplayDescriptor {
            label,
            category: "plan",
            progress_verb: "Entering planning mode",
            complete_verb: "Entered planning mode",
            target_arg_keys: &[],
            sensitive_arg_keys: &["reason"],
            approval_summary: "Enter client planning mode.",
        },
        DEN_PLAN_MODE_STATUS => ToolDisplayDescriptor {
            label,
            category: "plan",
            progress_verb: "Checking planning mode",
            complete_verb: "Checked planning mode",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Read current planning gate state.",
        },
        DEN_PLAN_MODE_RECORD_APPROVAL => ToolDisplayDescriptor {
            label,
            category: "plan",
            progress_verb: "Recording plan approval",
            complete_verb: "Recorded plan approval",
            target_arg_keys: &["plan_mode_id"],
            sensitive_arg_keys: &["approval_text"],
            approval_summary: "Record explicit human approval for the submitted plan.",
        },
        DEN_PLAN_MODE_EXIT => ToolDisplayDescriptor {
            label,
            category: "plan",
            progress_verb: "Submitting implementation plan",
            complete_verb: "Submitted implementation plan",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &["body"],
            approval_summary: "Submit an implementation workplan for approval.",
        },
        DEN_PLAN_MODE_CANCEL => ToolDisplayDescriptor {
            label,
            category: "plan",
            progress_verb: "Cancelling planning mode",
            complete_verb: "Cancelled planning mode",
            target_arg_keys: &["plan_mode_id"],
            sensitive_arg_keys: &[],
            approval_summary: "Cancel the current planning gate.",
        },
        DEN_TASK_WRITE_INTENT => ToolDisplayDescriptor {
            label,
            category: "tasks",
            progress_verb: "Writing task intent",
            complete_verb: "Wrote task intent",
            target_arg_keys: &["title"],
            sensitive_arg_keys: &["summary", "requested_outcome", "constraints"],
            approval_summary: "Write a task intent for curate review.",
        },
        DEN_TASK_APPROVE_INTENT => ToolDisplayDescriptor {
            label,
            category: "tasks",
            progress_verb: "Approving task intent",
            complete_verb: "Approved task intent",
            target_arg_keys: &["task_id", "title"],
            sensitive_arg_keys: &["approved_scope", "review_notes"],
            approval_summary: "Approve this task intent.",
        },
        DEN_TASK_REJECT_INTENT => ToolDisplayDescriptor {
            label,
            category: "tasks",
            progress_verb: "Rejecting task intent",
            complete_verb: "Rejected task intent",
            target_arg_keys: &["source_intent_path"],
            sensitive_arg_keys: &["rejection_reason", "review_notes"],
            approval_summary: "Reject this task intent.",
        },
        DEN_CORE_WRITE_RESULT_SUMMARY => ToolDisplayDescriptor {
            label,
            category: "memory",
            progress_verb: "Writing core result summary",
            complete_verb: "Wrote core result summary",
            target_arg_keys: &["task_id", "run_id"],
            sensitive_arg_keys: &["summary", "durable_learnings"],
            approval_summary: "Write a reviewed result summary to core memory.",
        },
        DEN_OBSERVATION_WRITE => ToolDisplayDescriptor {
            label,
            category: "observations",
            progress_verb: "Writing observation",
            complete_verb: "Wrote observation",
            target_arg_keys: &["observation_id"],
            sensitive_arg_keys: &["summary", "payload_ref", "source"],
            approval_summary: "Write a watch observation.",
        },
        DEN_RUN_WRITE_RESULT => ToolDisplayDescriptor {
            label,
            category: "runs",
            progress_verb: "Writing run result",
            complete_verb: "Wrote run result",
            target_arg_keys: &["task_id", "run_id", "status"],
            sensitive_arg_keys: &["summary", "result", "follow_up"],
            approval_summary: "Write a work run result.",
        },
        _ => ToolDisplayDescriptor {
            label,
            category: "den",
            progress_verb: "Using",
            complete_verb: "Used",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Use this Den tool.",
        },
    }
}

fn tool_domain(name: &str) -> &'static str {
    match name {
        DEN_CABINET_SEARCH | DEN_CABINET_READ | DEN_CABINET_CREATE | DEN_CABINET_UPDATE => {
            "cabinet"
        }
        DEN_PLAN_MODE_ENTER
        | DEN_PLAN_MODE_STATUS
        | DEN_PLAN_MODE_RECORD_APPROVAL
        | DEN_PLAN_MODE_EXIT
        | DEN_PLAN_MODE_CANCEL => "workplan",
        DEN_TASK_LISTS_LIST
        | DEN_TASK_LISTS_GET_STATUS
        | DEN_TASK_LISTS_UPDATE
        | DEN_TASK_LISTS_REQUEST_HANDOFF => "activity",
        DEN_JOB_CREATE
        | DEN_JOB_LIST
        | DEN_JOB_GET
        | DEN_JOB_UPDATE
        | DEN_JOB_EXECUTE
        | DEN_JOB_EVALUATE_CRITERION
        | DEN_TASK_CREATE
        | DEN_TASK_LIST
        | DEN_TASK_UPDATE
        | DEN_TASK_LIST_SYNC
        | DEN_TASK_LIST_CHECKOUT => "docket",
        DEN_MEMORY_WRITE_ENTRY
        | DEN_MEMORY_STATUS
        | DEN_MEMORY_TREE
        | DEN_MEMORY_READ
        | DEN_MEMORY_SEARCH
        | DEN_MEMORY_ORIENT_WORK_SURFACE
        | DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD
        | DEN_MEMORY_REQUEST_REVIEW
        | DEN_PROMPT_MEMORY_UPSERT
        | DEN_PROMPT_MEMORY_LIST
        | DEN_PROMPT_MEMORY_PATCH
        | DEN_MEMORY_LIST_PROPOSALS
        | DEN_MEMORY_READ_PROPOSAL
        | DEN_MEMORY_RESOLVE_PROPOSAL
        | DEN_MEMORY_APPLY_CORE_UPDATE
        | DEN_MEMORY_MARK_LIFECYCLE => "memory",
        DEN_CONVERSATION_SET_TITLE
        | DEN_WEB_FETCH
        | DEN_WEB_SEARCH
        | DEN_BEAR_ENVIRONMENT
        | DEN_SITUATION_GET => "execution",
        _ => "execution",
    }
}

fn tool_content_class(name: &str) -> Option<&'static str> {
    match name {
        DEN_MEMORY_WRITE_ENTRY => Some("semantic_memory"),
        DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD => Some("semantic_memory"),
        DEN_MEMORY_REQUEST_REVIEW => Some("semantic_memory"),
        DEN_BEAR_ENVIRONMENT => Some("activity_status"),
        DEN_PLAN_MODE_EXIT => Some("workplan_artifact"),
        DEN_TASK_LISTS_UPDATE => Some("activity_status"),
        DEN_TASK_LISTS_REQUEST_HANDOFF => Some("task_intent"),
        DEN_MEMORY_APPLY_CORE_UPDATE => Some("core_update"),
        DEN_MEMORY_MARK_LIFECYCLE => Some("semantic_memory"),
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
            "suggested_action": { "type": "string", "enum": ["unspecified", "summarize_into_core", "promote_to_core", "cabinet_update", "skill_review", "retain_profile_local", "delete_after_review", "human_review", "archive_index", "task_context"] },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_acp_surface_injects_eligible_docket_execution_controls() {
        let provider_names: std::collections::HashSet<_> =
            builtin_den_tool_descriptors_for_pair_acp_surface()
                .into_iter()
                .map(|descriptor| descriptor.provider_name)
                .collect();

        assert!(provider_names.contains(DEN_JOB_RECONCILE_PROVIDER));
        assert!(provider_names.contains(DEN_JOB_SETTLE_TASK_PROVIDER));
    }

    #[test]
    fn rust_dependency_preparation_is_work_only_and_narrowly_typed() {
        let descriptor = builtin_den_tool_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.name == DEN_WORK_PREPARE_RUST_DEPENDENCIES)
            .expect("descriptor");

        assert_eq!(descriptor.provider_name, "prepare_rust_dependencies");
        assert_eq!(descriptor.execution_target, "den");
        assert_eq!(descriptor.allowed_roles, WORK_PROFILES);
        assert_eq!(descriptor.input_schema["additionalProperties"], false);
        assert_eq!(
            descriptor.input_schema["properties"]["resolution"]["enum"],
            json!(["locked", "update_lockfile"])
        );
        assert_eq!(
            descriptor.input_schema["properties"]["preparation"]["enum"],
            json!(["check", "test_no_run"])
        );
    }

    #[test]
    fn work_dispatch_descriptor_explains_isolation_and_publication() {
        let descriptor = builtin_den_tool_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.name == DEN_WORK_DISPATCH)
            .expect("descriptor");

        assert!(descriptor
            .description
            .contains("isolated background execution in a sandbox"));
        assert!(descriptor
            .description
            .contains("never modifies Pair's attached checkout"));
        assert!(descriptor
            .description
            .contains("managed source or sandbox-provider root"));
    }

    #[test]
    fn resolve_stalled_work_run_descriptor_is_operator_facing() {
        let descriptor = builtin_den_tool_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.name == DEN_WORK_RUN_RESOLVE_STALLED)
            .expect("descriptor");

        assert_eq!(descriptor.provider_name, "resolve_stalled_work_run");
        assert_eq!(descriptor.allowed_roles, CHAT_AND_PAIR_PROFILES);
        assert_eq!(descriptor.input_schema["required"], json!(["work_run_id"]));
        assert_eq!(descriptor.input_schema["additionalProperties"], false);
    }

    #[test]
    fn den_tool_display_includes_conversation_title_target() {
        let display = den_tool_display_json_for_provider(
            "set_conversation_title",
            &json!({ "title": "Trace ACP tool card title rendering" }),
        )
        .expect("display");

        assert_eq!(
            display["title"],
            "Setting conversation title Trace ACP tool card title rendering"
        );
        assert_eq!(display["subtitle"], "Trace ACP tool card title rendering");
        assert_ne!(display["subtitle"], "conversation");
    }

    #[test]
    fn den_tool_display_includes_job_goal_target() {
        let display = den_tool_display_json_for_provider(
            "create_job",
            &json!({ "goal": "Improve ACP tool card summaries" }),
        )
        .expect("display");

        assert_eq!(display["label"], "Create job");
        assert_eq!(
            display["title"],
            "Creating job Improve ACP tool card summaries"
        );
        assert_eq!(display["subtitle"], "Improve ACP tool card summaries");
        assert_eq!(display["category"], "work");
    }

    #[test]
    fn den_tool_display_uses_short_display_paths_for_target_paths() {
        let display = den_tool_display_json_for_provider(
            "memory_apply_core_update",
            &json!({
                "target_path": "/workspace/project/core/decisions.md",
                "mode": "append_section"
            }),
        )
        .expect("display");

        assert_eq!(
            display["title"],
            "Applying core memory update …/project/core/decisions.md → append_section"
        );
        assert_eq!(
            display["subtitle"],
            "…/project/core/decisions.md → append_section"
        );
    }
}

fn prompt_memory_upsert_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "block_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "scope": { "type": "string", "enum": ["bear_wide", "profile_local", "work_surface", "session"] },
            "block_type": { "type": "string", "enum": ["profile_guidance", "work_surface_context", "session_focus", "user_instruction"] },
            "state": { "type": "string", "enum": ["draft", "active", "superseded", "archived"] },
            "work_surface": { "type": "string", "maxLength": 500 },
            "session_id": { "type": "string", "maxLength": 200 },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "body": { "type": "string", "minLength": 1, "maxLength": 50000 },
            "priority": { "type": "integer", "minimum": -1000, "maximum": 1000 },
            "supersedes_block_id": { "type": "string", "maxLength": 200 },
            "metadata": { "type": "object" }
        },
        "required": ["block_id", "scope", "block_type", "title", "body"],
        "additionalProperties": false
    })
}

fn prompt_memory_patch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "block_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "state": { "type": "string", "enum": ["draft", "active", "superseded", "archived"] },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "body": { "type": "string", "minLength": 1, "maxLength": 50000 },
            "priority": { "type": "integer", "minimum": -1000, "maximum": 1000 },
            "supersedes_block_id": { "type": "string", "maxLength": 200 },
            "metadata": { "type": "object" }
        },
        "required": ["block_id", "title", "body"],
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
                    "status": { "type": "string", "enum": ["active", "superseded", "stale", "archived", "archive-candidate"] }
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
            },
            "semantic_confirmation_token": { "type": "string", "minLength": 1, "maxLength": 2000 }
        },
        "required": ["kind", "title", "body"],
        "additionalProperties": false
    })
}

fn entity_browse_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_type": {
                "type": "string",
                "enum": ["person", "org", "event", "mission", "domain", "work_surface", "connection", "artifact", "place"]
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
        },
        "additionalProperties": false
    })
}

fn entity_resolve_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "include_relations": { "type": "boolean" },
            "include_handles": { "type": "boolean" }
        },
        "required": ["entity_id"],
        "additionalProperties": false
    })
}

fn entity_link_memory_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "memory_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "entity_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "relation": { "type": "string", "enum": ["subject", "source", "participant", "applies_when"] },
            "qualifiers": { "type": "object" },
            "confidence": { "type": "string", "maxLength": 80 }
        },
        "required": ["memory_id", "entity_id", "relation"],
        "additionalProperties": false
    })
}

fn entity_merge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "survivor_entity_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "loser_entity_id": { "type": "string", "minLength": 1, "maxLength": 200 }
        },
        "required": ["survivor_entity_id", "loser_entity_id"],
        "additionalProperties": false
    })
}

fn entity_split_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "new_entity_type": { "type": "string", "enum": ["person", "org", "event", "mission", "domain", "work_surface", "connection", "artifact"] },
            "display_name": { "type": "string", "maxLength": 200 },
            "handle_ids_to_move": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "maxLength": 200 },
                "minItems": 1,
                "maxItems": 50
            },
            "resolution": { "type": "string", "enum": ["observed", "provisional", "resolved", "confirmed"] },
            "trust": { "type": "string", "enum": ["inferred", "asserted"] }
        },
        "required": ["new_entity_type", "handle_ids_to_move"],
        "additionalProperties": false
    })
}

fn entity_write_access_rule_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "memory_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "entity_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "relation": { "type": "string", "enum": ["audience", "confined_to"] },
            "qualifiers": { "type": "object" },
            "confidence": { "type": "string", "maxLength": 80 }
        },
        "required": ["memory_id", "entity_id", "relation"],
        "additionalProperties": false
    })
}

fn entity_write_anchor_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_id": { "type": "string", "minLength": 1, "maxLength": 200 },
            "kind": { "type": "string", "enum": ["profile", "overview", "index"] },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "body": { "type": "string", "minLength": 1, "maxLength": 50000 },
            "salience": { "type": "string", "enum": ["low", "normal", "high", "critical"] },
            "supersedes_memory_id": { "type": "string", "maxLength": 200 }
        },
        "required": ["entity_id", "kind", "title", "body"],
        "additionalProperties": false
    })
}

impl DenToolDescriptor {
    pub fn allows_profile(&self, role: BearProfile) -> bool {
        self.allowed_roles
            .iter()
            .any(|allowed| *allowed == role.as_str())
    }
}

impl<'de> Deserialize<'de> for DenToolDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDenToolDescriptor {
            name: String,
            provider_name: String,
            provider_aliases: Vec<String>,
            label: String,
            description: String,
            kind: String,
            provider: String,
            execution_target: String,
            scope: String,
            domain: String,
            content_class: Option<String>,
            availability: String,
            permissions: Vec<String>,
            allowed_roles: Vec<String>,
            approval_policy: String,
            display: Value,
            input_schema: Value,
        }

        let raw = RawDenToolDescriptor::deserialize(deserializer)?;
        let name: &'static str = Box::leak(raw.name.into_boxed_str());
        let provider_name = raw.provider_name;
        let provider_aliases: &'static [&'static str] = Box::leak(
            raw.provider_aliases
                .into_iter()
                .map(|item| Box::leak(item.into_boxed_str()) as &'static str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let label: &'static str = Box::leak(raw.label.into_boxed_str());
        let description: &'static str = Box::leak(raw.description.into_boxed_str());
        let kind: &'static str = Box::leak(raw.kind.into_boxed_str());
        let provider: &'static str = Box::leak(raw.provider.into_boxed_str());
        let execution_target: &'static str = Box::leak(raw.execution_target.into_boxed_str());
        let scope: &'static str = Box::leak(raw.scope.into_boxed_str());
        let domain: &'static str = Box::leak(raw.domain.into_boxed_str());
        let content_class = raw
            .content_class
            .map(|value| Box::leak(value.into_boxed_str()) as &'static str);
        let availability: &'static str = Box::leak(raw.availability.into_boxed_str());
        let permissions: &'static [&'static str] = Box::leak(
            raw.permissions
                .into_iter()
                .map(|item| Box::leak(item.into_boxed_str()) as &'static str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let allowed_roles: &'static [&'static str] = Box::leak(
            raw.allowed_roles
                .into_iter()
                .map(|item| Box::leak(item.into_boxed_str()) as &'static str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let approval_policy: &'static str = Box::leak(raw.approval_policy.into_boxed_str());

        Ok(Self {
            name,
            provider_name,
            provider_aliases,
            label,
            description,
            kind,
            provider,
            execution_target,
            scope,
            domain,
            content_class,
            availability,
            permissions,
            allowed_roles,
            approval_policy,
            display: raw.display,
            input_schema: raw.input_schema,
        })
    }
}

#[cfg(test)]
mod guidance_test;
