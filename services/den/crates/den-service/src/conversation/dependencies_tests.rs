use super::*;
use serde_json::json;

fn dependency(bear: u128, fact: u128) -> ContextDependencies {
    ContextDependencies::personal_fact(
        BearId::new(Uuid::from_u128(bear)),
        PersonalFactId(Uuid::from_u128(fact)),
    )
}

#[test]
fn legacy_absence_and_declared_empty_dependencies_allow_unrestricted_reuse() {
    for metadata in [
        Value::Null,
        json!({}),
        json!({"event": "assistant_output", "unrelated": [1, 2]}),
        json!({"nested": {"den_context_dependencies": null}}),
        json!({"den_context_dependencies": {"version": 1, "personal_facts": []}}),
    ] {
        let dependencies = ContextDependencies::from_metadata(&metadata).unwrap();
        assert_eq!(dependencies, ContextDependencies::default());
        assert!(dependencies.is_empty());
        dependencies.require_unrestricted_reuse().unwrap();
        require_unrestricted_metadata(&metadata).unwrap();
    }
}

#[test]
fn union_is_deduplicated_order_independent_and_bear_qualified() {
    let first = dependency(1, 2);
    let second = dependency(3, 2);
    let mut forward = first.clone();
    forward.union_from(&first);
    forward.union_from(&second);
    forward.union_from(&ContextDependencies::default());
    let mut reverse = second;
    reverse.union_from(&first);
    assert_eq!(forward, reverse);
    assert_ne!(forward, first);
    assert!(!forward.is_empty());
    assert!(forward.require_unrestricted_reuse().is_err());

    let mut metadata = json!({});
    forward.attach_to_metadata(&mut metadata).unwrap();
    assert_eq!(
        metadata[METADATA_FIELD]["personal_facts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ContextDependencies::from_metadata(&metadata).unwrap(),
        forward
    );
    assert!(require_unrestricted_metadata(&metadata).is_err());
}

#[test]
fn wire_round_trip_contains_only_version_and_qualified_fact_locators() {
    let dependencies = dependency(1, 2);
    let mut metadata = Value::Null;
    dependencies.attach_to_metadata(&mut metadata).unwrap();
    assert_eq!(
        metadata,
        json!({
            "den_context_dependencies": {
                "version": 1,
                "personal_facts": [{
                    "bear_id": Uuid::from_u128(1),
                    "fact_id": Uuid::from_u128(2),
                }],
            },
        })
    );
    assert_eq!(
        ContextDependencies::from_metadata(&metadata).unwrap(),
        dependencies
    );
}

#[test]
fn duplicate_wire_entries_decode_to_one_dependency() {
    let fact = json!({"bear_id": Uuid::from_u128(1), "fact_id": Uuid::from_u128(2)});
    let metadata = json!({
        "den_context_dependencies": {"version": 1, "personal_facts": [fact.clone(), fact]},
    });
    assert_eq!(
        ContextDependencies::from_metadata(&metadata).unwrap(),
        dependency(1, 2)
    );
}

#[test]
fn attachment_unions_existing_protection_and_preserves_unrelated_metadata() {
    let unrelated = json!({
        "event": "tool_result",
        "scope_id": "scope",
        "request_id": "request",
        "tool_call_id": "call",
        "provider_message_id": "provider",
        "structured_content": {"content": "untouched"},
    });
    let mut metadata = unrelated.clone();
    dependency(1, 2).attach_to_metadata(&mut metadata).unwrap();
    dependency(3, 4).attach_to_metadata(&mut metadata).unwrap();
    let protected = metadata.clone();
    ContextDependencies::default()
        .attach_to_metadata(&mut metadata)
        .unwrap();
    assert_eq!(metadata, protected);

    let mut expected = dependency(1, 2);
    expected.union_from(&dependency(3, 4));
    assert_eq!(
        ContextDependencies::from_metadata(&metadata).unwrap(),
        expected
    );
    metadata.as_object_mut().unwrap().remove(METADATA_FIELD);
    assert_eq!(metadata, unrelated);
}

#[test]
fn non_object_metadata_is_rejected_except_for_legacy_null() {
    for metadata in [
        json!([]),
        json!([{}]),
        json!(true),
        json!(1),
        json!("opaque"),
    ] {
        assert!(ContextDependencies::from_metadata(&metadata).is_err());
        assert!(require_unrestricted_metadata(&metadata).is_err());
        let mut destination = metadata.clone();
        assert!(ContextDependencies::default()
            .attach_to_metadata(&mut destination)
            .is_err());
        assert_eq!(destination, metadata);
    }
    let mut metadata = Value::Null;
    ContextDependencies::default()
        .attach_to_metadata(&mut metadata)
        .unwrap();
    assert!(metadata.is_object());
    require_unrestricted_metadata(&metadata).unwrap();
}

#[test]
fn present_envelopes_and_fact_entries_must_be_maps_with_exact_v1_fields() {
    let bear = Uuid::from_u128(1);
    let fact = Uuid::from_u128(2);
    for envelope in [
        Value::Null,
        json!(false),
        json!("opaque"),
        json!([]),
        json!([1, []]),
        json!({}),
        json!({"version": 1}),
        json!({"personal_facts": []}),
        json!({"version": 1, "personal_facts": null}),
        json!({"version": 1, "personal_facts": {}}),
        json!({"version": 1, "personal_facts": [], "name": "private"}),
        json!({"version": 1, "personal_facts": [[bear, fact]]}),
        json!({"version": 1, "personal_facts": [null]}),
        json!({"version": 1, "personal_facts": [{"bear_id": bear}]}),
        json!({"version": 1, "personal_facts": [{"fact_id": fact}]}),
        json!({"version": 1, "personal_facts": [{"bear_id": "invalid", "fact_id": fact}]}),
        json!({"version": 1, "personal_facts": [{"bear_id": bear, "fact_id": 2}]}),
        json!({"version": 1, "personal_facts": [{"bear_id": bear, "fact_id": fact, "grant": true}]}),
    ] {
        let metadata = json!({"den_context_dependencies": envelope, "unrelated": "preserved"});
        assert!(ContextDependencies::from_metadata(&metadata).is_err());
        assert!(require_unrestricted_metadata(&metadata).is_err());
        for dependencies in [ContextDependencies::default(), dependency(1, 2)] {
            let mut destination = metadata.clone();
            assert!(dependencies.attach_to_metadata(&mut destination).is_err());
            assert_eq!(destination, metadata);
        }
    }
}

#[test]
fn unknown_null_and_non_integer_versions_fail_closed() {
    for version in [
        json!(0),
        json!(2),
        json!(256),
        json!(-1),
        json!(1.0),
        json!("1"),
        Value::Null,
    ] {
        let metadata = json!({
            "den_context_dependencies": {"version": version, "personal_facts": []},
        });
        assert!(ContextDependencies::from_metadata(&metadata).is_err());
        assert!(require_unrestricted_metadata(&metadata).is_err());
    }
}

#[test]
fn debug_and_errors_do_not_render_identifiers_or_rejected_payloads() {
    let dependencies = dependency(1, 2);
    assert_eq!(
        format!("{dependencies:?}"),
        "ContextDependencies([redacted])"
    );
    assert_eq!(
        format!("{:?}", ContextDependencies::default()),
        "ContextDependencies([redacted])"
    );
    let metadata = json!({
        "den_context_dependencies": {
            "version": 1,
            "personal_facts": [{"bear_id": "PrivateNameSentinel-8B21", "fact_id": Uuid::from_u128(2)}],
        },
    });
    for error in [
        dependencies.require_unrestricted_reuse().unwrap_err(),
        ContextDependencies::from_metadata(&metadata).unwrap_err(),
    ] {
        assert!(matches!(&error, DenError::Authorization(_)));
        for rendered in [format!("{error}"), format!("{error:?}")] {
            assert!(!rendered.contains("PrivateNameSentinel-8B21"));
            assert!(!rendered.contains(&Uuid::from_u128(1).to_string()));
            assert!(!rendered.contains(&Uuid::from_u128(2).to_string()));
        }
    }
}
