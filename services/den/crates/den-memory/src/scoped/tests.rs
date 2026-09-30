use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{
    append_memory_record,
    relations::append_relation,
    resolver::{resolve, Assertion, Resolution, Signal},
    test_support::new_test_store,
    LogicalMemoryPath,
};

#[tokio::test]
async fn direct_keyword_and_browse_only_see_own_source_curated_hat_and_core() {
    let store = new_test_store().await;
    let a = MemorySource::Conversation(Uuid::new_v4());
    let b = MemorySource::Conversation(Uuid::new_v4());
    let job = MemorySource::WorkRun(Uuid::new_v4());
    let hat = HatId::new(Uuid::new_v4());
    let other_hat = HatId::new(Uuid::new_v4());
    let records = [
        (LogicalMemoryPath::source_local(a, "note"), "private A"),
        (LogicalMemoryPath::source_local(b, "note"), "private B"),
        (LogicalMemoryPath::source_local(job, "note"), "private Work"),
        (LogicalMemoryPath::hat(hat, "note"), "reviewed hat"),
        (LogicalMemoryPath::hat(other_hat, "note"), "other hat"),
        (LogicalMemoryPath::shared_core("note"), "shared core"),
        (
            LogicalMemoryPath::profile_local("pair", "note"),
            "legacy pair",
        ),
    ];
    for (path, content) in &records {
        append_memory_record(&store, path, "note", "pair", None, content, &json!({}))
            .await
            .expect("write scoped fixture");
    }

    let a_grant = MemoryReadGrant::new(a, Some(hat));
    let b_grant = MemoryReadGrant::new(b, Some(hat));
    let work_grant = MemoryReadGrant::new(job, Some(hat));
    let no_hat = MemoryReadGrant::new(a, None);
    let access = AccessContext::empty();
    for (grant, expected) in [
        (a_grant, vec!["private A", "reviewed hat", "shared core"]),
        (b_grant, vec!["private B", "reviewed hat", "shared core"]),
        (
            work_grant,
            vec!["private Work", "reviewed hat", "shared core"],
        ),
        (no_hat, vec!["private A", "shared core"]),
    ] {
        let hits = search(&store, grant, &access, "", 20)
            .await
            .expect("keyword search");
        let contents: Vec<&str> = hits.iter().map(|row| row.content_text.as_str()).collect();
        assert_eq!(contents.len(), expected.len());
        for visible in &expected {
            assert!(
                contents.contains(visible),
                "missing visible record: {visible}"
            );
        }
        let paths = browse(&store, grant, &access).await.expect("browse");
        assert_eq!(paths.len(), expected.len());
        for (path, content) in &records {
            let readable = read_path(&store, grant, &access, &path.to_logical_path(), 20)
                .await
                .expect("read known path");
            assert_eq!(
                readable.iter().any(|row| row.content_text == *content),
                expected.contains(content)
            );
        }
    }
}

