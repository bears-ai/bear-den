use serde_json::json;

use crate::core::{
    bears::BearAgentRole,
    den_tools::{infer_work_surface_hint, DenToolInvocationContext},
};

fn pair_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        role_agent_id: "agent".to_string(),
        agent_role: Some(BearAgentRole::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        acp_session_id: Some("acp-test".to_string()),
        conversation_selection: Some("src/main.rs".to_string()),
        runtime_target: Some("repo:builder-bear".to_string()),
        workspace_roots: vec!["/workspace".to_string()],
        request_id: None,
        channel: Default::default(),
    }
}

#[test]
fn infer_work_surface_hint_surfaces_trusted_candidates() {
    let payload = infer_work_surface_hint(&pair_context(), BearAgentRole::Pair);
    assert_eq!(payload["workplace"]["role"], json!("pair"));
    assert_eq!(payload["workplace"]["memory_surface"], json!("pair/"));
    assert_eq!(payload["work_surface"]["status"], json!("candidate"));
    let candidates = payload["work_surface"]["reference_candidates"]
        .as_array()
        .expect("reference candidates array");
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("runtime_target")));
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("conversation_selection")));
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("workspace_root")));
}

#[test]
fn infer_work_surface_hint_reports_unresolved_without_trusted_candidates() {
    let mut context = pair_context();
    context.runtime_target = None;
    context.conversation_selection = None;
    context.workspace_roots.clear();

    let payload = infer_work_surface_hint(&context, BearAgentRole::Pair);
    assert_eq!(payload["work_surface"]["status"], json!("unresolved"));
    assert_eq!(payload["work_surface"]["reference_candidates"], json!([]));
}
