use super::support::{assert_denied, Fixture};
use crate::{
    conversation::{
        dependencies::ContextDependencies, events::CanonicalConversationRecord, persistence,
    },
    personal_memory as service,
};
use den_core::ids::UserId;
use den_memory::{personal_candidates, personal_erasure};
use service::{OwnerMemoryAction, PreferredName};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

async fn publish(f: &Fixture, user: UserId, name: &str) -> service::PublicationReceipt {
    f.enable(user).await;
    let candidate = service::stage_owner_preferred_name(
        &f.pool,
        &f.stores,
        f.bear,
        user,
        Uuid::new_v4(),
        PreferredName::new(name).unwrap(),
    )
    .await
    .unwrap();
    service::publish_preferred_name(&f.pool, &f.stores, f.bear, user, candidate.id(), None)
        .await
        .unwrap()
}

async fn message(
    f: &Fixture,
    conversation: Uuid,
    text: &str,
    dependencies: &ContextDependencies,
    human: bool,
) {
    let record = if human {
        CanonicalConversationRecord::visible_user_message(text, serde_json::Value::Null, None)
    } else {
        CanonicalConversationRecord::visible_assistant_message(text, serde_json::Value::Null, None)
    };
    let record = record.with_context_dependencies(dependencies).unwrap();
    persistence::append_message(&f.pool, conversation, &record.to_write(None))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_erasure_cleans_marked_derived_messages_but_preserves_original_human_history_and_bob(
    pool: PgPool,
) {
    let f = Fixture::new(pool).await;
    let alice = publish(&f, f.owner, "Alice erasure canary").await;
    let bob = publish(&f, f.other, "Bob must remain").await;
    let alice_dep = ContextDependencies::personal_fact(f.bear, alice.fact_id);
    let bob_dep = ContextDependencies::personal_fact(f.bear, bob.fact_id);
    message(
        &f,
        f.conversation,
        "Original human history remains",
        &ContextDependencies::default(),
        true,
    )
    .await;
    message(
        &f,
        f.conversation,
        "Alice erasure canary",
        &alice_dep,
        false,
    )
    .await;
    message(&f, f.conversation, "Bob must remain", &bob_dep, false).await;
    let reviewed = service::inspect_personal_memory(&f.pool, &f.stores, f.bear, f.owner)
        .await
        .unwrap();
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(f.pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service::apply_owner_memory_action(
            &single,
            &f.stores,
            f.bear,
            f.owner,
            &reviewed.owner,
            OwnerMemoryAction::ErasePreferredNames {
                request_id: Uuid::new_v4(),
            },
        ),
    )
    .await
    .expect("one-connection erasure must not borrow a second connection")
    .unwrap();
    let rows = persistence::list_messages_page(&f.pool, f.conversation, None, 100)
        .await
        .unwrap();
    assert!(rows
        .iter()
        .any(|row| row.content_text == "Original human history remains"));
    assert!(rows.iter().any(|row| row.content_text == "Bob must remain"));
    let erased = rows
        .iter()
        .find(|row| row.visibility == "diagnostic_only")
        .unwrap();
    assert!(erased.content_text.is_empty());
    assert!(erased.require_unrestricted_model_reuse().is_err());
    assert!(!erased
        .content_json
        .to_string()
        .contains("Alice erasure canary"));
    let current = service::inspect_personal_memory(&f.pool, &f.stores, f.bear, f.owner)
        .await
        .unwrap();
    assert!(!current.capture_enabled);
    assert!(!current.erasure_cleanup_pending);
    assert!(current
        .candidates
        .iter()
        .all(|candidate| candidate.preferred_name().is_none()));
    assert!(current.current.unwrap().preferred_name.is_none());
    assert_eq!(
        service::current_preferred_name(&f.pool, &f.stores, f.bear, f.other)
            .await
            .unwrap()
            .unwrap()
            .preferred_name
            .unwrap()
            .as_str(),
        "Bob must remain"
    );
    single.close().await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn rolled_back_pg_cleanup_remains_pending_and_owner_inspection_retries_it(pool: PgPool) {
    let f = Fixture::new(pool).await;
    let published = publish(&f, f.owner, "Retry erasure canary").await;
    message(
        &f,
        f.conversation,
        "Retry erasure canary",
        &ContextDependencies::personal_fact(f.bear, published.fact_id),
        false,
    )
    .await;
    let store = f.store().await;
    let binding = f.binding(f.owner).await;
    let candidate = personal_candidates::list_candidates(&store, &binding, 1)
        .await
        .unwrap()[0]
        .id();
    let capture = personal_candidates::capture_setting(&store, &binding)
        .await
        .unwrap();
    let receipt = personal_erasure::erase_preferred_name_memory(
        &store,
        &binding,
        Uuid::new_v4(),
        Some(published.fact_id),
        Some(candidate),
        capture.revision,
    )
    .await
    .unwrap();
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(f.pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let mut locked = service::authority::lock_member(&single, f.bear, f.owner)
        .await
        .unwrap();
    let prepared = service::erasure::prepare_pending(&mut locked, &store, &binding)
        .await
        .unwrap();
    assert_eq!(prepared[0].erasure_id, receipt.erasure_id);
    drop(locked); // Simulate interruption before the PG commit and SQLite acknowledgement.
    let before_retry = persistence::list_messages_page(&f.pool, f.conversation, None, 100)
        .await
        .unwrap();
    assert_eq!(before_retry[0].content_text, "Retry erasure canary");
    assert_eq!(
        personal_erasure::pending_erasure_cleanups(&store, &binding, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let inspected = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service::inspect_personal_memory(&single, &f.stores, f.bear, f.owner),
    )
    .await
    .expect("one-connection retry must finish")
    .unwrap();
    assert!(!inspected.erasure_cleanup_pending);
    assert!(
        personal_erasure::pending_erasure_cleanups(&store, &binding, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let rows = persistence::list_messages_page(&f.pool, f.conversation, None, 100)
        .await
        .unwrap();
    assert!(rows
        .iter()
        .all(|row| !row.content_text.contains("Retry erasure canary")));
    single.close().await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn stale_erasure_review_cannot_delete_a_newly_staged_request(pool: PgPool) {
    let f = Fixture::new(pool).await;
    publish(&f, f.owner, "Existing name").await;
    let old = service::inspect_personal_memory(&f.pool, &f.stores, f.bear, f.owner)
        .await
        .unwrap();
    service::stage_owner_preferred_name(
        &f.pool,
        &f.stores,
        f.bear,
        f.owner,
        Uuid::new_v4(),
        PreferredName::new("New request").unwrap(),
    )
    .await
    .unwrap();
    assert_denied(
        service::apply_owner_memory_action(
            &f.pool,
            &f.stores,
            f.bear,
            f.owner,
            &old.owner,
            OwnerMemoryAction::ErasePreferredNames {
                request_id: Uuid::new_v4(),
            },
        )
        .await,
    );
    let current = service::inspect_personal_memory(&f.pool, &f.stores, f.bear, f.owner)
        .await
        .unwrap();
    assert!(current.capture_enabled);
    assert_eq!(
        current.candidates[0].preferred_name().unwrap().as_str(),
        "New request"
    );
    assert_eq!(
        current.current.unwrap().preferred_name.unwrap().as_str(),
        "Existing name"
    );
}
