use super::{
    conversations::{self, Create},
    history,
};
use den_service::conversation::persistence::PersistedConversationMessage;
use serde_json::json;
use uuid::Uuid;

#[test]
fn create_command_identity_is_owner_bear_and_request_scoped_not_hat_scoped() {
    let mut request = Create {
        bear_id: Uuid::new_v4(),
        hat_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
    };
    let id = conversations::external_id(1, &request);
    assert_eq!(id, conversations::external_id(1, &request));
    request.hat_id = Uuid::new_v4();
    assert_eq!(
        id,
        conversations::external_id(1, &request),
        "a changed hat must conflict against the same command"
    );
    assert_ne!(id, conversations::external_id(2, &request));
    request.bear_id = Uuid::new_v4();
    assert_ne!(id, conversations::external_id(1, &request));
}

#[test]
fn history_query_bounds_are_explicit() {
    assert_eq!(history::bounds(None, None).unwrap(), 50);
    assert!(history::bounds(Some(0), Some(10)).is_err());
    assert!(history::bounds(None, Some(101)).is_err());
    assert!(history::bounds(None, Some(-1)).is_err());
    assert!(history::bounds(Some(10), Some(100)).is_ok());
}

pub(super) fn message(sequence: i64, role: &str, text: &str) -> PersistedConversationMessage {
    PersistedConversationMessage {
        sequence_no: sequence,
        message_type: role.into(),
        role: Some(role.into()),
        visibility: "default".into(),
        content_text: text.into(),
        content_json: json!({}),
        provider_message_id: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn canonical_history_identity_is_stable_and_source_scoped() {
    let conversation = Uuid::new_v4();
    let row = message(7, "user", "hello");
    let first = history::project(conversation, &row).unwrap();
    assert_eq!(first.id, format!("{conversation}:7"));
    assert_eq!(first.id, history::project(conversation, &row).unwrap().id);
    assert_ne!(first.id, history::project(Uuid::new_v4(), &row).unwrap().id);
    assert_eq!(first.role, "user");
    let mut hidden = row;
    hidden.visibility = "diagnostic_only".into();
    assert!(history::project(conversation, &hidden).is_none());
}

#[tokio::test]
async fn missing_authentication_is_rejected_before_database_access() {
    let config = std::sync::Arc::new(den_core::config::Config::test_stub());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let state = den_service::DenState::new(
        pool,
        config.clone(),
        std::sync::Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        den_memory::MemoryStoreManager::new(&config),
    );
    let result = super::access::authenticate(
        &state,
        &axum::http::HeaderMap::new(),
        den_oauth::oauth::OAuthScope::DataRead,
    )
    .await;
    assert_eq!(
        result.unwrap_err().status,
        axum::http::StatusCode::UNAUTHORIZED
    );
}
