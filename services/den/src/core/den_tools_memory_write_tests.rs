use serde_json::json;
use uuid::Uuid;

use crate::core::{
    bears::model::BearAgentRole,
    den_tools::{
        merge_memory_entry_source_with_human, DenToolChannelContext, DenToolInvocationContext,
    },
    user,
};

fn sample_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: Uuid::nil(),
        bear_slug: "meta".to_string(),
        role_agent_id: "agent-123".to_string(),
        agent_role: Some(BearAgentRole::Pair),
        user_id: 7,
        username: Some("context-user".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-1".to_string(),
        session_id: "session-1".to_string(),
        acp_session_id: Some("acp-1".to_string()),
        conversation_selection: Some("conv-1".to_string()),
        runtime_target: Some("conv-1".to_string()),
        workspace_roots: vec![],
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        request_id: Some("req-1".to_string()),
        channel: DenToolChannelContext::default(),
    }
}

fn sample_user() -> user::User {
    user::User {
        id: 7,
        username: "gerwitz".to_string(),
        display_name: "Hans Gerwitz".to_string(),
        email: "hans@example.test".to_string(),
        email_verified: Some(true),
        theme: "dark".to_string(),
        week_start_day: 1,
        created: time::PrimitiveDateTime::MIN,
    }
}

#[test]
fn merge_memory_entry_source_prefers_authenticated_user_username() {
    let context = sample_context();
    let current_user = sample_user();

    let merged =
        merge_memory_entry_source_with_human(None, &context, Some(&current_user)).unwrap();

    assert_eq!(merged["human"]["user_id"], 7);
    assert_eq!(merged["human"]["username"], "gerwitz");
    assert_eq!(merged["human"]["display_name"], "Hans Gerwitz");
    assert_eq!(merged["human"]["membership_role"], "admin");
    assert_eq!(merged["session"]["conversation_id"], "conv-1");
    assert_eq!(merged["session"]["request_id"], "req-1");
}

#[test]
fn merge_memory_entry_source_falls_back_to_context_username() {
    let context = sample_context();

    let merged = merge_memory_entry_source_with_human(
        Some(json!({"origin": "test"})),
        &context,
        None,
    )
    .unwrap();

    assert_eq!(merged["origin"], "test");
    assert_eq!(merged["human"]["username"], "context-user");
    assert_eq!(merged["human"]["authenticated_by"], "acp_token");
}
