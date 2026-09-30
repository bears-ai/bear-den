use den_core::ids::HatId;
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{
    append_memory_record, append_relation,
    resolver::{resolve, Assertion, Resolution, Signal},
    test_support::new_test_store,
    LogicalMemoryPath, MemoryScopeType, MemorySource,
};

async fn add(store: &BearMemoryStore, path: &LogicalMemoryPath, text: &str) -> MemoryRecordRow {
    append_memory_record(store, path, "note", "curate", None, text, &json!({}))
        .await
        .expect("append record")
}

async fn collide(store: &BearMemoryStore, row: &MemoryRecordRow, path: &str) {
    sqlx::query("UPDATE memory_records SET logical_path = ? WHERE memory_id = ?")
        .bind(path)
        .bind(&row.memory_id)
        .execute(store.pool())
        .await
        .expect("create path collision");
}

#[tokio::test]
async fn library_grants_only_shared_and_bound_hats_even_for_guessed_ids_and_colliding_paths() {
    let store = new_test_store().await;
    let a = HatId::new(Uuid::new_v4());
    let b = HatId::new(Uuid::new_v4());
    let other = HatId::new(Uuid::new_v4());
    let shared = add(&store, &LogicalMemoryPath::shared_core("note"), "shared").await;
    let hat_a = add(&store, &LogicalMemoryPath::hat(a, "note"), "hat A").await;
    let hat_a_second = add(&store, &LogicalMemoryPath::hat(a, "note"), "hat A second").await;
    let hat_b = add(&store, &LogicalMemoryPath::hat(b, "note"), "hat B").await;
    let other_hat = add(&store, &LogicalMemoryPath::hat(other, "note"), "other hat").await;
    let source = add(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note"),
        "private source",
    )
    .await;
    let legacy = add(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "legacy",
    )
    .await;
    let path = shared.logical_path.as_deref().unwrap();
    for row in [&hat_a, &hat_a_second, &hat_b, &other_hat, &source, &legacy] {
        collide(&store, row, path).await;
    }

    let shared_only = CuratedMemoryGrant::new(vec![]);
    let grant = CuratedMemoryGrant::new(vec![a, b]);
    let shared_rows = history(&store, &shared_only, &shared.memory_id, 20)
        .await
        .unwrap();
    assert_eq!(shared_rows.len(), 1);
    assert_eq!(shared_rows[0].memory_id, shared.memory_id);
    let a_rows = history(&store, &grant, &hat_a.memory_id, 20).await.unwrap();
    assert_eq!(a_rows.len(), 2);
    assert_eq!(a_rows[0].memory_id, hat_a_second.memory_id);
    assert_eq!(a_rows[1].memory_id, hat_a.memory_id);
    let b_rows = history(&store, &grant, &hat_b.memory_id, 20).await.unwrap();
    assert_eq!(b_rows.len(), 1);
    assert_eq!(b_rows[0].memory_id, hat_b.memory_id);
    assert!(history(&store, &shared_only, &hat_a.memory_id, 20)
        .await
        .unwrap()
        .is_empty());
    for row in [&shared, &hat_a, &hat_a_second, &hat_b] {
        assert_eq!(
            detail(&store, &grant, &row.memory_id)
                .await
                .unwrap()
                .unwrap()
                .content_text,
            row.content_text
        );
    }
    for row in [&other_hat, &source, &legacy] {
        assert!(detail(&store, &grant, &row.memory_id)
            .await
            .unwrap()
            .is_none());
        assert!(history(&store, &grant, &row.memory_id, 20)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(detail(&store, &grant, "guessed-id")
        .await
        .unwrap()
        .is_none());
    assert!(history(&store, &grant, "guessed-id", 20)
        .await
        .unwrap()
        .is_empty());

    let summaries = browse(&store, &grant).await.unwrap();
    assert_eq!(
        summaries.len(),
        3,
        "shared and each hat must not merge at a colliding path"
    );
    assert!(summaries.iter().all(|s| s.logical_path == path));
    assert_eq!(summaries.iter().map(|s| s.version_count).sum::<i64>(), 4);
    let a_summary = summaries
        .iter()
        .find(|s| s.head_memory_id == hat_a_second.memory_id)
        .unwrap();
    assert_eq!(a_summary.version_count, 2);
    assert_eq!(summaries.iter().filter(|s| s.version_count == 1).count(), 2);
    assert_eq!(
        summaries.iter().filter(|s| s.scope_type == "hat").count(),
        2
    );
    assert_eq!(browse(&store, &shared_only).await.unwrap().len(), 1);
    assert_eq!(
        count_current_entries(&store, &shared_only).await.unwrap(),
        1
    );
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 3);
    let hits = recent(&store, &grant, 20).await.unwrap();
    assert_eq!(hits.len(), 4);
    assert_eq!(
        search(&store, &grant, "private source", 20)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(search(&store, &grant, "hat", 20).await.unwrap().len(), 3);

    let pathless = add(
        &store,
        &LogicalMemoryPath::shared_core("pathless"),
        "no path",
    )
    .await;
    sqlx::query("UPDATE memory_records SET logical_path = NULL WHERE memory_id = ?")
        .bind(&pathless.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 3);
}

#[tokio::test]
async fn every_version_must_pass_lifecycle_visibility_and_access_gates() {
    let store = new_test_store().await;
    let grant = CuratedMemoryGrant::new(vec![]);
    let path = LogicalMemoryPath::shared_core("versions");
    let old = add(&store, &path, "eligible older").await;
    let head = add(&store, &path, "eligible head").await;
    // The old row has not been invalidated; supersession alone removes it from current views.
    sqlx::query("UPDATE memory_records SET supersedes_memory_id = ? WHERE memory_id = ?")
        .bind(&old.memory_id)
        .bind(&head.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(detail(&store, &grant, &old.memory_id)
        .await
        .unwrap()
        .is_some());
    assert!(current_detail(&store, &grant, &old.memory_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        current_detail(&store, &grant, &head.memory_id)
            .await
            .unwrap()
            .unwrap()
            .content_text,
        "eligible head"
    );
    let hidden = store
        .append_record(
            &path,
            "note",
            "curate",
            None,
            "hidden",
            &json!({}),
            "hidden",
        )
        .await
        .unwrap();
    let invalid = add(&store, &path, "invalid").await;
    sqlx::query("UPDATE memory_records SET invalid_at = ? WHERE memory_id = ?")
        .bind("2026-01-01T00:00:00Z")
        .bind(&invalid.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    let archived = append_memory_record(
        &store,
        &path,
        "note",
        "curate",
        None,
        "archived",
        &json!({"lifecycle":{"status":"archived"}}),
    )
    .await
    .unwrap();
    let candidate = append_memory_record(
        &store,
        &path,
        "note",
        "curate",
        None,
        "candidate",
        &json!({"lifecycle":{"status":"archive-candidate"}}),
    )
    .await
    .unwrap();
    let gated = add(&store, &path, "access-bearing").await;
    let surface = match resolve(
        &store,
        "work_surface",
        Some("restricted"),
        &[Signal::new(
            "git_remote",
            "github.com/example/private-library",
        )],
        Assertion::Asserted,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected resolved surface, got {other:?}"),
    };
    append_relation(
        &store,
        &gated.memory_id,
        &surface,
        "confined_to",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();

    let all = history(&store, &grant, &old.memory_id, 50).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].memory_id, head.memory_id);
    assert_eq!(all[1].memory_id, old.memory_id);
    for row in [&hidden, &invalid, &archived, &candidate, &gated] {
        assert!(detail(&store, &grant, &row.memory_id)
            .await
            .unwrap()
            .is_none());
        assert!(history(&store, &grant, &row.memory_id, 50)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(detail(&store, &grant, &old.memory_id)
        .await
        .unwrap()
        .is_some());
    let recent_rows = recent(&store, &grant, 50).await.unwrap();
    assert_eq!(recent_rows.len(), 1);
    assert_eq!(recent_rows[0].memory_id, head.memory_id);
    let search_rows = search(&store, &grant, "eligible", 50).await.unwrap();
    assert_eq!(search_rows.len(), 1);
    assert_eq!(search_rows[0].memory_id, head.memory_id);
    let summaries = browse(&store, &grant).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].head_memory_id, head.memory_id);
    assert_eq!(summaries[0].version_count, 2);
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 1);

    // Appending a superseding record invalidates the previous version as well.
    let superseding = store
        .append_record_with_options(
            &path,
            "note",
            "curate",
            None,
            "replacement",
            &json!({}),
            "normal",
            "normal",
            Some(&head.memory_id),
        )
        .await
        .unwrap();
    assert!(detail(&store, &grant, &head.memory_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        history(&store, &grant, &old.memory_id, 50)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        browse(&store, &grant).await.unwrap()[0].head_memory_id,
        superseding.memory_id
    );
    assert_eq!(browse(&store, &grant).await.unwrap()[0].version_count, 2);
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 1);

    // A visible historical row cannot become a browse head if its successor is gated.
    let lonely = add(
        &store,
        &LogicalMemoryPath::shared_core("lonely"),
        "old lonely",
    )
    .await;
    let gated_successor = add(
        &store,
        &LogicalMemoryPath::shared_core("lonely"),
        "new lonely",
    )
    .await;
    sqlx::query("UPDATE memory_records SET supersedes_memory_id = ? WHERE memory_id = ?")
        .bind(&lonely.memory_id)
        .bind(&gated_successor.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    append_relation(
        &store,
        &gated_successor.memory_id,
        &surface,
        "confined_to",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    assert!(browse(&store, &grant)
        .await
        .unwrap()
        .iter()
        .all(|s| s.logical_path != lonely.logical_path.as_deref().unwrap()));
    assert_eq!(
        history(&store, &grant, &lonely.memory_id, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 1);
}

#[tokio::test]
async fn limited_reads_page_past_access_bearing_records() {
    let store = new_test_store().await;
    let grant = CuratedMemoryGrant::new(vec![]);
    let path = LogicalMemoryPath::shared_core("paged");
    let first = add(&store, &path, "findable first").await;
    let second = add(&store, &path, "findable second").await;
    let surface = match resolve(
        &store,
        "work_surface",
        Some("private"),
        &[Signal::new(
            "git_remote",
            "github.com/example/paged-private",
        )],
        Assertion::Asserted,
    )
    .await
    .unwrap()
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected resolved surface, got {other:?}"),
    };
    let mut restricted_id = String::new();
    for _ in 0..70 {
        let restricted = add(&store, &path, "findable restricted").await;
        restricted_id = restricted.memory_id.clone();
        append_relation(
            &store,
            &restricted.memory_id,
            &surface,
            "confined_to",
            &json!({}),
            "curate",
            None,
            None,
        )
        .await
        .unwrap();
    }
    assert!(history(&store, &grant, &restricted_id, 2)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count_current_entries(&store, &grant).await.unwrap(), 1);
    for rows in [
        recent(&store, &grant, 2).await.unwrap(),
        search(&store, &grant, "findable", 2).await.unwrap(),
        history(&store, &grant, &first.memory_id, 2).await.unwrap(),
    ] {
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].memory_id, second.memory_id);
        assert_eq!(rows[1].memory_id, first.memory_id);
    }
    assert_eq!(
        recent(&store, &grant, 1).await.unwrap()[0].memory_id,
        second.memory_id
    );
    assert_eq!(
        search(&store, &grant, "findable", 1).await.unwrap()[0].memory_id,
        second.memory_id
    );
    assert_eq!(
        history(&store, &grant, &first.memory_id, 1).await.unwrap()[0].memory_id,
        second.memory_id
    );
}

#[tokio::test]
async fn keyword_like_is_literal_and_scope_filter_precedes_limits() {
    let store = new_test_store().await;
    let grant = CuratedMemoryGrant::new(vec![]);
    let first = add(
        &store,
        &LogicalMemoryPath::shared_core("literal"),
        r"A%_\B memory",
    )
    .await;
    let second = add(
        &store,
        &LogicalMemoryPath::shared_core("other"),
        "plain memory",
    )
    .await;
    for _ in 0..5 {
        add(
            &store,
            &LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "noise"),
            "A%_\\B memory",
        )
        .await;
        add(
            &store,
            &LogicalMemoryPath::profile_local("pair", "noise"),
            "A%_\\B memory",
        )
        .await;
    }
    assert_eq!(
        recent(&store, &grant, 1).await.unwrap()[0].memory_id,
        second.memory_id
    );
    assert_eq!(recent(&store, &grant, 0).await.unwrap().len(), 1);
    let literal = search(&store, &grant, r"%_\", 1).await.unwrap();
    assert_eq!(literal.len(), 1);
    assert_eq!(literal[0].memory_id, first.memory_id);
    assert!(search(&store, &grant, "AxxB", 10).await.unwrap().is_empty());
    assert_eq!(
        search(&store, &grant, "MEMORY", 1).await.unwrap()[0].memory_id,
        second.memory_id
    );
    assert_eq!(
        history(&store, &grant, &first.memory_id, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(first.scope_type, MemoryScopeType::Shared);
}
