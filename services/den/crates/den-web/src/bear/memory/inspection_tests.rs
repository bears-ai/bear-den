use super::{read_result, reflection_feedback, PartialList};

#[test]
fn partial_lists_keep_available_rows_and_never_claim_completeness() {
    let mut errors = Vec::new();
    let partial = PartialList::combine(
        Ok::<_, &str>(vec!["surviving row"]),
        Err("store failed"),
        ["First store", "Second store"],
        &mut errors,
    );
    assert_eq!(partial.items, ["surviving row"]);
    assert!(!partial.complete);
    assert_eq!(errors, ["Second store unavailable: store failed"]);
    let empty = PartialList::combine(
        Ok::<Vec<u8>, &str>(vec![]),
        Ok(vec![]),
        ["First store", "Second store"],
        &mut errors,
    );
    assert!(empty.complete && empty.items.is_empty());
}

#[tokio::test]
async fn failed_postgres_reflection_reads_are_errors_not_zero_statistics() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused/unused")
        .unwrap();
    pool.close().await;
    let bear_id = uuid::Uuid::new_v4();
    assert!(
        super::super::list_recent_reflection_runs(&pool, bear_id, 10, None, None, None)
            .await
            .is_err()
    );
    assert!(super::super::reflection_performance_slo(&pool, bear_id)
        .await
        .is_err());
}

#[test]
fn failed_inspection_reads_preserve_evidence_without_inventing_zero_counts() {
    let mut errors = Vec::new();
    let failed: Option<i64> = read_result(
        Err::<i64, _>("actual read failure"),
        "Review count",
        &mut errors,
    );
    assert!(failed.is_none());
    assert_eq!(errors, ["Review count unavailable: actual read failure"]);
    let empty = read_result(Ok::<_, &str>(0_i64), "Review count", &mut errors);
    assert_eq!(empty, Some(0));
    assert_eq!(errors.len(), 1);
}

#[test]
fn normal_states_are_not_failures_but_actual_skips_keep_their_reason() {
    for reason in [
        "below_compaction_threshold",
        "no_uncompacted_content",
        "live_reflection_disabled",
    ] {
        let feedback = reflection_feedback(Some("skipped"), Some(reason), None);
        assert!(!feedback.needs_attention);
        assert!(feedback.error.is_none());
        assert!(!feedback.status_explanation.is_empty());
    }
    let missing = reflection_feedback(Some("skipped"), Some("no_compaction_artifact"), None);
    assert!(missing.needs_attention);
    assert!(missing
        .status_explanation
        .contains("Retry manual reflection"));
    let unknown_skip = reflection_feedback(Some("skipped"), Some("recorded_custom_reason"), None);
    assert!(unknown_skip
        .status_explanation
        .contains("recorded_custom_reason"));
}

#[test]
fn failures_never_claim_success_and_preserve_actual_diagnostics() {
    for status in ["failed", "error"] {
        let feedback =
            reflection_feedback(Some(status), None, Some("provider rejected the request"));
        assert!(feedback.needs_attention);
        assert_eq!(
            feedback.error.as_deref(),
            Some("provider rejected the request")
        );
        assert_eq!(feedback.status_label, "Failed");
        assert!(!feedback.status_explanation.contains("was inspected"));
    }
    let missing = reflection_feedback(Some("failed"), None, None);
    assert!(missing
        .status_explanation
        .contains("inspect the recorded payload"));
    let unknown = reflection_feedback(None, None, None);
    assert_eq!(unknown.status_label, "Unknown");
    assert!(unknown.needs_attention);
}
