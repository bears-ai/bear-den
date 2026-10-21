use den_core::ids::HatId;

use super::*;

async fn reject_source(store: &BearMemoryStore, alice: &AuthenticatedUserEntity, source: Note) {
    let before = snapshot(store).await;
    let generic_before = generic_snapshot(store).await;
    assert_denied(
        stage_preferred_name(
            store,
            &source.proof(alice),
            PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    assert_eq!(snapshot(store).await, before);
    assert_eq!(generic_snapshot(store).await, generic_before);
}

#[tokio::test]
async fn source_kind_scope_and_uuid_are_canonical_columns_not_path_or_metadata_claims() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let conversation = Uuid::new_v4();
    for logical in [
        LogicalMemoryPath::source_local(MemorySource::WorkRun(conversation), "note"),
        LogicalMemoryPath::source_local(MemorySource::Intake(conversation), "note"),
        LogicalMemoryPath::profile_local("pair", "note"),
        LogicalMemoryPath::shared_core("note"),
        LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "note"),
    ] {
        let metadata =
            serde_json::json!({"conversation_id": conversation, "owner_user_id": ALICE.get()})
                .to_string();
        let memory = insert_note(&store, &logical, &metadata, NoteVisibility::Normal).await;
        let id = memory.to_string();
        let claimed_path =
            LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note")
                .to_logical_path();
        sqlx::query!(
            "UPDATE memory_records SET logical_path = ? WHERE memory_id = ?",
            claimed_path,
            id
        )
        .execute(store.pool())
        .await
        .unwrap();
        reject_source(
            &store,
            &alice,
            Note {
                conversation,
                memory,
            },
        )
        .await;
    }
    let correct = note_in(&store, conversation).await;
    reject_source(
        &store,
        &alice,
        Note {
            conversation: Uuid::new_v4(),
            ..correct
        },
    )
    .await;
    reject_source(
        &store,
        &alice,
        Note {
            memory: Uuid::new_v4(),
            ..correct
        },
    )
    .await;
    assert_eq!(
        stage(&store, &alice, correct, "Alice").await.state(),
        CandidateState::Pending
    );
}

#[tokio::test]
async fn exact_note_must_belong_to_this_bear_even_with_a_matching_conversation_uuid() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let other_bear = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let foreign = note(&other_bear).await;
    let local = note_in(&store, foreign.conversation).await;
    reject_source(&store, &alice, foreign).await;
    let candidate = stage(&store, &alice, local, "Alice").await;
    assert_eq!(candidate.source_memory_id(), local.memory);
}

#[tokio::test]
async fn only_normal_visibility_and_active_valid_lifecycle_metadata_are_admissible() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let conversation = Uuid::new_v4();
    let logical = LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note");
    for visibility in [
        NoteVisibility::Quarantined,
        NoteVisibility::Hidden,
        NoteVisibility::Private,
        NoteVisibility::Withdrawn,
    ] {
        let memory = insert_note(&store, &logical, "{}", visibility).await;
        reject_source(
            &store,
            &alice,
            Note {
                conversation,
                memory,
            },
        )
        .await;
    }
    for metadata in [
        "not-json",
        "{",
        r#"{"lifecycle":{"status":"stale"}}"#,
        r#"{"lifecycle":{"status":"archived"}}"#,
        r#"{"lifecycle":{"status":"archive-candidate"}}"#,
        r#"{"lifecycle":{"status":"superseded"}}"#,
        r#"{"lifecycle":{"status":"withdrawn"}}"#,
        r#"{"lifecycle":{"status":"unknown"}}"#,
        r#"{"lifecycle":{"status":42}}"#,
    ] {
        let memory = insert_note(&store, &logical, metadata, NoteVisibility::Normal).await;
        reject_source(
            &store,
            &alice,
            Note {
                conversation,
                memory,
            },
        )
        .await;
    }
    for metadata in ["{}", r#"{"lifecycle":{"status":"active"}}"#] {
        let memory = insert_note(&store, &logical, metadata, NoteVisibility::Normal).await;
        assert_eq!(
            stage(
                &store,
                &alice,
                Note {
                    conversation,
                    memory
                },
                "Alice"
            )
            .await
            .source_memory_id(),
            memory
        );
    }
}

