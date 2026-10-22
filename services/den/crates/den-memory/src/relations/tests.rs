use super::*;
use crate::{
    append_memory_record,
    resolver::{resolve, Assertion, Resolution, Signal},
    test_support::new_test_store,
    LogicalMemoryPath,
};
use serde_json::json;

async fn resolved_person(store: &BearMemoryStore, name: &str, email: &str) -> String {
    match resolve(
        store,
        "person",
        Some(name),
        &[Signal::new("email", email)],
        Assertion::Inferred,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected Resolved, got {other:?}"),
    }
}

async fn source_record(store: &BearMemoryStore, label: &str) -> String {
    append_memory_record(
        store,
        &LogicalMemoryPath::profile_local("pair", label),
        "note",
        "pair",
        None,
        label,
        &json!({}),
    )
    .await
    .unwrap()
    .memory_id
}

#[tokio::test]
async fn descriptive_and_access_route_to_distinct_tables() {
    let store = new_test_store().await;
    let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;
    let restricted = source_record(&store, "restricted").await;
    let open = source_record(&store, "open").await;
    for source in [&restricted, &open] {
        let descriptive = append_relation(
            &store,
            source,
            &person,
            "subject",
            &json!({ "is_primary": true }),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(descriptive.class, RelationClass::Descriptive);
    }
    assert_eq!(
        list_relations_for_source(&store, &restricted, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let access = append_relation(
        &store,
        &restricted,
        &person,
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(access.class, RelationClass::AccessBearing);
    let gate = list_access_rules_for_source(&store, &restricted)
        .await
        .unwrap();
    assert_eq!(gate.len(), 1);
    assert_eq!(gate[0].relation, "den.memory.relation.audience");

    let bear_id = store.bear_id().to_string();
    for withdrawn in [false, true] {
        if withdrawn {
            sqlx::query!(
                "UPDATE memory_access_rules SET state = 'retracted' WHERE bear_id = ? AND link_id = ?",
                bear_id,
                access.link_id,
            )
            .execute(store.pool())
            .await
            .unwrap();
        }
        assert!(has_access_rule_history_for_source(&store, &restricted)
            .await
            .unwrap());
        assert_eq!(
            list_access_rules_for_source(&store, &restricted)
                .await
                .unwrap()
                .is_empty(),
            withdrawn
        );
        assert!(list_relations_for_source(&store, &restricted, 10)
            .await
            .unwrap()
            .is_empty());
        let links = list_relations_for_entity(&store, &person, 1).await.unwrap();
        assert_eq!(links.len(), 1, "SQL filtering must precede LIMIT");
        assert_eq!(links[0].src_memory_id, open);
        assert!(
            descriptive_entity_ids_for_records(&store, std::slice::from_ref(&restricted))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            record_ids_for_entities(&store, std::slice::from_ref(&person))
                .await
                .unwrap(),
            vec![open.clone()]
        );
        let map = descriptive_entity_ids_by_source(&store).await.unwrap();
        assert!(!map.contains_key(&restricted));
        assert_eq!(map.get(&open), Some(&vec![person.clone()]));
        assert!(
            bounded_graph_expand(&store, std::slice::from_ref(&restricted), 2, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            bounded_graph_expand(&store, std::slice::from_ref(&open), 2, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn access_bearing_rejects_unresolved_target() {
    let store = new_test_store().await;
    let source = source_record(&store, "provisional").await;
    let provisional = match resolve(
        &store,
        "person",
        Some("Maybe-Ryan"),
        &[Signal::new("mention", "Ryan")],
        Assertion::Inferred,
    )
    .await
    .unwrap()
    {
        Resolution::Created(e) => e.entity_id,
        other => panic!("expected Created provisional, got {other:?}"),
    };
    let err = append_relation(
        &store,
        &source,
        &provisional,
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DenError::ValidationError(_)), "got {err:?}");
    append_relation(
        &store,
        &source,
        &provisional,
        "subject",
        &json!({}),
        "pair",
        None,
        None,
    )
    .await
    .expect("descriptive relation on provisional entity allowed");
}

#[tokio::test]
async fn disallowed_qualifier_and_unknown_relation_rejected() {
    let store = new_test_store().await;
    let source = source_record(&store, "invalid").await;
    let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;
    for (relation, qualifiers) in [
        ("subject", json!({ "nonsense": 1 })),
        ("definitely_not_a_relation", json!({})),
    ] {
        let err = append_relation(
            &store,
            &source,
            &person,
            relation,
            &qualifiers,
            "pair",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DenError::ValidationError(_)));
    }
}

#[tokio::test]
async fn descriptive_entity_ids_by_source_excludes_access_bearing() {
    let store = new_test_store().await;
    let source = source_record(&store, "descriptive").await;
    let other_source = source_record(&store, "access").await;
    let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;
    let other = resolved_person(&store, "Dana", "dana@acme.com").await;
    for (source, target, relation) in [
        (&source, &person, "subject"),
        (&source, &other, "participant"),
        (&other_source, &person, "audience"),
    ] {
        append_relation(
            &store,
            source,
            target,
            relation,
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
    }
    let map = descriptive_entity_ids_by_source(&store).await.unwrap();
    let mut ids = map.get(&source).cloned().unwrap_or_default();
    ids.sort();
    let mut expected = vec![person, other];
    expected.sort();
    assert_eq!(ids, expected, "descriptive entities only");
    assert!(!map.contains_key(&other_source));
}

#[tokio::test]
async fn bounded_graph_expand_reaches_indirect_records_depth_capped() {
    let store = new_test_store().await;
    let e1 = resolved_person(&store, "Alice", "alice@acme.com").await;
    let e2 = resolved_person(&store, "Bob", "bob@acme.com").await;
    let a = source_record(&store, "A").await;
    let b = source_record(&store, "B").await;
    let c = source_record(&store, "C").await;
    // A—e1—B (one hop), B—e2—C (two hops from A).
    for (source, target, relation) in [
        (&a, &e1, "subject"),
        (&b, &e1, "participant"),
        (&b, &e2, "subject"),
        (&c, &e2, "participant"),
    ] {
        append_relation(
            &store,
            source,
            target,
            relation,
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
    }
    let reached = bounded_graph_expand(&store, std::slice::from_ref(&a), 2, 10)
        .await
        .unwrap();
    let by_id: HashMap<&str, u32> = reached
        .iter()
        .map(|r| (r.memory_id.as_str(), r.hop))
        .collect();
    assert_eq!(by_id.get(b.as_str()), Some(&1));
    assert_eq!(by_id.get(c.as_str()), Some(&2));
    assert!(!by_id.contains_key(a.as_str()), "seed is excluded");
    let shallow = bounded_graph_expand(&store, &[a], 1, 10).await.unwrap();
    assert_eq!(
        shallow,
        vec![GraphReach {
            memory_id: b,
            hop: 1
        }]
    );
}

#[tokio::test]
async fn relation_follows_merge_forward_pointer() {
    let store = new_test_store().await;
    let source = source_record(&store, "merge").await;
    let survivor = resolved_person(&store, "Ryan", "ryan@acme.com").await;
    let loser = resolved_person(&store, "Ryan2", "ryan@personal.com").await;
    crate::entity::merge_entities(&store, &survivor, &loser)
        .await
        .unwrap();
    let rel = append_relation(
        &store,
        &source,
        &loser,
        "subject",
        &json!({}),
        "pair",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(rel.entity_id, survivor);
}
