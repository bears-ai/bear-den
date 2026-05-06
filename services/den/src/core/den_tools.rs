use std::net::{IpAddr, ToSocketAddrs};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    config::Config,
    core::{
        bears::{db as bears_db, db::role_is_bear_admin, BearAgentRole},
        memory_manager_head::{write_memfs_role_note, MemfsWriteRoleNoteRequest},
        user,
        work_plans::{
            self, WorkPlanListFilter, WorkPlanLookup, WorkPlanStatus, WorkPlanUpdate,
            WorkPlanUpsert, WorkPlanVisibility,
        },
    },
    errors::CustomError,
};

pub const DEN_BEAR_GET_SELF: &str = "den.bear.get_self";
pub const DEN_USER_GET_CURRENT: &str = "den.user.get_current";
pub const DEN_BEAR_LIST_MEMBERS: &str = "den.bear.list_members";
pub const DEN_CAPABILITIES_LIST_SELF: &str = "den.capabilities.list_self";
pub const DEN_CHANNEL_GET_CONTEXT: &str = "den.channel.get_context";
pub const DEN_POLICY_GET_SELF: &str = "den.policy.get_self";
pub const DEN_WEB_FETCH: &str = "den.web.fetch";
pub const DEN_WEB_SEARCH: &str = "den.web.search";
pub const DEN_WRITE_NOTE: &str = "den.write_note";
pub const DEN_SKILL_PROPOSE: &str = "den.skill.propose";
pub const DEN_SKILL_APPROVE_PROPOSAL: &str = "den.skill.approve_proposal";
pub const DEN_SKILL_REJECT_PROPOSAL: &str = "den.skill.reject_proposal";
pub const DEN_WORK_PLAN_LIST: &str = "den.work_plan.list";
pub const DEN_WORK_PLAN_GET_STATUS: &str = "den.work_plan.get_status";
pub const DEN_WORK_PLAN_UPDATE: &str = "den.work_plan.update";
pub const DEN_WORK_PLAN_REQUEST_HANDOFF: &str = "den.work_plan.request_handoff";
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
const CURATE_ROLES: &[&str] = &["curate"];
const WATCH_ROLES: &[&str] = &["watch"];
const WORK_ROLES: &[&str] = &["work"];

pub fn provider_safe_tool_name(name: &str) -> String {
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
    pub label: &'static str,
    pub description: &'static str,
    pub kind: &'static str,
    pub provider: &'static str,
    pub execution_target: &'static str,
    pub scope: &'static str,
    pub availability: &'static str,
    pub permissions: &'static [&'static str],
    pub allowed_roles: &'static [&'static str],
    pub approval_policy: &'static str,
    pub input_schema: Value,
}

pub fn builtin_den_tool_descriptors() -> Vec<DenToolDescriptor> {
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
            DEN_WRITE_NOTE,
            "Write pair note",
            "Write a role-scoped durable note for the current pair role under pair/notes/ in MemFS.",
            "bear.memory",
            &["memory.note.write"],
            PAIR_ROLES,
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "source": { "type": "object" }
                },
                "required": ["title", "body"],
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
            "List work plans",
            "List visible Den workboard plans for the current bear with role-safe projection.",
            "bear.work_plans",
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
                    "include_archived": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            DEN_WORK_PLAN_GET_STATUS,
            "Get work plan status",
            "Return current status for one visible Den workboard plan or this session's active plan.",
            "bear.work_plans",
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
            "Update work plan",
            "Create or update the current role's live Den workboard plan for user-visible task planning.",
            "bear.work_plans",
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
            "Request conversion of selected live plan items into a schema-validated task intent for curate review.",
            "bear.work_plans",
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
        label,
        description,
        kind: "server_tool",
        provider: "den",
        execution_target: "den",
        scope,
        availability: "available",
        permissions,
        allowed_roles,
        approval_policy: "never",
        input_schema,
    }
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