async fn set_invalidated(store: &BearMemoryStore, source: Note, invalidated: bool) {
    let id = source.memory.to_string();
    let invalid_at = invalidated.then_some("2026-01-02T00:00:00Z");
    sqlx::query!(
        "UPDATE memory_records SET invalid_at = ? WHERE memory_id = ?",
        invalid_at,
        id,
    )
    .execute(store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn invalidation_and_any_superseder_reject_the_exact_old_note_without_following_a_path_head() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    set_invalidated(&store, source, true).await;
    reject_source(&store, &alice, source).await;
    let old_id = source.memory.to_string();
    let row = store
        .append_record_with_options(
            &LogicalMemoryPath::source_local(
                MemorySource::Conversation(source.conversation),
                "note",
            ),
            "note",
            "pair",
            None,
            "Superseding source note.",
            &serde_json::json!({}),
            NoteVisibility::Quarantined.as_str(),
            "normal",
            Some(&old_id),
        )
        .await
        .unwrap();
    let newer = Note {
        conversation: source.conversation,
        memory: Uuid::parse_str(&row.memory_id).unwrap(),
    };
    let newer_id = newer.memory.to_string();
    // append_record invalidates its predecessor; clear that to isolate the superseder check.
    set_invalidated(&store, source, false).await;
    // Even an inadmissible superseder with the same logical path blocks the old note.
    reject_source(&store, &alice, source).await;
    reject_source(&store, &alice, newer).await;
    sqlx::query!(
        "UPDATE memory_records SET visibility = 'normal' WHERE memory_id = ?",
        newer_id
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(
        stage(&store, &alice, newer, "Alice")
            .await
            .source_memory_id(),
        newer.memory
    );
}

#[derive(Clone, Copy)]
enum AccessRuleState {
    Active,
    Detached,
    Superseded,
    Withdrawn,
}

async fn insert_access_rule(
    store: &BearMemoryStore,
    source: Note,
    binding: &AuthenticatedUserEntity,
    state: AccessRuleState,
) {
    let state = match state {
        AccessRuleState::Active => "active",
        AccessRuleState::Detached => "detached",
        AccessRuleState::Superseded => "superseded",
        AccessRuleState::Withdrawn => "withdrawn",
    };
    let memory = source.memory.to_string();
    let bear = store.bear_id().to_string();
    let entity = binding.entity_id();
    let link = Uuid::new_v4().to_string();
    sqlx::query!(
        r"INSERT INTO memory_access_rules (link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, author_profile, state, created_at)
          VALUES (?, ?, (SELECT next_sequence FROM bear_sequence WHERE id = 1),
                  ?, ?, 'audience', 'curate', ?, '2026-01-01T00:00:00Z')",
        link,
        bear,
        memory,
        entity,
        state,
    )
    .execute(store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn all_access_rule_history_blocks_capture_even_after_detachment_or_withdrawal() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    for rule_state in [
        AccessRuleState::Active,
        AccessRuleState::Detached,
        AccessRuleState::Superseded,
        AccessRuleState::Withdrawn,
    ] {
        let source = note(&store).await;
        insert_access_rule(&store, source, &alice, rule_state).await;
        reject_source(&store, &alice, source).await;
    }
}

#[tokio::test]
async fn unrelated_rule_history_does_not_block_an_unprotected_note_and_dedup_rechecks_eligibility()
{
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let unrelated = note_in(&store, source.conversation).await;
    insert_access_rule(&store, unrelated, &alice, AccessRuleState::Withdrawn).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    reject_source(&store, &alice, unrelated).await;
    insert_access_rule(&store, source, &alice, AccessRuleState::Withdrawn).await;
    reject_source(&store, &alice, source).await;
    assert_eq!(state(&store, candidate.id()).await, CandidateState::Pending);
}

#[tokio::test]
async fn staging_neither_decodes_copies_nor_interprets_the_source_body_as_authority() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let id = source.memory.to_string();
    // A SELECT/decode of this TEXT into Rust String would fail. Admission needs only
    // source membership/lifecycle columns, never an identity claim or complete transcript.
    let unreadable_body = vec![0x80_u8, 0xff];
    sqlx::query!(
        "UPDATE memory_records SET content_text = CAST(? AS TEXT) WHERE memory_id = ?",
        unreadable_body,
        id
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert!(crate::fetch_record_by_id(&store, &id).await.is_err());
    let candidate = stage(&store, &alice, source, "Minimized value only").await;
    assert_eq!(candidate.preferred_name().as_str(), "Minimized value only");
    let bytes = sqlx::query_scalar!(
        r#"SELECT hex(content_text) AS "bytes!: String" FROM memory_records WHERE memory_id = ?"#,
        id,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(bytes, "80FF");
}
