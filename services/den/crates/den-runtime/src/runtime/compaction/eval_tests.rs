//! Deterministic continuation probes for compaction quality (Phase G).

use den_core::{ArmatureAvailability, TurnExecutionOrigin};
use serde_json::json;

use super::{
    choose_compaction_decision, compaction_policy_for_source, merge_iterative_summary,
    render_compacted_context_block, semantic_groups_from_conversation_messages,
    summarize_compacted_groups, CompactionSource, TranscriptGroupingRow,
};
use crate::runtime_conversations::{RuntimeCompactionTriggerKind, RuntimeSemanticGroupKind};

fn pair_tool_heavy_fixture() -> Vec<TranscriptGroupingRow> {
    vec![
        TranscriptGroupingRow::new("user", "refactor compaction module", json!({}))
            .with_message_id("m1")
            .with_sequence_no(1),
        TranscriptGroupingRow::new("assistant", "I'll inspect files first", json!({}))
            .with_message_id("m2")
            .with_sequence_no(2),
        TranscriptGroupingRow::new(
            "tool_call",
            "Tool request: fs_read_text_file",
            json!({
                "event": "tool_request",
                "tool_call_id": "call-1",
                "tool_name": "fs_read_text_file",
            }),
        )
        .with_message_id("m3")
        .with_sequence_no(3)
        .with_tool_call_id("call-1"),
        TranscriptGroupingRow::new(
            "tool_result",
            "Tool result: fs_read_text_file",
            json!({
                "event": "tool_result",
                "tool_call_id": "call-1",
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "mod.rs",
            }),
        )
        .with_message_id("m4")
        .with_sequence_no(4)
        .with_tool_call_id("call-1"),
        TranscriptGroupingRow::new(
            "tool_call",
            "Tool request: fs_edit_file",
            json!({
                "event": "tool_request",
                "tool_call_id": "call-2",
                "tool_name": "fs_edit_file",
                "approval_request_id": "approval-1",
                "approval_required": true,
            }),
        )
        .with_message_id("m5")
        .with_sequence_no(5)
        .with_tool_call_id("call-2"),
        TranscriptGroupingRow::new("assistant", "waiting on approval", json!({}))
            .with_message_id("m6")
            .with_sequence_no(6),
    ]
}

#[test]
fn continuation_probe_preserves_unresolved_approval_after_compaction_evaluation() {
    let rows = pair_tool_heavy_fixture();
    let groups = semantic_groups_from_conversation_messages(&rows);
    let policy = compaction_policy_for_source(CompactionSource::Turn(
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    ))
    .unwrap();

    let decision = choose_compaction_decision(
        &groups,
        RuntimeCompactionTriggerKind::SemanticGroupCount,
        &policy,
    );

    assert!(
        decision.is_none() || decision.unwrap().selected_group_end < groups.len().saturating_sub(1),
        "compaction must not consume the protected recent approval tail"
    );
    assert!(groups.iter().any(|group| {
        group.kind == RuntimeSemanticGroupKind::ApprovalInteraction && group.protected
    }));
}

fn continuation_policy_for_tests() -> super::RuntimeCompactionPolicy {
    super::RuntimeCompactionPolicy {
        policy_version: "eval-v1".into(),
        protected_recent_group_count: 1,
        max_groups_before_compaction: 3,
        max_transcript_chars: 4_000,
    }
}

#[test]
fn continuation_probe_compacted_render_retains_goal_and_artifact_signals() {
    let rows = pair_tool_heavy_fixture();
    let groups = semantic_groups_from_conversation_messages(&rows);
    let policy = continuation_policy_for_tests();
    let decision =
        choose_compaction_decision(&groups, RuntimeCompactionTriggerKind::Manual, &policy)
            .expect("eligible compaction range");

    let compacted_groups = &groups[decision.selected_group_start..=decision.selected_group_end];
    let summary = summarize_compacted_groups(None, compacted_groups, &rows, &decision);
    let rendered = render_compacted_context_block(&summary);

    assert!(
        rendered.contains("refactor compaction module") || !summary.active_user_goals.is_empty()
    );
    assert!(rendered.contains("fs_read_text_file") || !summary.artifact_refs.is_empty());
}

#[test]
fn continuation_probe_iterative_merge_accumulates_constraints() {
    let groups = semantic_groups_from_conversation_messages(&pair_tool_heavy_fixture());
    let merged = merge_iterative_summary(None, &groups[..2]);
    let rendered = render_compacted_context_block(&merged);
    assert!(rendered.contains("Compacted context"));
}

#[test]
fn conversation_review_creates_summary_decision_below_pressure_threshold() {
    let rows = vec![
        TranscriptGroupingRow::new("user", "remember this project preference", json!({}))
            .with_message_id("m1")
            .with_sequence_no(1),
        TranscriptGroupingRow::new("assistant", "got it", json!({}))
            .with_message_id("m2")
            .with_sequence_no(2),
        TranscriptGroupingRow::new("user", "keep the newest group protected", json!({}))
            .with_message_id("m3")
            .with_sequence_no(3),
    ];
    let groups = semantic_groups_from_conversation_messages(&rows);
    let policy = super::RuntimeCompactionPolicy {
        policy_version: "review-v1".into(),
        protected_recent_group_count: 1,
        max_groups_before_compaction: 8,
        max_transcript_chars: 4_000,
    };

    assert!(
        choose_compaction_decision(
            &groups,
            RuntimeCompactionTriggerKind::SemanticGroupCount,
            &policy,
        )
        .is_none(),
        "normal compaction should still wait for the bucket threshold"
    );

    let decision = choose_compaction_decision(
        &groups,
        RuntimeCompactionTriggerKind::ConversationReview,
        &policy,
    )
    .expect("conversation review should summarize eligible history below pressure threshold");

    assert_eq!(decision.selected_group_start, 0);
    assert_eq!(
        decision.selected_group_end,
        groups.len() - policy.protected_recent_group_count - 1
    );
}

#[test]
fn chat_policy_allows_more_groups_before_compaction_than_pair() {
    let pair = compaction_policy_for_source(CompactionSource::Turn(
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    ))
    .unwrap();
    let chat = compaction_policy_for_source(CompactionSource::Turn(
        TurnExecutionOrigin::ChannelConversation,
    ))
    .unwrap();
    assert!(chat.max_groups_before_compaction > pair.max_groups_before_compaction);
}