#[tokio::test]
async fn recent_source_is_source_only_even_with_newer_curated_or_colliding_rows() {
    let store = new_test_store().await;
    let own = MemorySource::Conversation(Uuid::new_v4());
    let other = MemorySource::Conversation(Uuid::new_v4());
    let own_note = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(own, "my-note"),
        "note",
        "pair",
        None,
        "only my private note",
        &json!({}),
    )
    .await
    .unwrap();
    for index in 0..70 {
        append_memory_record(
            &store,
            &LogicalMemoryPath::source_local(other, &format!("other-{index}")),
            "note",
            "pair",
            None,
            "someone else's source note",
            &json!({}),
        )
        .await
        .unwrap();
    }
    append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("recent-core"),
        "note",
        "curate",
        None,
        "curated core",
        &json!({}),
    )
    .await
    .unwrap();
    let own_records = recent_source(&store, MemoryReadGrant::new(own, None), 50)
        .await
        .unwrap();
    assert_eq!(own_records.len(), 1);
    assert_eq!(own_records[0].memory_id, own_note.memory_id);
    assert_eq!(
        recent_source(&store, MemoryReadGrant::new(other, None), 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn owned_note_preview_filters_scope_lifecycle_and_access_before_limit() {
    let store = new_test_store().await;
    let own = Uuid::new_v4();
    let another = Uuid::new_v4();
    let foreign = Uuid::new_v4();
    let add = |source: Uuid, label: &str| {
        let path = LogicalMemoryPath::source_local(MemorySource::Conversation(source), label);
        (path, label.to_string())
    };
    let (path, label) = add(own, "visible-first");
    let first = append_memory_record(&store, &path, "note", "pair", None, &label, &json!({}))
        .await
        .unwrap();
    let (path, label) = add(another, "visible-second");
    let second = append_memory_record(&store, &path, "note", "pair", None, &label, &json!({}))
        .await
        .unwrap();
    for index in 0..12 {
        let (path, label) = add(foreign, &format!("foreign-{index}"));
        append_memory_record(&store, &path, "note", "pair", None, &label, &json!({}))
            .await
            .unwrap();
    }
    for (label, metadata, visibility) in [
        (
            "archived",
            json!({"lifecycle": {"status": "archived"}}),
            "normal",
        ),
        (
            "candidate",
            json!({"lifecycle": {"status": "archive-candidate"}}),
            "normal",
        ),
        ("hidden", json!({}), "hidden"),
    ] {
        let (path, _) = add(own, label);
        store
            .append_record(&path, "note", "pair", None, label, &metadata, visibility)
            .await
            .unwrap();
    }
    let (path, _) = add(own, "restricted");
    let restricted = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "restricted",
        &json!({}),
    )
    .await
    .unwrap();
    // A rule must exclude the row even without an access context.
    sqlx::query("INSERT INTO memory_access_rules (link_id, bear_id, sequence_no, src_memory_id, relation, entity_id, author_profile, created_at) VALUES (?, ?, ?, ?, 'confined_to', ?, 'curate', '2026-01-01T00:00:00Z')")
        .bind(Uuid::new_v4().to_string())
        .bind(store.bear_id().to_string())
        .bind(store.next_sequence().await.unwrap())
        .bind(&restricted.memory_id)
        .bind(Uuid::new_v4().to_string())
        .execute(store.pool()).await.unwrap();
    let (path, _) = add(own, "superseded");
    let superseded = append_memory_record(&store, &path, "note", "pair", None, "old", &json!({}))
        .await
        .unwrap();
    let replacement = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "replacement",
        &json!({}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE memory_records SET supersedes_memory_id = ? WHERE memory_id = ?")
        .bind(&superseded.memory_id)
        .bind(&replacement.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    let collision = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "not-a-source"),
        "note",
        "pair",
        None,
        "colliding path",
        &json!({}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE memory_records SET logical_path = ? WHERE memory_id = ?")
        .bind(first.logical_path.as_ref().unwrap())
        .bind(&collision.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    let notes = recent_own_conversation_notes(&store, &[own, another], 8)
        .await
        .unwrap();
    let ids: Vec<_> = notes.iter().map(|note| note.memory_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            replacement.memory_id.as_str(),
            second.memory_id.as_str(),
            first.memory_id.as_str()
        ]
    );
    assert_eq!(notes[0].conversation_id, own);
    assert_eq!(notes[1].conversation_id, another);
    assert!(recent_own_conversation_notes(&store, &[], 8)
        .await
        .unwrap()
        .is_empty());
    assert!(recent_own_conversation_notes(&store, &vec![own; 201], 8)
        .await
        .is_err());
}

#[tokio::test]
async fn path_collision_cannot_read_legacy_record_through_visible_scope() {
    let store = new_test_store().await;
    let source = MemorySource::Conversation(Uuid::new_v4());
    let path = LogicalMemoryPath::source_local(source, "note");
    let shared_path = path.to_logical_path();
    append_memory_record(&store, &path, "note", "pair", None, "owned", &json!({}))
        .await
        .expect("write source note");
    // A legacy writer can store an unrelated record at the same logical path.
    // Authorization must be per row, not inferred from a visible path prefix.
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type,
            scope_profile, kind, author_profile, created_at, content_text, logical_path)
         VALUES (?, ?, 900, 'profile_local', 'pair', 'note', 'pair',
                 '2026-01-01T00:00:00Z', 'legacy secret', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(store.bear_id().to_string())
    .bind(&shared_path)
    .execute(store.pool())
    .await
    .expect("create legacy path collision");
    let grant = MemoryReadGrant::new(source, None);
    let access = AccessContext::empty();
    let read = read_path(&store, grant, &access, &shared_path, 20)
        .await
        .expect("read owned note");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].content_text, "owned");
    let hits = search(&store, grant, &access, "legacy secret", 20)
        .await
        .expect("search private text");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn access_bearing_rules_still_constrain_scoped_reads() {
    let store = new_test_store().await;
    let source = MemorySource::Conversation(Uuid::new_v4());
    let path = LogicalMemoryPath::source_local(source, "note");
    let row = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "confidential note",
        &json!({}),
    )
    .await
    .expect("write source note");
    let surface = match resolve(
        &store,
        "work_surface",
        Some("protected repo"),
        &[Signal::new("git_remote", "github.com/acme/protected")],
        Assertion::Asserted,
    )
    .await
    .expect("resolve protected surface")
    {
        Resolution::Resolved(entity) => entity.entity_id,
        other => panic!("expected resolved surface, got {other:?}"),
    };
    append_relation(
        &store,
        &row.memory_id,
        &surface,
        "confined_to",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .expect("gate source record");

    let grant = MemoryReadGrant::new(source, None);
    let denied = AccessContext::empty();
    assert!(
        read_path(&store, grant, &denied, &path.to_logical_path(), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(search(&store, grant, &denied, "confidential", 10)
        .await
        .unwrap()
        .is_empty());
    assert!(browse(&store, grant, &denied).await.unwrap().is_empty());
    assert!(recent_source(&store, grant, 10).await.unwrap().is_empty());

    let allowed = AccessContext::empty().with_confinement([surface]);
    assert_eq!(
        read_path(&store, grant, &allowed, &path.to_logical_path(), 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        search(&store, grant, &allowed, "confidential", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(browse(&store, grant, &allowed).await.unwrap().len(), 1);
}
