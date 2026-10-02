//! Identity / membership / policy tool executors + dispatcher authorization.
//!
//! These executors own the JSON shaping for the `bear/*`, `user/*`, `policy/*`,
//! `capabilities/*`, and `channel/*` read tools, plus the dispatcher's
//! authorization (`authorize_context`/`context_role`/`authorize_tool_for_profile`).
//! All DB access flows through the [`BearDirectory`] seam.

mod store;

pub use store::{
    role_is_bear_admin, BearDirectory, BearMemberRecord, BearRecord, CurrentUser, BEAR_ROLE_ADMIN,
};

use serde_json::{json, Value};

use crate::{
    ArmatureAvailability, BearCapability, BearProfile, DenError, EffectivePolicy, Governance,
    TurnExecutionOrigin,
};

use crate::tools::{
    capability_catalog::{
        code_mode_capability, describe_capability, search_capabilities, session_capability_entries,
        tool_descriptor_to_capability, CapabilityDescribeArguments, CapabilitySearchArguments,
    },
    context::DenToolInvocationContext,
    descriptor::{
        builtin_den_tool_descriptors, builtin_den_tool_descriptors_for_origin,
        builtin_den_tool_descriptors_for_profile, ToolAudience,
    },
};

/// Pure: the trusted channel-context echo for `channel_get_context`.
pub fn channel_context(context: &DenToolInvocationContext) -> Value {
    json!({
        "bear_id": context.bear_id,
        "binding_id": context.binding_id,
        "profile": context.profile,
        "user_id": context.user_id,
        "conversation_id": context.conversation_id,
        "session_id": context.session_id,
        "request_id": context.request_id,
        "channel": context.channel,
    })
}

/// Pure: the callable-tool descriptors for the caller's resolved role.
pub fn list_capabilities_self(context: &DenToolInvocationContext, role: BearProfile) -> Value {
    let descriptors = builtin_den_tool_descriptors_for_profile(role);
    json!({
        "bear_id": context.bear_id,
        "channel": context.channel,
        "capabilities": descriptors,
    })
}

/// Native model-facing catalog from the same typed descriptor audiences used
/// for advertisement and execution. Client-local instances additionally need
/// an available connected armature; a forwarded list alone is never enough.
pub fn list_capabilities_for_origin(
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
) -> Value {
    json!({
        "bear_id": context.bear_id,
        "channel": context.channel,
        "capabilities": builtin_den_tool_descriptors_for_origin(origin),
    })
}

pub fn capability_entries_for_origin(
    origin: TurnExecutionOrigin,
    context: &DenToolInvocationContext,
) -> Vec<crate::tools::capability_catalog::CapabilityEntry> {
    let mut entries: Vec<_> = builtin_den_tool_descriptors_for_origin(origin)
        .into_iter()
        .map(tool_descriptor_to_capability)
        .collect();
    entries.push(code_mode_capability(
        ToolAudience::from_origin(origin).compatibility_profile(),
    ));
    if matches!(
        origin,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected)
            | TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected)
    ) && EffectivePolicy::compile_for_origin(origin, Governance::Interactive)
        .capabilities
        .contains(BearCapability::UseArmatureTools)
    {
        entries.extend(session_capability_entries(&context.session_capabilities));
    }
    entries
}

pub fn capability_search_for_origin(
    arguments: Value,
    origin: TurnExecutionOrigin,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let args: CapabilitySearchArguments = serde_json::from_value(arguments).map_err(|err| {
        DenError::ValidationError(format!("invalid capability_search arguments: {err}"))
    })?;
    Ok(search_capabilities(
        &capability_entries_for_origin(origin, context),
        args,
    ))
}

