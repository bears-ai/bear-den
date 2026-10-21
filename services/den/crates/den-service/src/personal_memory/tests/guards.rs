use super::support::{assert_denied, conversation, new_bear, Fixture};
use crate::{
    bears::{db, hats},
    personal_memory as service,
};
use den_core::ids::{HatId, UserId};
use den_memory::{
    append_memory_record, append_relation, fetch_record_by_id, list_memory_proposals,
    LogicalMemoryPath, MemorySource,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn stages_only_for_the_canonical_owner_without_hat_sharing_or_publication(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let store = fixture.store().await;
    let before = fetch_record_by_id(&store, &fixture.source.to_string())
        .await
        .unwrap()
        .unwrap();
    let candidate = fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .unwrap();
    let binding = fixture.binding(fixture.owner).await;
    assert_eq!(candidate.person_entity_id(), binding.entity_id());
    assert_eq!(candidate.source_conversation_id(), fixture.conversation);
    assert_eq!(candidate.source_memory_id(), fixture.source);
    let after = fetch_record_by_id(&store, &fixture.source.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.content_text, before.content_text);
    assert_eq!(after.metadata_json, before.metadata_json);
    assert_eq!(after.sequence_no, before.sequence_no);
    assert!(list_memory_proposals(&store, None, 20)
        .await
        .unwrap()
        .is_empty());
    let shared = den_memory::library::recent(
        &store,
        &den_memory::library::CuratedMemoryGrant::new(vec![fixture.hat]),
        20,
    )
    .await
    .unwrap();
    assert!(
        shared.is_empty(),
        "staging must not publish hat/core memory"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn capture_defaults_off_and_the_service_setting_is_person_scoped(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    assert!(fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .is_err());
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.other,
        true,
    )
    .await
    .unwrap();
    assert!(fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .is_err());
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        true,
    )
    .await
    .unwrap();
    fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .unwrap();
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        false,
    )
    .await
    .unwrap();
    assert!(fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_admin_outsider_and_unknown_principals_cannot_claim_the_source(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    fixture.enable(fixture.other).await;
    for user in [
        fixture.other,
        fixture.outsider,
        UserId::new(2_000_000_000),
        UserId::new(0),
    ] {
        assert_denied(
            fixture
                .stage_as(user, fixture.conversation, fixture.source)
                .await,
        );
    }
    for user in [
        fixture.outsider,
        UserId::new(2_000_000_000),
        UserId::new(-1),
    ] {
        assert_denied(
            service::set_capture_enabled(&fixture.pool, &fixture.stores, fixture.bear, user, true)
                .await,
        );
    }
    let other_conversation = conversation(
        &fixture.pool,
        fixture.bear,
        Some(fixture.other),
        Some(fixture.hat),
    )
    .await;
    let store = fixture.store().await;
    let other_source = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(other_conversation), "other"),
        "note",
        "pair",
        None,
        "Different human's evidence",
        &json!({}),
    )
    .await
    .unwrap();
    let other_candidate = fixture
        .stage_as(
            fixture.other,
            other_conversation,
            Uuid::parse_str(&other_source.memory_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        other_candidate.person_entity_id(),
        fixture.binding(fixture.other).await.entity_id()
    );
    assert_ne!(
        other_candidate.person_entity_id(),
        fixture.binding(fixture.owner).await.entity_id()
    );
    assert_denied(
        fixture
            .stage_as(fixture.owner, other_conversation, fixture.source)
            .await,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_membership_is_required_for_staging_and_capture_changes(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    db::revoke_membership(&fixture.pool, fixture.owner.get(), fixture.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, fixture.source)
            .await,
    );
    assert_denied(
        service::set_capture_enabled(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            true,
        )
        .await,
    );
    db::grant_membership(
        &fixture.pool,
        fixture.owner.get(),
        fixture.bear.as_uuid(),
        None,
    )
    .await
    .unwrap();
    fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_must_be_active_owned_and_hat_bound(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, Uuid::new_v4(), fixture.source)
            .await,
    );
    for status in ["archived", "deleted", "migrated"] {
        fixture
            .set_conversation_state(status, Some(fixture.owner), Some(fixture.hat))
            .await;
        assert_denied(
            fixture
                .stage_as(fixture.owner, fixture.conversation, fixture.source)
                .await,
        );
    }
    fixture
        .set_conversation_state("active", Some(fixture.owner), None)
        .await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, fixture.source)
            .await,
    );
    fixture
        .set_conversation_state("active", None, Some(fixture.hat))
        .await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, fixture.source)
            .await,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_hat_locator_must_resolve_to_a_real_hat_of_this_bear(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let foreign_bear = new_bear(&fixture.pool, "personal-foreign").await;
    db::grant_membership(
        &fixture.pool,
        fixture.owner.get(),
        foreign_bear.as_uuid(),
        Some(db::BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    assert_denied(
        service::stage_preferred_name(
            &fixture.pool,
            &fixture.stores,
            foreign_bear,
            fixture.owner,
            fixture.conversation,
            fixture.source,
            service::PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    let foreign_hat = hats::create_hat(
        &fixture.pool,
        foreign_bear,
        fixture.owner,
        "Foreign",
        "Different Bear",
    )
    .await
    .unwrap();
    // Deliberately model corrupt/legacy rows: authorization must not depend on
    // the composite FK being present to validate a non-null hat locator.
    sqlx::query!("ALTER TABLE conversations DROP CONSTRAINT conversations_bear_hat_fkey")
        .execute(&fixture.pool)
        .await
        .unwrap();
    for hat in [HatId::new(Uuid::new_v4()), foreign_hat.id] {
        fixture
            .set_conversation_state("active", Some(fixture.owner), Some(hat))
            .await;
        assert_denied(
            fixture
                .stage_as(fixture.owner, fixture.conversation, fixture.source)
                .await,
        );
    }
    let foreign_conversation = conversation(
        &fixture.pool,
        foreign_bear,
        Some(fixture.owner),
        Some(foreign_hat.id),
    )
    .await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, foreign_conversation, fixture.source)
            .await,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn sqlite_source_identity_and_scope_override_claimed_ids_and_metadata(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let store = fixture.store().await;
    let another = conversation(
        &fixture.pool,
        fixture.bear,
        Some(fixture.owner),
        Some(fixture.hat),
    )
    .await;
    let paths = [
        LogicalMemoryPath::source_local(MemorySource::Conversation(another), "wrong-conversation"),
        LogicalMemoryPath::source_local(MemorySource::WorkRun(fixture.conversation), "work"),
        LogicalMemoryPath::source_local(MemorySource::Intake(fixture.conversation), "intake"),
        LogicalMemoryPath::hat(fixture.hat, "shared-hat"),
        LogicalMemoryPath::shared_core("shared-core"),
        LogicalMemoryPath::profile_local("pair", "legacy"),
    ];
    let binding = fixture.binding(fixture.owner).await;
    for path in paths {
        let note = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "Not this conversation's eligible evidence",
            &json!({
                "conversation_id": fixture.conversation,
                "source_memory_id": fixture.source,
                "user_id": fixture.owner.get(),
                "person_entity_id": binding.entity_id(),
            }),
        )
        .await
        .unwrap();
        assert!(fixture
            .stage_as(
                fixture.owner,
                fixture.conversation,
                Uuid::parse_str(&note.memory_id).unwrap(),
            )
            .await
            .is_err());
    }
    assert!(fixture
        .stage_as(fixture.owner, fixture.conversation, Uuid::new_v4())
        .await
        .is_err());
    let foreign = new_bear(&fixture.pool, "personal-source-other-bear").await;
    let foreign_store = fixture
        .stores
        .store_for_bear(foreign.as_uuid())
        .await
        .unwrap();
    let foreign_note = append_memory_record(
        &foreign_store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(fixture.conversation), "note"),
        "note",
        "pair",
        None,
        "Same source locator in another Bear store",
        &json!({}),
    )
    .await
    .unwrap();
    assert!(fixture
        .stage_as(
            fixture.owner,
            fixture.conversation,
            Uuid::parse_str(&foreign_note.memory_id).unwrap(),
        )
        .await
        .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn sqlite_rejects_hidden_archived_superseded_and_access_bearing_sources(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let store = fixture.store().await;
    for (kind, metadata, visibility) in [
        ("hidden", json!({}), "hidden"),
        ("stale", json!({"lifecycle": {"status": "stale"}}), "normal"),
        (
            "superseded",
            json!({"lifecycle": {"status": "superseded"}}),
            "normal",
        ),
        (
            "archived",
            json!({"lifecycle": {"status": "archived"}}),
            "normal",
        ),
        (
            "archive-candidate",
            json!({"lifecycle": {"status": "archive-candidate"}}),
            "normal",
        ),
    ] {
        let note = store
            .append_record(
                &LogicalMemoryPath::source_local(
                    MemorySource::Conversation(fixture.conversation),
                    kind,
                ),
                "note",
                "pair",
                None,
                "Ineligible source evidence",
                &metadata,
                visibility,
            )
            .await
            .unwrap();
        assert!(fixture
            .stage_as(
                fixture.owner,
                fixture.conversation,
                Uuid::parse_str(&note.memory_id).unwrap(),
            )
            .await
            .is_err());
    }
    store
        .append_record_with_options(
            &LogicalMemoryPath::source_local(
                MemorySource::Conversation(fixture.conversation),
                "note",
            ),
            "note",
            "pair",
            None,
            "Replacement evidence",
            &json!({}),
            "normal",
            "normal",
            Some(&fixture.source.to_string()),
        )
        .await
        .unwrap();
    assert!(fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .is_err());
    let note = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(fixture.conversation), "gated"),
        "note",
        "pair",
        None,
        "Access-bearing evidence needs a different future lane",
        &json!({}),
    )
    .await
    .unwrap();
    let binding = fixture.binding(fixture.owner).await;
    append_relation(
        &store,
        &note.memory_id,
        binding.entity_id(),
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    assert!(fixture
        .stage_as(
            fixture.owner,
            fixture.conversation,
            Uuid::parse_str(&note.memory_id).unwrap(),
        )
        .await
        .is_err());
}
