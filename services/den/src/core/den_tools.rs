use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    core::{
        bears::{db as bears_db, db::role_is_bear_admin, BearAgentRole},
        user,
    },
    errors::CustomError,
};

pub const DEN_BEAR_GET_SELF: &str = "den.bear.get_self";
pub const DEN_USER_GET_CURRENT: &str = "den.user.get_current";
pub const DEN_BEAR_LIST_MEMBERS: &str = "den.bear.list_members";
pub const DEN_CAPABILITIES_LIST_SELF: &str = "den.capabilities.list_self";
pub const DEN_CHANNEL_GET_CONTEXT: &str = "den.channel.get_context";
pub const DEN_POLICY_GET_SELF: &str = "den.policy.get_self";
pub const DEN_SKILL_PROPOSE: &str = "den.skill.propose";
pub const DEN_WORK_PLAN_LIST: &str = "den.work_plan.list";
pub const DEN_WORK_PLAN_GET_STATUS: &str = "den.work_plan.get_status";
pub const DEN_WORK_PLAN_UPDATE: &str = "den.work_plan.update";
pub const DEN_WORK_PLAN_REQUEST_HANDOFF: &str = "den.work_plan.request_handoff";
pub const DEN_TASK_WRITE_INTENT: &str = "den.task.write_intent";
pub const DEN_OBSERVATION_WRITE: &str = "den.observation.write";
pub const DEN_RUN_WRITE_RESULT: &str = "den.run.write_result";

const ALL_ROLES: &[&str] = &["talk", "pair", "curate", "work", "watch"];
const WORK_PLAN_READ_ROLES: &[&str] = &["talk", "pair", "curate", "work"];
const WORK_PLAN_UPDATE_ROLES: &[&str] = &["talk", "pair", "work"];
const TALK_AND_PAIR_ROLES: &[&str] = &["talk", "pair"];
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
            | DEN_SKILL_PROPOSE
            | DEN_WORK_PLAN_LIST
            | DEN_WORK_PLAN_GET_STATUS
            | DEN_WORK_PLAN_UPDATE
            | DEN_WORK_PLAN_REQUEST_HANDOFF
            | DEN_TASK_WRITE_INTENT
            | DEN_OBSERVATION_WRITE
            | DEN_RUN_WRITE_RESULT
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DenToolInvocationContext {
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub role_agent_id: String,
    pub user_id: i32,
    pub username: Option<String>,
    pub membership_role: Option<String>,
    pub conversation_id: String,
    pub session_id: String,
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

pub async fn invoke_den_tool(
    pool: &PgPool,
    tool_name: &str,
    _arguments: Value,
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
        DEN_SKILL_PROPOSE
        | DEN_WORK_PLAN_LIST
        | DEN_WORK_PLAN_GET_STATUS
        | DEN_WORK_PLAN_UPDATE
        | DEN_WORK_PLAN_REQUEST_HANDOFF
        | DEN_TASK_WRITE_INTENT
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
    let role = row
        .ok_or_else(|| {
            CustomError::Authorization("role_agent_id is not registered for this bear".to_string())
        })?
        .0
        .parse()
        .map_err(CustomError::System)?;
    Ok(role)
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
    fn role_authorization_rejects_disallowed_tools() {
        assert!(authorize_tool_for_role(DEN_TASK_WRITE_INTENT, BearAgentRole::Talk).is_ok());
        assert!(authorize_tool_for_role(DEN_TASK_WRITE_INTENT, BearAgentRole::Watch).is_err());
        assert!(authorize_tool_for_role(DEN_RUN_WRITE_RESULT, BearAgentRole::Work).is_ok());
        assert!(authorize_tool_for_role(DEN_RUN_WRITE_RESULT, BearAgentRole::Talk).is_err());
        assert!(authorize_tool_for_role(DEN_WORK_PLAN_UPDATE, BearAgentRole::Pair).is_ok());
        assert!(authorize_tool_for_role(DEN_WORK_PLAN_UPDATE, BearAgentRole::Watch).is_err());
        assert!(
            authorize_tool_for_role(DEN_WORK_PLAN_REQUEST_HANDOFF, BearAgentRole::Work).is_err()
        );
    }
}
