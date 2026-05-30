use serde_json::json;

use super::runtime_compaction::{
    build_runtime_context_envelope, merge_iterative_summary,
    semantic_groups_from_runtime_messages, RuntimeContextEnvelopeInput,
};
use super::runtime_conversations::RuntimeSemanticGroupKind;

fn assert_contains(values: &[String], needle: &str) {
    assert!(
        values.iter().any(|value| value.contains(needle)),
        "expected to find {needle:?} in {values:?}"
    );
}

#[test]
fn pair_compaction_preserves_workflow_artifact_and_protected_approval_signals() {
    let messages = vec![
        json!({"role": "user", "content": "Patch /workspace/foo.rs but avoid broad refactors."}),
        json!({"role": "assistant", "content": "I will inspect foo.rs and keep the patch narrow."}),
        json!({"role": "tool", "tool_call_id": "call-read", "tool_name": "fs_read_text_file", "content": "read foo.rs"}),
        json!({"role": "assistant", "content": "artifact saved to file:///workspace/foo.rs"}),
        json!({"role": "assistant", "approval_request_id": "approval-1", "content": "approval required before editing"}),
        json!({"role": "system", "content": "workflow_state: awaiting_approval"}),
        json!({"role": "system", "content": "workflow_state: approved_for_editing"}),
    ];

    let groups = semantic_groups_from_runtime_messages(&messages);
    assert_eq!(groups[0].kind, RuntimeSemanticGroupKind::UserTurn);
    assert_eq!(groups[2].kind, RuntimeSemanticGroupKind::ToolInteraction);
    assert!(groups[2].protected);
    assert_eq!(groups[4].kind, RuntimeSemanticGroupKind::ApprovalInteraction);
    assert!(groups[4].protected);
    assert_eq!(groups[5].kind, RuntimeSemanticGroupKind::ApprovalInteraction);
    assert!(groups[5].protected);

    let summary = merge_iterative_summary(None, &groups);
    assert_contains(&summary.active_user_goals, "UserTurn");
    assert_contains(&summary.artifact_refs, "ArtifactUpdate");
    assert_contains(&summary.workflow_state_refs, "WorkflowUpdate");
    assert_contains(&summary.decisions_made, "ApprovalInteraction");
    assert_contains(&summary.important_constraints, "protected:ApprovalInteraction");
    assert_contains(&summary.important_constraints, "protected:WorkflowUpdate");

    let envelope = build_runtime_context_envelope(RuntimeContextEnvelopeInput {
        active_instructions: vec!["pair system".into()],
        workflow_state: summary.workflow_state_refs.clone(),
        recent_groups: groups[4..].to_vec(),
        compacted_summary: Some(summary.clone()),
    });

    assert_eq!(envelope.recent_groups.len(), 3);
    assert!(envelope.compacted_context.is_some());
    let compacted = envelope.compacted_context.unwrap();
    assert_contains(&compacted.active_user_goals, "UserTurn");
    assert_contains(&compacted.artifact_refs, "ArtifactUpdate");
    assert_contains(&compacted.workflow_state_refs, "WorkflowUpdate");
}

#[test]
fn chat_compaction_preserves_goal_constraint_and_followup_signals() {
    let messages = vec![
        json!({"role": "user", "content": "Help me plan a launch announcement and keep it concise."}),
        json!({"role": "assistant", "content": "I will keep it concise and produce a draft outline."}),
        json!({"role": "assistant", "content": "artifact saved to file:///workspace/launch-outline.md"}),
        json!({"role": "assistant", "content": "Next we should turn the outline into a first draft."}),
    ];

    let groups = semantic_groups_from_runtime_messages(&messages);
    let summary = merge_iterative_summary(None, &groups);

    assert_contains(&summary.active_user_goals, "UserTurn");
    assert_contains(&summary.artifact_refs, "ArtifactUpdate");
    assert_contains(&summary.unresolved_followups, "AssistantReply");

    let envelope = build_runtime_context_envelope(RuntimeContextEnvelopeInput {
        active_instructions: vec!["chat system".into()],
        workflow_state: vec![],
        recent_groups: vec![groups.last().cloned().unwrap()],
        compacted_summary: Some(summary.clone()),
    });

    assert_eq!(envelope.instructions, vec!["chat system"]);
    assert_eq!(envelope.recent_groups.len(), 1);
    let compacted = envelope.compacted_context.unwrap();
    assert_contains(&compacted.active_user_goals, "UserTurn");
    assert_contains(&compacted.artifact_refs, "ArtifactUpdate");
}
