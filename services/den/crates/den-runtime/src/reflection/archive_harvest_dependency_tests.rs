use super::*;
use den_memory::personal_facts::PersonalFactId;
use den_service::conversation::dependencies::ContextDependencies;
use serde_json::{json, Value};

fn artifact(artifact_json: Value) -> CompactionArtifactHarvestRow {
    CompactionArtifactHarvestRow {
        id: Uuid::new_v4(),
        conversation_id: Uuid::new_v4(),
        external_conversation_id: Some("ordinary-source".into()),
        artifact_kind: "iterative_summary".into(),
        policy_version: "test".into(),
        trigger: "manual".into(),
        source_message_start_seq: 1,
        source_message_end_seq: 2,
        source_group_start: Some(0),
        source_group_end: Some(1),
        artifact_json,
    }
}

#[test]
fn entire_harvest_batch_is_admitted_before_hashing_marking_or_proposals() {
    let mut protected = json!({"private": "harvest-canary-219F"});
    ContextDependencies::personal_fact(Uuid::new_v4().into(), PersonalFactId(Uuid::new_v4()))
        .attach_to_metadata(&mut protected)
        .unwrap();
    let artifacts = [artifact(json!({})), artifact(protected)];
    let mut effects_reached = false;
    let result = require_unrestricted_artifacts(&artifacts).map(|()| {
        effects_reached = true;
        artifacts
            .iter()
            .map(|row| source_hash(&row.artifact_json))
            .collect::<Vec<_>>()
    });
    assert!(matches!(result, Err(DenError::Authorization(_))));
    assert!(!effects_reached);
}

#[test]
fn harvest_decoder_and_batch_reject_malformed_and_protected_metadata() {
    for envelope in [
        Value::Null,
        json!([1, []]),
        json!({"version": 2, "personal_facts": []}),
        json!({"version": 1, "personal_facts": [{
            "bear_id": Uuid::new_v4(), "fact_id": Uuid::new_v4(),
        }]}),
    ] {
        let value = json!({
            "den_context_dependencies": envelope,
            "private": "harvest-canary-219F",
        });
        let artifacts = [artifact(json!({})), artifact(value.clone())];
        let errors = [
            require_unrestricted_artifacts(&artifacts).unwrap_err(),
            decode_summary(value).unwrap_err(),
        ];
        for error in errors {
            assert!(matches!(error, DenError::Authorization(_)));
            assert!(!format!("{error:?}").contains("harvest-canary-219F"));
        }
    }
}

#[test]
fn ordinary_and_explicit_empty_artifacts_remain_harvest_eligible() {
    let mut annotated = json!({"ordinary": "unchanged"});
    ContextDependencies::default()
        .attach_to_metadata(&mut annotated)
        .unwrap();
    require_unrestricted_artifacts(&[artifact(json!({})), artifact(annotated)]).unwrap();
}
