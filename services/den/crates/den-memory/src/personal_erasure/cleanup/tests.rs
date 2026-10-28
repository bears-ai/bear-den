use super::*;
use crate::{
    personal_candidates::capture_setting, personal_erasure::erase_preferred_name_memory,
    resolve_authenticated_user_entity, test_support::new_test_store,
};
use den_core::ids::UserId;

#[tokio::test]
async fn recorded_source_host_is_not_a_destination_cleanup_grant_even_if_owner_scope_matches() {
    let source = new_test_store().await;
    let owner = resolve_authenticated_user_entity(&source, UserId::new(11))
        .await
        .unwrap();
    let revision = capture_setting(&source, &owner).await.unwrap().revision;
    let receipt =
        erase_preferred_name_memory(&source, &owner, Uuid::new_v4(), None, None, revision)
            .await
            .unwrap();
    let destination = BearMemoryStore::new(Uuid::new_v4(), source.pool().clone());
    let mut tx = source.pool().begin().await.unwrap();
    // Isolate the SQL recorded-host predicate: ownership deliberately matches the stored
    // row, just as it would after rebasing; the processing host is the destination. This
    // does not simulate an importer or bypass the public APIs' current-binding checks.
    assert!(pending_inside(&mut tx, &destination, &owner, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        mark_inside(&mut tx, &destination, &owner, receipt.erasure_id).await,
        Err(DenError::Authorization(_))
    ));
    tx.rollback().await.unwrap();
    assert_eq!(
        pending_erasure_cleanups(&source, &owner, 100)
            .await
            .unwrap(),
        vec![receipt]
    );
}
