use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{test_support::new_test_store, MemorySource};

#[tokio::test]
async fn source_entry_uses_canonical_owner_without_promoting_legacy_notes() {
    use crate::{
        scoped::{self, MemoryReadGrant},
        AccessContext,
    };

    let store = new_test_store().await;
    let source = MemorySource::Conversation(Uuid::new_v4());
    let tags = vec!["review".to_string()];
    let entry = SqliteMemoryEntryWrite {
        kind: "note",
        title: "Finding",
        body: "Only this conversation has the raw finding",
        tags: &tags,
        refs: None,
        lifecycle: Some(json!({ "status": "active" })),
        source: Some(json!({ "origin": "test" })),
        author: Some("human".to_string()),
    };
    let written = write_semantic_entry(
        &store,
        LogicalMemoryPath::source_local(source, "note"),
        "pair",
        entry,
    )
    .await
    .expect("write source-local memory");
    let path = written["path"].as_str().expect("logical path");
    let own = scoped::read_path(
        &store,
        MemoryReadGrant::new(source, None),
        &AccessContext::empty(),
        path,
        10,
    )
    .await
    .expect("read own source memory");
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].metadata_json["source"]["origin"], "test");
    assert_eq!(own[0].metadata_json["tags"], json!(["review"]));
    assert!(scoped::read_path(
        &store,
        MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), None),
        &AccessContext::empty(),
        path,
        10,
    )
    .await
    .unwrap()
    .is_empty());
}
