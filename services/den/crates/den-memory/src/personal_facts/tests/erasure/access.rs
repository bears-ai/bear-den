use super::*;
use crate::scoped::{read_path, search, MemoryReadGrant};

#[tokio::test]
async fn scoped_erased_evidence_is_hidden_with_or_without_acl_while_unrelated_notes_survive() {
    for source_has_acl in [false, true] {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let bob = person(&store, BOB).await;
        let conversation = Uuid::new_v4();
        let path = LogicalMemoryPath::source_local(
            MemorySource::Conversation(conversation),
            "erasure-source",
        );
        let other_path =
            LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "unrelated");
        let name = "ScopedReplayNameSentinel-1F6C";
        let original = "Please remember my preferred name is ScopedReplayNameSentinel-1F6C. SourceEvidenceOnlySentinel.";
        let unrelated = "Unrelated note also mentions ScopedReplayNameSentinel-1F6C but is not erased source evidence.";
        let source_row = store
            .append_record(&path, "note", "pair", None, original, &json!({}), "normal")
            .await
            .unwrap();
        let other_row = store
            .append_record(
                &other_path,
                "note",
                "pair",
                None,
                unrelated,
                &json!({}),
                "normal",
            )
            .await
            .unwrap();
        let source = Note {
            conversation,
            memory: Uuid::parse_str(&source_row.memory_id).unwrap(),
        };
        publish(&store, &alice, source, name, None).await;
        if source_has_acl {
            // Source admission precedes this ordinary audience grant. Erasure must override
            // even a still-active ACL which would otherwise authorize the source's owner.
            crate::append_relation(
                &store,
                &source_row.memory_id,
                alice.entity_id(),
                "audience",
                &json!({}),
                "curate",
                None,
                None,
            )
            .await
            .unwrap();
        }
        let grant = MemoryReadGrant::new(MemorySource::Conversation(conversation), None);
        let path = path.to_logical_path();
        let other_path = other_path.to_logical_path();
        let owner = AccessContext::for_authenticated_user(&alice);
        assert!(crate::record_visible(&store, &source_row.memory_id, &owner)
            .await
            .unwrap());
        let direct = read_path(&store, grant, &owner, &path, 100).await.unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].memory_id, source_row.memory_id);
        assert_eq!(direct[0].content_text, original);
        let found = search(&store, grant, &owner, "SourceEvidenceOnlySentinel", 100)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].memory_id, source_row.memory_id);

        Reviewed::read(&store, &alice)
            .await
            .erase(&store, &alice)
            .await
            .unwrap();
        for access in [
            owner,
            AccessContext::empty(),
            AccessContext::for_authenticated_user(&bob),
        ] {
            assert!(
                !crate::record_visible(&store, &source_row.memory_id, &access)
                    .await
                    .unwrap()
            );
            assert!(read_path(&store, grant, &access, &path, 100)
                .await
                .unwrap()
                .is_empty());
            assert!(
                search(&store, grant, &access, "SourceEvidenceOnlySentinel", 100)
                    .await
                    .unwrap()
                    .is_empty()
            );
            // A matching name in a different record is not itself an erasure classification.
            let found = search(&store, grant, &access, name, 100).await.unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].memory_id, other_row.memory_id);
            assert_eq!(found[0].content_text, unrelated);
            assert!(crate::record_visible(&store, &other_row.memory_id, &access)
                .await
                .unwrap());
            let direct = read_path(&store, grant, &access, &other_path, 100)
                .await
                .unwrap();
            assert_eq!(direct.len(), 1);
            assert_eq!(direct[0].memory_id, other_row.memory_id);
            assert_eq!(direct[0].content_text, unrelated);
        }
        assert_eq!(
            crate::list_access_rules_for_source(&store, &source_row.memory_id)
                .await
                .unwrap()
                .len(),
            usize::from(source_has_acl),
        );
        assert!(canonical_record_contents_match(
            &store,
            &[
                (&source_row.memory_id, original),
                (&other_row.memory_id, unrelated),
            ]
        )
        .await
        .unwrap());
    }
}

#[tokio::test]
async fn scoped_classified_fact_checks_owner_and_eligibility_before_empty_acl_grant() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = person(&store, BOB).await;
    let source = note(&store).await;
    let name = "ClassifiedLostAclNameSentinel-4B32";
    let fact = publish(&store, &alice, source, name, None).await;
    let id = fact.fact_id.to_string();
    let path = LogicalMemoryPath::source_local(
        MemorySource::Conversation(source.conversation),
        "classified",
    );
    fixtures::alter_record(&store, &id, &path, "normal", "{}", None).await;
    let path = path.to_logical_path();
    let grant = MemoryReadGrant::new(MemorySource::Conversation(source.conversation), None);
    let owner = AccessContext::for_authenticated_user(&alice);
    assert!(crate::record_visible(&store, &id, &owner).await.unwrap());
    let visible = read_path(&store, grant, &owner, &path, 100).await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].memory_id, id);
    assert_eq!(visible[0].content_text, name);
    for access in [
        AccessContext::empty(),
        AccessContext::empty().with_audience([alice.entity_id()]),
        AccessContext::for_authenticated_user(&bob),
    ] {
        assert!(!crate::record_visible(&store, &id, &access).await.unwrap());
        assert!(read_path(&store, grant, &access, &path, 100)
            .await
            .unwrap()
            .is_empty());
        assert!(search(&store, grant, &access, name, 100)
            .await
            .unwrap()
            .is_empty());
    }

    fixtures::lose_all_acl(&store, &id).await;
    assert!(crate::list_access_rules_for_source(&store, &id)
        .await
        .unwrap()
        .is_empty());
    for access in [
        owner,
        AccessContext::empty(),
        AccessContext::for_authenticated_user(&bob),
    ] {
        assert!(!crate::record_visible(&store, &id, &access).await.unwrap());
        assert!(read_path(&store, grant, &access, &path, 100)
            .await
            .unwrap()
            .is_empty());
        assert!(search(&store, grant, &access, name, 100)
            .await
            .unwrap()
            .is_empty());
    }
    assert_head(&store, &alice, fact, PersonalFactState::Suppressed, None).await;
    assert!(canonical_record_contents_match(&store, &[(&id, name)])
        .await
        .unwrap());
}
