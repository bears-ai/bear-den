use super::*;

#[test]
fn later_stage_failure_retains_known_checkpoint_and_proposals_without_success_copy() {
    let proposal = Uuid::new_v4();
    let mut result = ManualReflectionResult {
        compaction_applied: true,
        compaction_status: "Applied".into(),
        candidate_count: 3,
        proposals_created: 1,
        proposal_ids: vec![proposal],
        proposals_complete: true,
        ..Default::default()
    };
    fail(
        &mut result,
        Stage::EventPersistence,
        "private-provider-diagnostic",
    );
    let copy = summary(&result);
    assert!(copy.contains("Checkpoint created"));
    assert!(copy.contains("1 proposal(s) created"));
    assert!(copy.contains("Reflection event recording failed"));
    assert!(!copy.contains("processed"));
    assert!(!copy.contains("private-provider-diagnostic"));
    assert_eq!(result.proposal_ids, vec![proposal]);
    assert!(result.needs_attention);
}

#[test]
fn extraction_error_does_not_report_unknown_partial_proposals_as_zero() {
    let mut result = ManualReflectionResult {
        compaction_applied: true,
        compaction_status: "Applied".into(),
        ..Default::default()
    };
    fail(
        &mut result,
        Stage::Extraction,
        "proposal persistence interrupted",
    );
    let copy = summary(&result);
    assert!(copy.contains("Checkpoint created"));
    assert!(copy.contains("proposal creation did not finish"));
    assert!(copy.contains("inspect Memory"));
    assert!(copy.contains("Memory extraction failed"));
    assert!(!copy.contains("0 proposal"));
    assert!(!copy.contains("processed"));
}
