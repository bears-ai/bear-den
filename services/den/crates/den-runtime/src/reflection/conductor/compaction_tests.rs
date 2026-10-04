use super::*;
use crate::runtime_compaction::{compaction_policy_for_source, CompactionSource};
use serde_json::json;

fn queued_run(input_summary: serde_json::Value) -> ReflectionRunRow {
    ReflectionRunRow {
        id: Uuid::nil(),
        bear_id: Uuid::nil(),
        lane: "context_compact".into(),
        trigger: "post_turn".into(),
        status: "queued".into(),
        role_agent_id: None,
        conversation_id: Some("stored-conversation".into()),
        conversation_key: None,
        conversation_date: None,
        input_summary,
        output_summary: json!({}),
        error: None,
        started_at: None,
        completed_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn queued_audit_claims_are_not_worker_policy_inputs() {
    for profile in [
        json!("pair"),
        json!("work"),
        json!("curate"),
        json!("unknown"),
        json!({}),
        json!(null),
    ] {
        let run = queued_run(json!({
            "conversation_id": "queued-conversation",
            "profile": profile,
            "origin": "authorized_work_run",
            "source": "Turn(AuthorizedWorkRun(Connected))",
        }));
        // Parsing yields only the target conversation, not an origin or policy.
        assert_eq!(
            parse_context_compact_input(&run).unwrap(),
            "queued-conversation"
        );
        let policy = compaction_policy_for_source(CompactionSource::ContextMaintenance).unwrap();
        assert_eq!(policy.policy_version, "background-v1");
    }
}

#[test]
fn queued_compaction_conversation_fallback_and_missing_target_are_preserved() {
    let mut run = queued_run(json!({"profile": "work"}));
    assert_eq!(
        parse_context_compact_input(&run).unwrap(),
        "stored-conversation"
    );
    run.conversation_id = None;
    assert_eq!(
        parse_context_compact_input(&run).unwrap_err(),
        "context_compact missing conversation_id"
    );
}
