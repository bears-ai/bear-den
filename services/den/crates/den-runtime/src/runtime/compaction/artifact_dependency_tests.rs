use super::*;
use den_memory::personal_facts::PersonalFactId;
use den_service::conversation::dependencies::ContextDependencies;
use serde_json::{json, Value};

const CANARY: &str = "private-summary-canary-39A2";

fn summary_json() -> Value {
    json!({
        "active_user_goals": ["Finish the migration"],
        "important_constraints": [],
        "decisions_made": ["Keep the existing API"],
        "artifact_refs": ["artifact://ordinary"],
        "workflow_state_refs": [],
        "unresolved_followups": [],
    })
}

#[test]
fn protected_summary_is_rejected_before_text_deserialization() {
    let mut value = summary_json();
    // Intentionally invalid summary shape: the dependency gate must win over
    // deserialization, which could otherwise render rejected private values.
    value["active_user_goals"] = json!({"private": CANARY});
    let fact_id = PersonalFactId(Uuid::new_v4());
    ContextDependencies::personal_fact(Uuid::new_v4().into(), fact_id)
        .attach_to_metadata(&mut value)
        .unwrap();
    let error = decode_unrestricted_iterative_summary(value).unwrap_err();
    assert!(matches!(error, DenError::Authorization(_)));
    for rendered in [format!("{error}"), format!("{error:?}")] {
        assert!(!rendered.contains(CANARY));
        assert!(!rendered.contains(&fact_id.0.to_string()));
    }
}

#[test]
fn malformed_summary_dependencies_cannot_be_discarded_as_unknown_serde_fields() {
    for envelope in [
        Value::Null,
        json!([1, []]),
        json!({"version": 2, "personal_facts": []}),
        json!({"version": 1, "personal_facts": [{"private": CANARY}]}),
    ] {
        let mut value = summary_json();
        value["den_context_dependencies"] = envelope;
        let error = decode_unrestricted_iterative_summary(value).unwrap_err();
        assert!(matches!(error, DenError::Authorization(_)));
        assert!(!format!("{error:?}").contains(CANARY));
    }
}

#[test]
fn legacy_and_explicit_empty_summary_dependencies_preserve_ordinary_text() {
    let value = summary_json();
    let ordinary = decode_unrestricted_iterative_summary(value.clone()).unwrap();
    let mut annotated = value;
    ContextDependencies::default()
        .attach_to_metadata(&mut annotated)
        .unwrap();
    let empty = decode_unrestricted_iterative_summary(annotated).unwrap();
    assert_eq!(to_value(&ordinary).unwrap(), to_value(&empty).unwrap());
    assert_eq!(ordinary.active_user_goals, ["Finish the migration"]);
    assert_eq!(ordinary.decisions_made, ["Keep the existing API"]);
}

#[test]
fn unprotected_invalid_summary_remains_a_schema_error() {
    assert!(matches!(
        decode_unrestricted_iterative_summary(json!({})),
        Err(DenError::Parsing(_))
    ));
}
