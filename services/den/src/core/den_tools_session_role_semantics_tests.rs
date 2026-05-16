use serde_json::json;

use crate::core::{
    bears::BearAgentRole,
    den_tools::{infer_work_surface_hint, DenToolInvocationContext},
};

fn context_for(role: BearAgentRole) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        role_agent_id: "agent".to_string(),
        agent_role: Some(role),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        acp_session_id: Some("acp-test".to_string()),
        conversation_selection: Some("src/main.rs".to_string()),
        runtime_target: Some("repo:builder-bear".to_string()),
        workspace_roots: vec!["/workspace".to_string()],
        session_policy: None,
        activity: None,
        request_id: None,
        channel: Default::default(),
    }
}

#[test]
fn infer_work_surface_hint_marks_pair_as_active_mode() {
    let payload = infer_work_surface_hint(&context_for(BearAgentRole::Pair), BearAgentRole::Pair);
    assert_eq!(payload["work_surface"]["mode"], json!("active"));
}

#[test]
fn infer_work_surface_hint_marks_work_as_active_mode() {
    let payload = infer_work_surface_hint(&context_for(BearAgentRole::Work), BearAgentRole::Work);
    assert_eq!(payload["work_surface"]["mode"], json!("active"));
}

#[test]
fn infer_work_surface_hint_marks_talk_as_reference_only_mode() {
    let payload = infer_work_surface_hint(&context_for(BearAgentRole::Talk), BearAgentRole::Talk);
    assert_eq!(payload["work_surface"]["mode"], json!("reference_only"));
    assert!(payload["work_surface"]["note"]
        .as_str()
        .unwrap()
        .contains("answer about relevant Bear work surfaces"));
}