impl DenToolDescriptor {
    pub fn allows_role(&self, role: BearAgentRole) -> bool {
        self.allowed_roles.contains(&role.as_str())
    }
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
            | DEN_WEB_FETCH
            | DEN_WEB_SEARCH
            | DEN_WRITE_NOTE
            | DEN_SKILL_PROPOSE
            | DEN_SKILL_APPROVE_PROPOSAL
            | DEN_SKILL_REJECT_PROPOSAL
            | DEN_WORK_PLAN_LIST
            | DEN_WORK_PLAN_GET_STATUS
            | DEN_WORK_PLAN_UPDATE
            | DEN_WORK_PLAN_REQUEST_HANDOFF
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
struct WriteNoteArguments {
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source: Option<Value>,
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
    let role = authorize_context(pool, &context).await?;
    authorize_tool_for_role(tool_name, role)?;
    match tool_name {
        DEN_BEAR_GET_SELF => get_bear_self(pool, &context).await,
        DEN_USER_GET_CURRENT => get_current_user(pool, &context).await,
        DEN_BEAR_LIST_MEMBERS => list_bear_members(pool, &context).await,
        DEN_CAPABILITIES_LIST_SELF => list_capabilities_self(pool, &context).await,
        DEN_CHANNEL_GET_CONTEXT => Ok(channel_context(&context)),
        DEN_POLICY_GET_SELF => policy_self(pool, &context).await,
        DEN_WEB_FETCH => web_fetch(arguments).await,
        DEN_WEB_SEARCH => web_search(config, arguments).await,
        DEN_WRITE_NOTE => write_note(pool, config, &context, role, arguments).await,
        DEN_WORK_PLAN_LIST => list_work_plans(pool, &context, role, arguments).await,
        DEN_WORK_PLAN_GET_STATUS => get_work_plan_status(pool, &context, role, arguments).await,
        DEN_WORK_PLAN_UPDATE => update_work_plan(pool, &context, role, arguments).await,
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

async fn web_fetch(arguments: Value) -> Result<Value, CustomError> {
    let args: WebFetchArguments = serde_json::from_value(arguments)?;
    let max_chars = args.max_chars.unwrap_or(8_000).clamp(1, 20_000);
    let url = validate_public_http_url(&args.url)?;
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
    Ok(json!({
        "url": final_url.as_str(),
        "status": status.as_u16(),
        "content_type": content_type,
        "text_excerpt": text_excerpt,
        "truncated": bytes_truncated || char_truncated,
    }))
}

async fn web_search(config: &Config, arguments: Value) -> Result<Value, CustomError> {
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
    match config.den_search_provider.as_str() {
        "brave" => brave_web_search(config, args.query.trim(), max_results).await,
        "" => Err(CustomError::System(format!(
            "den.web.search is registered but DEN_SEARCH_PROVIDER is not configured (query={}, max_results={max_results}). Set DEN_SEARCH_PROVIDER=brave and BRAVE_SEARCH_API_KEY.",
            serde_json::Value::String(args.query.trim().to_string())
        ))),
        other => Err(CustomError::System(format!(
            "unsupported DEN_SEARCH_PROVIDER={other:?}; supported providers: brave"
        ))),
    }
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
        "note": "Search snippets are untrusted external content. Use den.web.fetch on selected URLs for bounded page content."
    }))
}

async fn write_note(
    _pool: &PgPool,
    config: &Config,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    if role != BearAgentRole::Pair {
        return Err(CustomError::Authorization(
            "den.write_note is currently available only to the pair role".to_string(),
        ));
    }
    let args: WriteNoteArguments = serde_json::from_value(arguments)?;
    let title = args.title.trim();
    let body = args.body.trim();
    if title.is_empty() {
        return Err(CustomError::ValidationError(
            "title must not be empty".to_string(),
        ));
    }
    if body.is_empty() {
        return Err(CustomError::ValidationError(
            "body must not be empty".to_string(),
        ));
    }
    let tags = args
        .tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .take(20)
        .collect::<Vec<_>>();
    let request = MemfsWriteRoleNoteRequest {
        title: title.to_string(),
        body: body.to_string(),
        tags,
        source: args.source,
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
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| CustomError::System(format!("MemFS note client build failed: {e}")))?;
    let response = write_memfs_role_note(
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
        "path": response.path,
        "commit": response.commit,
        "canonical_tip": response.canonical_tip,
        "view": response.view,
    }))
}

async fn list_work_plans(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearAgentRole,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: WorkPlanListArguments = serde_json::from_value(arguments)?;
    let plans = work_plans::list_visible_work_plans(
        pool,
        context.bear_id,
        role,
        context.user_id,
        WorkPlanListFilter {
            statuses: args.statuses,
            owner_role: args.owner_role,
            include_archived: args.include_archived,
        },
    )
    .await?;
    Ok(json!({
        "bear_id": context.bear_id,
        "plans": plans,
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
        "bear_id": context.bear_id,
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
        "bear_id": context.bear_id,
        "plan": plan,
    }))
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
    fn pair_has_web_search_and_fetch_tools() {
        let pair = names_for_role(BearAgentRole::Pair);
        assert!(pair.contains(DEN_WEB_FETCH));
        assert!(pair.contains(DEN_WEB_SEARCH));
        assert!(pair.contains(DEN_WRITE_NOTE));

        let talk = names_for_role(BearAgentRole::Talk);
        assert!(!talk.contains(DEN_WEB_FETCH));
        assert!(!talk.contains(DEN_WEB_SEARCH));
        assert!(!talk.contains(DEN_WRITE_NOTE));
    }

    #[tokio::test]
    async fn web_search_reports_missing_provider_config() {
        let config = Config::test_stub();
        let err = web_search(&config, json!({ "query": "rust docs" }))
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
