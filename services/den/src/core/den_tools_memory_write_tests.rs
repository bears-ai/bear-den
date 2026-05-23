use serde_json::json;

use crate::core::den_tools::merge_memory_entry_source_with_human;

#[test]
fn merge_memory_entry_source_prefers_authenticated_user_username() {
    let merged = merge_memory_entry_source_with_human(
        None,
        &crate::core::den_tools::DenToolInvocationContext {
            user_id: 7,
            username: Some("context-user".to_string()),
            membership_role: Some("admin".to_string()),
            bear_id: uuid::Uuid::nil(),
            role_agent_id: "agent-123".to_string(),
            agent_role: Some(crate::core::bears::model::BearAgentRole::Pair),
            conversation_id: Some("conv-1".to_string()),
            session_id: Some("session-1".to_string()),
            acp_session_id: Some("acp-1".to_string()),
            conversation_selection: Some("conv-1".to_string()),
            runtime_target: Some("conv-1".to_string()),
            request_id: Some("req-1".to_string()),
            workspace_root: None,
            work_surface_anchor: None,
        },
        Some(&crate::core::user::db::User {
            id: 7,
            email: "hans@example.test".to_string(),
            username: "gerwitz".to_string(),
            display_name: "Hans Gerwitz".to_string(),
            passhash: "hash".to_string(),
            is_admin: true,
            theme: "dark".to_string(),
        }),
    )
    .unwrap();
    assert_eq!(merged["human"]["username"], "gerwitz");
    assert_eq!(merged["human"]["display_name"], "Hans Gerwitz");
}

#[test]
fn merge_memory_entry_source_falls_back_to_context_username() {
    let merged = merge_memory_entry_source_with_human(
        Some(json!({"origin": "test"})),
        &crate::core::den_tools::DenToolInvocationContext {
            user_id: 7,
            username: Some("context-user".to_string()),
            membership_role: Some("admin".to_string()),
            bear_id: uuid::Uuid::nil(),
            role_agent_id: "agent-123".to_string(),
            agent_role: Some(crate::core::bears::model::BearAgentRole::Pair),
            conversation_id: Some("conv-1".to_string()),
            session_id: Some("session-1".to_string()),
            acp_session_id: Some("acp-1".to_string()),
            conversation_selection: Some("conv-1".to_string()),
            runtime_target: Some("conv-1".to_string()),
            request_id: Some("req-1".to_string()),
            workspace_root: None,
            work_surface_anchor: None,
        },
        None,
    )
    .unwrap();
    assert_eq!(merged["human"]["username"], "context-user");
    assert_eq!(merged["origin"], "test");
}
