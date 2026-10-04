use super::merge_memory_entry_source_with_human;
use crate::tools::arguments::DenToolChannelContext;
use crate::tools::context::DenToolInvocationContext;
use crate::RuntimeContextLabel;
use serde_json::json;

fn sample_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "meta".to_string(),
        binding_id: "agent-123".to_string(),
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id: 7,
        username: Some("context-user".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-1".to_string(),
        session_id: "session-1".to_string(),
        work_run_id: None,
        client_session_id: Some("client-1".to_string()),
        conversation_selection: Some("conv-1".to_string()),
        runtime_target: Some("conv-1".to_string()),
        workspace_roots: vec![],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some("req-1".to_string()),
        channel: DenToolChannelContext::default(),
    }
}

#[test]
fn merge_memory_entry_source_prefers_authenticated_user_username() {
    let context = sample_context();

    let merged = merge_memory_entry_source_with_human(
        None,
        &context,
        Some("gerwitz".to_string()),
        Some("Hans Gerwitz".to_string()),
    )
    .unwrap();

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

    let merged =
        merge_memory_entry_source_with_human(Some(json!({"origin": "test"})), &context, None, None)
            .unwrap();

    assert_eq!(merged["origin"], "test");
    assert_eq!(merged["human"]["username"], "context-user");
    assert_eq!(merged["human"]["authenticated_by"], "client_token");
}