pub fn capability_describe_for_origin(
    arguments: Value,
    origin: TurnExecutionOrigin,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let args: CapabilityDescribeArguments = serde_json::from_value(arguments).map_err(|err| {
        DenError::ValidationError(format!("invalid capability_describe arguments: {err}"))
    })?;
    describe_capability(&capability_entries_for_origin(origin, context), &args.r#ref)
        .ok_or_else(|| DenError::NotFound(format!("unknown capability: {}", args.r#ref)))
}

/// Pure: searchable capability catalog entries for the caller's resolved role.
pub fn capability_entries_for_role(
    role: BearProfile,
) -> Vec<crate::tools::capability_catalog::CapabilityEntry> {
    let mut entries: Vec<_> = builtin_den_tool_descriptors_for_profile(role)
        .into_iter()
        .map(tool_descriptor_to_capability)
        .collect();
    entries.push(code_mode_capability(role));
    entries
}

pub fn capability_entries_for_context(
    role: BearProfile,
    context: &DenToolInvocationContext,
) -> Vec<crate::tools::capability_catalog::CapabilityEntry> {
    let mut entries = capability_entries_for_role(role);
    entries.extend(session_capability_entries(&context.session_capabilities));
    entries
}

/// Pure: search the caller-visible Capability Catalog.
pub fn capability_search(
    arguments: Value,
    role: BearProfile,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let args: CapabilitySearchArguments = serde_json::from_value(arguments).map_err(|err| {
        DenError::ValidationError(format!("invalid capability_search arguments: {err}"))
    })?;
    let entries = capability_entries_for_context(role, context);
    Ok(search_capabilities(&entries, args))
}

/// Pure: describe one caller-visible Capability Catalog entry.
pub fn capability_describe(
    arguments: Value,
    role: BearProfile,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let args: CapabilityDescribeArguments = serde_json::from_value(arguments).map_err(|err| {
        DenError::ValidationError(format!("invalid capability_describe arguments: {err}"))
    })?;
    let entries = capability_entries_for_context(role, context);
    describe_capability(&entries, &args.r#ref)
        .ok_or_else(|| DenError::NotFound(format!("unknown capability: {}", args.r#ref)))
}

pub async fn get_bear_self(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let bear = dir
        .bear_self(context.bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;
    let member_count = dir.member_count(context.bear_id).await?;
    Ok(json!({
        "bear": {
            "bear_id": bear.id,
            "slug": bear.slug,
            "name": bear.name,
            "description": bear.description,
                        "default_model": bear.default_model,
                        "member_count": member_count,
            "created_at": bear.created_at,
            "updated_at": bear.updated_at
        },
        "viewer": {
            "user_id": context.user_id,
            "username": context.username,
            "membership_role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref())
        }
    }))
}

pub async fn get_current_user(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let current = dir.current_user(context.user_id).await?;
    Ok(json!({
        "user": {
            "user_id": current.id,
            "username": current.username,
            "display_name": current.display_name,
            "email_verified": current.email_verified,
            "created_at": current.created_at
        },
        "bear_membership": {
            "bear_id": context.bear_id,
            "role": context.membership_role,
            "is_bear_admin": role_is_bear_admin(context.membership_role.as_deref())
        }
    }))
}

pub async fn list_bear_members(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let members = dir.members(context.bear_id).await?;
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

pub async fn policy_self(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<Value, DenError> {
    let member_count = dir.member_count(context.bear_id).await?;
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

/// Resolve and verify the caller's role from its `binding_id`.
pub async fn context_role(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<BearProfile, DenError> {
    let agent_id = context.binding_id.trim();
    if agent_id.is_empty() {
        return Err(DenError::Authorization(
            "Den tool context is missing binding_id".to_string(),
        ));
    }
    let registered_profile = dir
        .registered_profile(context.bear_id, agent_id)
        .await?
        .ok_or_else(|| {
            DenError::Authorization("binding_id is not registered for this bear".to_string())
        })?;
    if let Some(declared_profile) = context.profile {
        if declared_profile != registered_profile {
            return Err(DenError::Authorization(format!(
                "Den tool context profile `{declared_profile}` does not match registered profile `{registered_profile}` for binding_id"
            )));
        }
    }
    Ok(registered_profile)
}

/// Verify membership, then resolve the caller's role.
pub async fn authorize_context(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
) -> Result<BearProfile, DenError> {
    if !dir
        .user_may_use_bear(context.user_id, context.bear_id)
        .await?
    {
        return Err(DenError::Authorization(
            "user is not a member of this bear".to_string(),
        ));
    }
    context_role(dir, context).await
}

/// The native runtime's verified origin selects the descriptor audience.
/// Ordinary turns additionally recheck canonical conversation/Work ownership
/// in the in-process invoker; the registry is not another authority source for
/// those turns. Internal Curate/Watch still require their explicit system
/// registration until their own origin-specific source checks replace it.
pub async fn authorize_context_for_origin(
    dir: &impl BearDirectory,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
) -> Result<BearProfile, DenError> {
    let projected = ToolAudience::from_origin(origin).compatibility_profile();
    if context.profile != Some(projected) || context.binding_id.trim().is_empty() {
        return Err(DenError::Authorization(
            "Den tool context does not match the verified execution origin".into(),
        ));
    }
    if !dir
        .user_may_use_bear(context.user_id, context.bear_id)
        .await?
    {
        return Err(DenError::Authorization(
            "user is not a member of this bear".into(),
        ));
    }
    if matches!(
        origin,
        TurnExecutionOrigin::InternalCuration | TurnExecutionOrigin::InboundObservation
    ) && context_role(dir, context).await? != projected
    {
        return Err(DenError::Authorization(
            "internal Den tool origin does not match registered system authority".into(),
        ));
    }
    Ok(projected)
}

pub fn authorize_tool_for_origin(
    tool_name: &str,
    origin: TurnExecutionOrigin,
) -> Result<(), DenError> {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == tool_name)
        .ok_or_else(|| DenError::NotFound(format!("unknown Den tool: {tool_name}")))?;
    if descriptor.allows_origin(origin) {
        Ok(())
    } else {
        Err(DenError::Authorization(format!(
            "Den tool `{tool_name}` is unavailable to this verified execution origin"
        )))
    }
}

/// Legacy no-hat dispatcher compatibility. Never derive a native turn's
/// authority from this profile projection.
pub fn authorize_tool_for_profile(tool_name: &str, role: BearProfile) -> Result<(), DenError> {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == tool_name)
        .ok_or_else(|| DenError::NotFound(format!("unknown Den tool: {tool_name}")))?;
    if descriptor.allows_profile(role) {
        Ok(())
    } else {
        Err(DenError::Authorization(format!(
            "Den tool `{tool_name}` is not available to the `{role}` role"
        )))
    }
}

#[cfg(test)]
#[path = "authority_tests.rs"]
mod authority_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tools::{
            arguments::DenToolChannelContext, capability_catalog::SessionCapabilityDescriptor,
            context::DenToolInvocationContext,
        },
        BearProfile,
    };
    use uuid::Uuid;

    fn context(session_capabilities: Vec<SessionCapabilityDescriptor>) -> DenToolInvocationContext {
        DenToolInvocationContext {
            bear_id: Uuid::nil(),
            bear_slug: "test".to_string(),
            binding_id: "test".to_string(),
            profile: Some(BearProfile::Pair),
            user_id: 1,
            username: None,
            membership_role: None,
            conversation_id: "conversation".to_string(),
            session_id: "session".to_string(),
            work_run_id: None,
            client_session_id: Some("client-1".to_string()),
            conversation_selection: None,
            runtime_target: None,
            workspace_roots: vec!["/workspace/repo".to_string()],
            session_capabilities,
            session_policy: None,
            activity: None,
            runtime: None,
            context_budget: None,
            projected_memory: None,
            recalled_memory: None,
            request_id: None,
            channel: DenToolChannelContext::default(),
        }
    }

    fn live_session_tool() -> SessionCapabilityDescriptor {
        SessionCapabilityDescriptor {
            instance_id: "client-1:mcp__filesystem__read".to_string(),
            name: "mcp__filesystem__read".to_string(),
            summary: "Read files through the connected MCP provider.".to_string(),
            kind: "tool".to_string(),
            provider: "mcp".to_string(),
            execution_locality: "connected MCP provider".to_string(),
            authority: "current client connection and turn policy".to_string(),
            surface: "workspace roots: /workspace/repo".to_string(),
            availability: "available".to_string(),
            tags: vec!["session-bound".to_string(), "mcp".to_string()],
        }
    }

    #[test]
    fn discovery_includes_available_session_instances_but_not_unavailable_ones() {
        let mut unavailable = live_session_tool();
        unavailable.instance_id = "client-1:mcp__stale".to_string();
        unavailable.availability = "unavailable".to_string();
        let context = context(vec![live_session_tool(), unavailable]);

        let result = capability_describe(
            json!({ "ref": "capability-instance:client-1:mcp__filesystem__read" }),
            BearProfile::Pair,
            &context,
        )
        .unwrap();
        assert_eq!(
            result["capability"]["ref"],
            "capability-instance:client-1:mcp__filesystem__read"
        );
        assert!(capability_describe(
            json!({ "ref": "capability-instance:client-1:mcp__stale" }),
            BearProfile::Pair,
            &context,
        )
        .is_err());
        assert_eq!(
            result["capability"]["descriptor_lifecycle"],
            "session_instance"
        );
        assert_eq!(
            result["capability"]["code_mode_compatibility"],
            "requires_explicit_local_provider_mediation"
        );
    }

    #[test]
    fn session_instances_do_not_change_durable_role_filtering() {
        let context = context(vec![live_session_tool()]);
        let chat_entries = capability_entries_for_context(BearProfile::Chat, &context);
        assert!(chat_entries
            .iter()
            .any(|entry| { entry.r#ref == "capability-instance:client-1:mcp__filesystem__read" }));
        assert!(!chat_entries
            .iter()
            .any(|entry| entry.r#ref == "tool:den.docket.create_job"));
    }
}
