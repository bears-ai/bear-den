use super::*;
use crate::{access::AccessContext, resolve_authenticated_user_entity, AuthenticatedUserEntity};
use den_core::ids::UserId;

async fn audience_rule(store: &BearMemoryStore, record_id: &str, owner: &AuthenticatedUserEntity) {
    append_relation(
        store,
        record_id,
        owner.entity_id(),
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
}

async fn set_rule_state(store: &BearMemoryStore, record_id: &str, state: &str) {
    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        "UPDATE memory_access_rules SET state = ? WHERE bear_id = ? AND src_memory_id = ?",
        state,
        bear_id,
        record_id,
    )
    .execute(store.pool())
    .await
    .unwrap();
}

async fn assert_generic_read_denied(store: &BearMemoryStore, record: &MemoryRecordRow) {
    let path = record.logical_path.as_deref().unwrap();
    let ids = std::slice::from_ref(&record.memory_id);
    assert!(fetch_record_by_id(store, &record.memory_id)
        .await
        .unwrap()
        .is_none());
    assert!(head_record_for_logical_path(store, path)
        .await
        .unwrap()
        .is_none());
    assert!(list_records_for_logical_path(store, path, 1)
        .await
        .unwrap()
        .is_empty());
    assert!(list_record_history_for_logical_path(store, path, 1)
        .await
        .unwrap()
        .is_empty());
    assert!(
        list_profile_local_head_records(store, "pair", record.work_surface_ref.as_deref(), 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fetch_records_min(store, ids, "pair")
        .await
        .unwrap()
        .is_empty());
    assert!(head_record_as_of(store, path, at(&record.created_at))
        .await
        .unwrap()
        .is_none());
    assert!(effective_time_by_ids(store, ids).await.unwrap().is_empty());
    assert_eq!(
        memory_sequence_high_water(store).await.unwrap(),
        record.sequence_no
    );
}

#[tokio::test]
async fn generic_record_read_matrix_hides_all_access_rule_history() {
    let mut surface = LogicalMemoryPath::profile_local("pair", "note");
    surface.work_surface_ref = Some("client-a".into());
    for path in [
        LogicalMemoryPath::shared_core("note"),
        LogicalMemoryPath::profile_local("pair", "note"),
        surface,
    ] {
        let store = new_test_store().await;
        let owner = resolve_authenticated_user_entity(&store, UserId::new(11))
            .await
            .unwrap();
        let record = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "Protected record canary",
            &json!({}),
        )
        .await
        .unwrap();
        audience_rule(&store, &record.memory_id, &owner).await;
        let access = AccessContext::for_authenticated_user(&owner);
        assert_eq!(
            head_record_for_logical_path_with_access(&store, &path.to_logical_path(), &access)
                .await
                .unwrap()
                .unwrap()
                .memory_id,
            record.memory_id,
        );
        assert!(head_record_for_logical_path_with_access(
            &store,
            &path.to_logical_path(),
            &AccessContext::empty().with_audience([owner.entity_id()]),
        )
        .await
        .unwrap()
        .is_none());
        for state in ["active", "withdrawn", "superseded"] {
            set_rule_state(&store, &record.memory_id, state).await;
            assert_generic_read_denied(&store, &record).await;
            if state != "active" {
                assert!(head_record_for_logical_path_with_access(
                    &store,
                    &path.to_logical_path(),
                    &access,
                )
                .await
                .unwrap()
                .is_none());
            }
        }
    }
}

