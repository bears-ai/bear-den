use serde_json::json;

use crate::core::tools::{
    session::DenToolInvocationContext, work_surface::infer_work_surface_hint,
};
use den_service::bears::RuntimeContextLabel;

fn context_for(role: RuntimeContextLabel) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(role),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("client-test".to_string()),
        conversation_selection: Some("src/main.rs".to_string()),
        runtime_target: Some("repo:builder-bear".to_string()),
        workspace_roots: vec!["/workspace".to_string()],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    }
}

#[test]
fn infer_work_surface_hint_marks_pair_as_active_mode() {
    let payload = infer_work_surface_hint(
        &context_for(RuntimeContextLabel::ArmatureConversation),
        RuntimeContextLabel::ArmatureConversation,
    );
    assert_eq!(payload["work_surface"]["mode"], json!("active"));
}

#[test]
fn infer_work_surface_hint_marks_work_as_active_mode() {
    let payload = infer_work_surface_hint(
        &context_for(RuntimeContextLabel::JobRun),
        RuntimeContextLabel::JobRun,
    );
    assert_eq!(payload["work_surface"]["mode"], json!("active"));
}

#[test]
fn infer_work_surface_hint_marks_chat_as_reference_only_mode() {
    let payload = infer_work_surface_hint(
        &context_for(RuntimeContextLabel::ChannelConversation),
        RuntimeContextLabel::ChannelConversation,
    );
    assert_eq!(payload["work_surface"]["mode"], json!("reference_only"));
    assert!(payload["work_surface"]["note"]
        .as_str()
        .unwrap()
        .contains("answer about relevant Bear work surfaces"));
}
