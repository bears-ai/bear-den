use super::super::session_review::{replacement_consent, ReviewScope};
use super::*;

#[test]
fn prepare_csrf_is_random_and_scoped_to_actor_bear_session_and_age() {
    let bear = BearId::new(Uuid::new_v4());
    let actor = UserId::new(7);
    let scope = ReviewScope::new(bear, actor, "signed-in-session".into());
    let token = PrepareToken::new(scope.clone());
    let other = PrepareToken::new(scope.clone());
    assert_ne!(token.nonce, other.nonce);
    assert_eq!(token.nonce.0.get_version(), Some(uuid::Version::Random));
    assert!(token
        .authorize(token.nonce, bear, actor, "signed-in-session")
        .is_ok());
    assert!(token
        .authorize(other.nonce, bear, actor, "signed-in-session")
        .is_err());
    assert!(token
        .authorize(
            token.nonce,
            BearId::new(Uuid::new_v4()),
            actor,
            "signed-in-session"
        )
        .is_err());
    assert!(token
        .authorize(token.nonce, bear, UserId::new(8), "signed-in-session")
        .is_err());
    assert!(token
        .authorize(token.nonce, bear, actor, "another-session")
        .is_err());
    let issued = super::super::session_review::now();
    assert!(scope
        .authorize(bear, actor, "signed-in-session", issued + 901)
        .is_err());
    assert!(scope
        .authorize(bear, actor, "signed-in-session", issued - 5)
        .is_err());
    assert!(matches!(
        replacement_consent(false, false).unwrap(),
        core::ReplacementConsent::NoReplacement
    ));
    assert!(replacement_consent(true, false).is_err());
    assert!(matches!(
        replacement_consent(true, true).unwrap(),
        core::ReplacementConsent::PreserveDisplacedIdentity
    ));
}

#[tokio::test]
async fn typed_core_review_stays_server_side_and_nonce_expiry_is_enforced() {
    let store = den_memory::test_support::new_test_store().await;
    let imported = den_memory::resolve_authenticated_user_entity(&store, UserId::new(42))
        .await
        .unwrap();
    den_memory::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let actor = UserId::new(7);
    let bear = BearId::new(store.bear_id());
    let expected = core::review_reconnection(&store, actor, imported.entity_id(), actor)
        .await
        .unwrap();
    let scope = ReviewScope::new(bear, actor, "signed-in-session".into());
    let label = super::super::authority::AccountLabel {
        user_id: actor,
        username: "actor".into(),
        display_name: "Actor".into(),
    };
    let metadata = super::super::presentation::ReviewedMetadata::capture(
        label.clone(),
        Some(label),
        imported.entity_id(),
        &core::list_identities(&store).await.unwrap(),
    )
    .unwrap();
    let pending = PendingReview::new(scope, expected, metadata);
    let json = serde_json::to_value(&pending).unwrap();
    let restored: PendingReview = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(restored.expected.id(), pending.expected.id());
    assert!(restored
        .authorize(pending.nonce, bear, actor, "signed-in-session")
        .is_ok());
    assert!(restored
        .authorize(ReviewNonce::new(), bear, actor, "signed-in-session")
        .is_err());
    assert!(restored.consent(ReviewNonce::new(), true, false).is_err());
    assert!(restored
        .consent(pending.confirmation_token, false, false)
        .is_err());
    let mut expired = json;
    expired["scope"]["issued_at"] = json!(super::super::session_review::now() - 901);
    let expired: PendingReview = serde_json::from_value(expired).unwrap();
    assert!(expired
        .authorize(pending.nonce, bear, actor, "signed-in-session")
        .is_err());
}