#[tokio::test]
async fn hidden_successors_never_resurrect_generic_current_or_as_of_heads() {
    let store = new_test_store().await;
    let owner = resolve_authenticated_user_entity(&store, UserId::new(11))
        .await
        .unwrap();
    // Deliberately leave invalid_at unset to test the canonical supersession edges themselves.
    insert_version(&store, "v1", 100, "2026-01-01T00:00:00Z", None).await;
    insert_version(&store, "v2", 101, "2026-03-01T00:00:00Z", Some("v1")).await;
    audience_rule(&store, "v2", &owner).await;
    let path = "core/policy.md";
    assert!(head_record_for_logical_path(&store, path)
        .await
        .unwrap()
        .is_none());
    assert!(list_records_for_logical_path(&store, path, 10)
        .await
        .unwrap()
        .is_empty());
    let ids = vec!["v1".into(), "v2".into()];
    assert!(fetch_records_min(&store, &ids, "pair")
        .await
        .unwrap()
        .is_empty());
    assert!(head_record_as_of(&store, path, at("2026-04-01T00:00:00Z"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        head_record_as_of(&store, path, at("2026-02-01T00:00:00Z"))
            .await
            .unwrap()
            .unwrap()
            .memory_id,
        "v1",
    );
    let times = superseder_times_by_superseded(&store, &ids).await.unwrap();
    assert_eq!(times["v1"], vec![at("2026-03-01T00:00:00Z")]);
    assert!(!times.contains_key("v2"));
    let history = list_record_history_for_logical_path(&store, path, 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].memory_id, "v1");
    assert_eq!(
        head_record_for_logical_path_with_access(
            &store,
            path,
            &AccessContext::for_authenticated_user(&owner),
        )
        .await
        .unwrap()
        .unwrap()
        .memory_id,
        "v2",
    );
    assert_eq!(memory_sequence_high_water(&store).await.unwrap(), 101);
}

#[tokio::test]
async fn generic_limits_apply_after_guarding_and_authorized_surface_anchors_still_work() {
    let store = new_test_store().await;
    let owner = resolve_authenticated_user_entity(&store, UserId::new(11))
        .await
        .unwrap();
    let path = LogicalMemoryPath::profile_local("pair", "note");
    let mut records = Vec::new();
    for content in ["Public record", "Protected record"] {
        records.push(
            append_memory_record(&store, &path, "note", "pair", None, content, &json!({}))
                .await
                .unwrap(),
        );
    }
    audience_rule(&store, &records[1].memory_id, &owner).await;
    for rows in [
        list_records_for_logical_path(&store, &path.to_logical_path(), 1)
            .await
            .unwrap(),
        list_record_history_for_logical_path(&store, &path.to_logical_path(), 1)
            .await
            .unwrap(),
        list_profile_local_head_records(&store, "pair", None, 1)
            .await
            .unwrap(),
    ] {
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_id, records[0].memory_id);
    }
    let mut path = LogicalMemoryPath::shared_core("index");
    path.work_surface_ref = Some("client-a".into());
    let record = append_memory_record(
        &store,
        &path,
        "index",
        "curate",
        None,
        "Protected surface",
        &json!({}),
    )
    .await
    .unwrap();
    audience_rule(&store, &record.memory_id, &owner).await;
    assert!(!has_work_surface_canonical_anchor(&store, "client-a")
        .await
        .unwrap());
    assert!(has_work_surface_canonical_anchor_with_access(
        &store,
        "client-a",
        &AccessContext::for_authenticated_user(&owner),
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn entity_anchors_require_authorized_salience_evidence_and_authorized_bodies() {
    let store = new_test_store().await;
    let owner = resolve_authenticated_user_entity(&store, UserId::new(11))
        .await
        .unwrap();
    let entity = match resolve(
        &store,
        "person",
        Some("Ryan Dahl"),
        &[Signal::new("email", "ryan@example.com")],
        Assertion::Inferred,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(entity) => entity,
        other => panic!("expected resolved entity, got {other:?}"),
    };
    let source = store
        .append_record_with_options(
            &LogicalMemoryPath::shared_core("source"),
            "note",
            "curate",
            None,
            "Protected salience evidence",
            &json!({}),
            "normal",
            "high",
            None,
        )
        .await
        .unwrap();
    append_relation(
        &store,
        &source.memory_id,
        &entity.entity_id,
        "subject",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    let anchor = append_memory_record(
        &store,
        &LogicalMemoryPath::from_logical_path("core/people/ryan-dahl/profile.md"),
        "profile",
        "curate",
        None,
        "Anchor body",
        &json!({}),
    )
    .await
    .unwrap();
    audience_rule(&store, &source.memory_id, &owner).await;
    let allowed = AccessContext::for_authenticated_user(&owner);
    let denied = AccessContext::empty().with_audience([owner.entity_id()]);
    // A public anchor must not become eligible solely through hidden subject evidence.
    for protect_anchor in [false, true] {
        if protect_anchor {
            audience_rule(&store, &anchor.memory_id, &owner).await;
        }
        assert!(list_entity_anchor_head_records(&store, 10)
            .await
            .unwrap()
            .is_empty());
        assert!(
            list_entity_anchor_head_records_with_access(&store, 10, &denied)
                .await
                .unwrap()
                .is_empty()
        );
        let rows = list_entity_anchor_head_records_with_access(&store, 10, &allowed)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_id, anchor.memory_id);
    }
    set_rule_state(&store, &source.memory_id, "withdrawn").await;
    assert!(
        list_entity_anchor_head_records_with_access(&store, 10, &allowed)
            .await
            .unwrap()
            .is_empty()
    );
}
