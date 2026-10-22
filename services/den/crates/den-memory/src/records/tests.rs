use super::*;
use crate::relations::append_relation;
use crate::resolver::{resolve, Assertion, Resolution, Signal};
use crate::test_support::new_test_store;
use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

mod guarded;

fn at(rfc: &str) -> OffsetDateTime {
    OffsetDateTime::parse(rfc, &Rfc3339).unwrap()
}

async fn insert_version(
    store: &BearMemoryStore,
    memory_id: &str,
    seq: i64,
    valid_from: &str,
    supersedes: Option<&str>,
) {
    let bear_id = store.bear_id().to_string();
    sqlx::query!(
        r"
        INSERT INTO memory_records (
            memory_id, bear_id, sequence_no, scope_type, scope_profile, kind,
            author_profile, author_agent_id, created_at, content_text, metadata_json,
            visibility, logical_path, work_surface_ref, valid_from, supersedes_memory_id, salience
        ) VALUES (?, ?, ?, 'shared', NULL, 'note', 'curate', NULL, ?, 'policy text', '{}',
                  'normal', 'core/policy.md', NULL, ?, ?, 'normal')
        ",
        memory_id,
        bear_id,
        seq,
        valid_from,
        valid_from,
        supersedes,
    )
    .execute(store.pool())
    .await
    .expect("insert version");
}

#[tokio::test]
async fn head_record_as_of_walks_supersession_chain() {
    let store = new_test_store().await;
    insert_version(&store, "v1", 1, "2026-01-01T00:00:00Z", None).await;
    insert_version(&store, "v2", 2, "2026-03-01T00:00:00Z", Some("v1")).await;

    // Before anything existed.
    assert!(
        head_record_as_of(&store, "core/policy.md", at("2025-12-01T00:00:00Z"))
            .await
            .unwrap()
            .is_none()
    );

    // v1 is current until v2 becomes effective.
    let feb = head_record_as_of(&store, "core/policy.md", at("2026-02-01T00:00:00Z"))
        .await
        .unwrap()
        .expect("v1 head in feb");
    assert_eq!(feb.memory_id, "v1");

    // Once v2 is effective it supersedes v1.
    let apr = head_record_as_of(&store, "core/policy.md", at("2026-04-01T00:00:00Z"))
        .await
        .unwrap()
        .expect("v2 head in apr");
    assert_eq!(apr.memory_id, "v2");
}

#[tokio::test]
async fn append_record_options_store_salience_and_supersession() {
    let store = new_test_store().await;
    let path = LogicalMemoryPath::shared_core("policy");
    let first = store
        .append_record_with_options(
            &path,
            "note",
            "curate",
            None,
            "first",
            &serde_json::json!({}),
            "normal",
            "high",
            None,
        )
        .await
        .unwrap();
    let second = store
        .append_record_with_options(
            &path,
            "note",
            "curate",
            None,
            "second",
            &serde_json::json!({}),
            "normal",
            "critical",
            Some(&first.memory_id),
        )
        .await
        .unwrap();

    assert_eq!(first.salience, "high");
    assert_eq!(second.salience, "critical");
    let head = head_record_for_logical_path(&store, "core/policy.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.memory_id, second.memory_id);
    assert!(fetch_record_by_id(&store, &first.memory_id)
        .await
        .unwrap()
        .unwrap()
        .invalid_at
        .is_some());
}

#[tokio::test]
async fn lifecycle_markers_track_stale_archive_candidate_and_superseded() {
    let store = new_test_store().await;
    let path = LogicalMemoryPath::profile_local("pair", "note");
    let record = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "Durable role-local note.",
        &json!({}),
    )
    .await
    .unwrap();

    let stale = mark_memory_record_lifecycle(&store, &record.memory_id, "stale", Some("old fact"))
        .await
        .unwrap();
    assert_eq!(stale.lifecycle_status, "stale");
    assert_eq!(stale.freshness_trend, "stale");
    assert!(stale.invalid_at.is_none());

    let archive_candidate = mark_memory_record_lifecycle(
        &store,
        &record.memory_id,
        "archive-candidate",
        Some("low value outside hot recall"),
    )
    .await
    .unwrap();
    assert_eq!(archive_candidate.lifecycle_status, "archive-candidate");
    assert_eq!(archive_candidate.freshness_trend, "weakening");

    let superseded = mark_memory_record_lifecycle(
        &store,
        &record.memory_id,
        "superseded",
        Some("replaced by curated core entry"),
    )
    .await
    .unwrap();
    assert_eq!(superseded.lifecycle_status, "superseded");
    assert_eq!(superseded.freshness_trend, "stale");
    assert!(superseded.invalid_at.is_some());
}

#[tokio::test]
async fn active_path_reads_hide_superseded_archived_and_archive_candidates() {
    let store = new_test_store().await;
    let path = LogicalMemoryPath::profile_local("pair", "note");
    let superseded = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "superseded note",
        &json!({}),
    )
    .await
    .unwrap();
    mark_memory_record_lifecycle(&store, &superseded.memory_id, "superseded", None)
        .await
        .unwrap();
    append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "archived note",
        &json!({ "lifecycle": { "status": "archived" } }),
    )
    .await
    .unwrap();
    append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "archive candidate note",
        &json!({ "lifecycle": { "status": "archive-candidate" } }),
    )
    .await
    .unwrap();
    let active = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "active note",
        &json!({}),
    )
    .await
    .unwrap();

    let rows = list_records_for_logical_path(&store, "pair/note.md", 10)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, active.memory_id);
    assert_eq!(rows[0].lifecycle_status, "active");
    assert_eq!(rows[0].freshness_trend, "stable");

    let history = list_record_history_for_logical_path(&store, "pair/note.md", 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].memory_id, active.memory_id);
    assert!(history
        .iter()
        .any(|row| row.lifecycle_status == "superseded"));
    assert!(history.iter().any(|row| row.lifecycle_status == "archived"));
    assert!(history
        .iter()
        .any(|row| row.lifecycle_status == "archive-candidate"));
}

#[tokio::test]
async fn entity_anchor_heads_require_resolved_salient_subject_and_explicit_anchor() {
    let store = new_test_store().await;
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

    let source_path = LogicalMemoryPath::shared_core("notes/ryan-source");
    let source = store
        .append_record_with_options(
            &source_path,
            "note",
            "pair",
            None,
            "Ryan prefers concise architecture notes.",
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
        "pair",
        None,
        None,
    )
    .await
    .unwrap();

    assert!(list_entity_anchor_head_records(&store, 10)
        .await
        .unwrap()
        .is_empty());

    let anchor_path = LogicalMemoryPath::from_logical_path("core/people/ryan-dahl/profile.md");
    let anchor = append_memory_record(
        &store,
        &anchor_path,
        "profile",
        "curate",
        None,
        "Ryan Dahl: concise architecture notes.",
        &json!({}),
    )
    .await
    .unwrap();

    let anchors = list_entity_anchor_head_records(&store, 10).await.unwrap();
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].memory_id, anchor.memory_id);
}
