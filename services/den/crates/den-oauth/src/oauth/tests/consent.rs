use super::*;
use axum_login::tower_sessions::MemoryStore;
use std::sync::Arc;

fn request() -> AuthorizationRequestWithPKCE {
    AuthorizationRequestWithPKCE {
        response_type: "code".into(),
        client_id: "native".into(),
        redirect_uri: "com.bears:/callback".into(),
        scope: Some("profile:read".into()),
        state: Some("state".into()),
        code_challenge: Some("challenge".into()),
        code_challenge_method: Some("S256".into()),
    }
}

#[tokio::test]
async fn consent_requires_the_stored_session_actor_and_exact_displayed_request() {
    let store = Arc::new(MemoryStore::default());
    let session = Session::new(None, store.clone(), None);
    let request = request();
    let token = issue(&session, 1, &request).await.unwrap();
    assert!(consume(&session, 1, None, &request).await.is_err());
    assert!(consume(&session, 1, Some("invented"), &request)
        .await
        .is_err());
    assert!(consume(&session, 2, Some(&token), &request).await.is_err());
    let other_session = Session::new(None, store.clone(), None);
    assert!(consume(&other_session, 1, Some(&token), &request)
        .await
        .is_err());
    for change in 0..5 {
        let mut changed = request.clone();
        match change {
            0 => changed.scope = Some("data:write".into()),
            1 => changed.redirect_uri = "com.attacker:/callback".into(),
            2 => changed.code_challenge = Some("replacement".into()),
            3 => changed.state = Some("replacement".into()),
            _ => changed.client_id = "another-client".into(),
        }
        assert!(consume(&session, 1, Some(&token), &changed).await.is_err());
    }
    let reloaded = Session::new(session.id(), store, None);
    consume(&reloaded, 1, Some(&token), &request).await.unwrap();
    assert!(consume(&reloaded, 1, Some(&token), &request).await.is_err());
}

#[tokio::test]
async fn expired_consent_is_refused_and_pending_pages_are_bounded() {
    let session = Session::new(None, Arc::new(MemoryStore::default()), None);
    let request = request();
    let token = issue(&session, 1, &request).await.unwrap();
    let mut entries: Vec<PendingConsent> = session.get(SESSION_KEY).await.unwrap().unwrap();
    entries[0].issued_at -= LIFETIME_SECONDS;
    session.insert(SESSION_KEY, entries).await.unwrap();
    assert!(consume(&session, 1, Some(&token), &request).await.is_err());
    for _ in 0..MAX_PENDING + 2 {
        issue(&session, 1, &request).await.unwrap();
    }
    assert_eq!(
        session
            .get::<Vec<PendingConsent>>(SESSION_KEY)
            .await
            .unwrap()
            .unwrap()
            .len(),
        MAX_PENDING
    );
}
