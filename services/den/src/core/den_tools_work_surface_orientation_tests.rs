use serde_json::json;

use crate::core::{
    bears::BearAgentRole,
    den_tools::{
        build_work_surface_orientation_payload, collect_memory_tree_paths, infer_work_surface_hint,
        work_surface_anchor_paths, work_surface_candidate_slug, DenToolInvocationContext,
    },
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
        conversation_selection: Some("builder-bear/src/lib.rs".to_string()),
        runtime_target: Some("repo:builder-bear".to_string()),
        workspace_roots: vec!["/workspace/builder-bear".to_string()],
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        request_id: None,
        channel: Default::default(),
    }
}

#[test]
fn work_surface_candidate_slug_prefers_trusted_repo_like_hint() {
    assert_eq!(
        work_surface_candidate_slug(&context_for(BearAgentRole::Pair)).as_deref(),
        Some("builder-bear")
    );
}

#[test]
fn work_surface_anchor_paths_are_stable() {
    let (canonical, role_local) = work_surface_anchor_paths(BearAgentRole::Pair, "builder-bear");
    assert_eq!(canonical[0], "core/work_surfaces/builder-bear/index.md");
    assert_eq!(canonical[1], "core/work_surfaces/builder-bear/overview.md");
    assert_eq!(
        role_local[0],
        "pair/work_surfaces/builder-bear/current-understanding.md"
    );
}

#[test]
fn collect_memory_tree_paths_walks_nested_values() {
    let tree = json!({
        "path": "core/work_surfaces/builder-bear/index.md",
        "children": [
            {"path": "core/work_surfaces/builder-bear/overview.md"},
            {"nested": {"path": "pair/work_surfaces/builder-bear/current-understanding.md"}}
        ]
    });
    let mut paths = Vec::new();
    collect_memory_tree_paths(&tree, &mut paths);
    assert!(paths.contains(&"core/work_surfaces/builder-bear/index.md".to_string()));
    assert!(paths.contains(&"core/work_surfaces/builder-bear/overview.md".to_string()));
    assert!(paths.contains(&"pair/work_surfaces/builder-bear/current-understanding.md".to_string()));
}

#[test]
fn build_work_surface_orientation_payload_reports_existing_anchors() {
    let context = context_for(BearAgentRole::Pair);
    let hint_payload = infer_work_surface_hint(&context, BearAgentRole::Pair);
    let files = vec![
        "core/work_surfaces/builder-bear/index.md".to_string(),
        "core/work_surfaces/builder-bear/overview.md".to_string(),
        "pair/work_surfaces/builder-bear/current-understanding.md".to_string(),
    ];
    let payload = build_work_surface_orientation_payload(
        BearAgentRole::Pair,
        &hint_payload,
        &files,
        Some("builder-bear".to_string()),
    );
    assert_eq!(payload["work_surface"]["status"], json!("oriented"));
    assert_eq!(payload["work_surface"]["slug"], json!("builder-bear"));
    assert!(payload["canonical_paths"].as_array().unwrap().len() >= 2);
    assert!(payload["role_local_paths"].as_array().unwrap().len() >= 1);
    assert!(payload["recommended_read_order"].as_array().unwrap().len() >= 3);
}

#[test]
fn build_work_surface_orientation_payload_reports_unresolved_without_slug() {
    let context = context_for(BearAgentRole::Pair);
    let hint_payload = infer_work_surface_hint(&context, BearAgentRole::Pair);
    let payload =
        build_work_surface_orientation_payload(BearAgentRole::Pair, &hint_payload, &[], None);
    assert_eq!(payload["work_surface"]["status"], json!("unresolved"));
    assert_eq!(payload["canonical_paths"], json!([]));
}

#[test]
fn work_surface_anchor_paths_skip_role_local_paths_for_talk() {
    let (canonical, role_local) = work_surface_anchor_paths(BearAgentRole::Talk, "builder-bear");
    assert_eq!(canonical[0], "core/work_surfaces/builder-bear/index.md");
    assert!(role_local.is_empty());
}

#[test]
fn build_work_surface_orientation_payload_for_talk_is_reference_only() {
    let context = context_for(BearAgentRole::Talk);
    let hint_payload = infer_work_surface_hint(&context, BearAgentRole::Talk);
    let files = vec![
        "core/work_surfaces/builder-bear/index.md".to_string(),
        "core/work_surfaces/builder-bear/overview.md".to_string(),
        "talk/work_surfaces/builder-bear/current-understanding.md".to_string(),
    ];
    let payload = build_work_surface_orientation_payload(
        BearAgentRole::Talk,
        &hint_payload,
        &files,
        Some("builder-bear".to_string()),
    );
    assert_eq!(payload["work_surface"]["mode"], json!("reference_only"));
    assert_eq!(payload["role_local_paths"], json!([]));
    assert!(payload["notes"][1]
        .as_str()
        .unwrap()
        .contains("rather than claiming role-local ownership"));
}
