use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    core::{
        bears::{db as bears_db, db::role_is_bear_admin},
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
        ),
        descriptor(
            DEN_USER_GET_CURRENT,
            "Current user",
            "Return Den's trusted profile for the current user in this interaction.",
            "session",
            &["user.current.read"],
        ),
        descriptor(
            DEN_BEAR_LIST_MEMBERS,
            "Bear members",
            "List users who have access to the current bear, with policy redaction.",
            "bear",
            &["bear.members.read"],
        ),
        descriptor(
            DEN_CAPABILITIES_LIST_SELF,
            "Available Den capabilities",
            "List Den-managed tools available to the current bear/session.",
            "session",
            &["capabilities.read"],
        ),
        descriptor(
            DEN_CHANNEL_GET_CONTEXT,
            "Channel context",
            "Return trusted Den/Codepool channel and session context for this interaction.",
            "session",
            &["channel.context.read"],
        ),
        descriptor(
            DEN_POLICY_GET_SELF,
            "Current policy",
            "Explain current user and bear policy for this interaction.",
            "session",
            &["policy.read"],
        ),
    ]
}

fn descriptor(
    name: &'static str,
    label: &'static str,
    description: &'static str,
    scope: &'static str,
    permissions: &'static [&'static str],
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
        approval_policy: "never",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
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
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DenToolInvocationContext {
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub letta_agent_id: String,
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
    authorize_context(pool, &context).await?;
    match tool_name {
        DEN_BEAR_GET_SELF => get_bear_self(pool, &context).await,
        DEN_USER_GET_CURRENT => get_current_user(pool, &context).await,
        DEN_BEAR_LIST_MEMBERS => list_bear_members(pool, &context).await,
        DEN_CAPABILITIES_LIST_SELF => list_capabilities_self(pool, &context).await,
        DEN_CHANNEL_GET_CONTEXT => Ok(channel_context(&context)),
        DEN_POLICY_GET_SELF => policy_self(pool, &context).await,
        _ => Err(CustomError::NotFound(format!(
            "unknown Den tool: {tool_name}"
        ))),
    }
}

async fn authorize_context(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<(), CustomError> {
    if !bears_db::user_may_use_bear(pool, context.user_id, context.bear_id).await? {
        return Err(CustomError::Authorization(
            "user is not a member of this bear".to_string(),
        ));
    }
    Ok(())
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
            "letta_agent_id": bear.letta_agent_id,
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
    let _ = pool;
    let descriptors = builtin_den_tool_descriptors();
    Ok(json!({
        "bear_id": context.bear_id,
        "channel": context.channel,
        "capabilities": descriptors,
    }))
}

fn channel_context(context: &DenToolInvocationContext) -> Value {
    json!({
        "bear_id": context.bear_id,
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
