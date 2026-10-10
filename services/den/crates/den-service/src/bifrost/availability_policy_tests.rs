use super::availability_tests::{assert_failure, bear, key, Mock, LIVE, PRIVATE};
use super::*;
use crate::bears::model_configurations::PrimaryModelSource;
use den_core::ModelAvailabilityFailureKind as Kind;
use sqlx::PgPool;

const MODEL: &str = "openai/gpt-6-sol";
const SOURCES: [PrimaryModelSource; 4] = [
    PrimaryModelSource::ConversationPin,
    PrimaryModelSource::HatOverride,
    PrimaryModelSource::BearDefault,
    PrimaryModelSource::DeploymentDefault,
];

#[sqlx::test(migrations = "../../migrations")]
async fn execution_always_refreshes_and_successful_absence_never_uses_pin_continuity(pool: PgPool) {
    let mock = Mock::new(200, LIVE);
    let config = mock.config();
    let client = BifrostClient::new(&config);
    let bear_id = bear(&pool, "execution-fresh").await;
    key(&pool, bear_id, "sk-bf-secret", &config).await;
    for source in SOURCES {
        mock.respond(200, LIVE);
        let entry = client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                MODEL,
                &config.den_secret_encryption_key,
                source,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(entry.available);
        mock.respond(200, r#"{"data":[]}"#);
        assert_failure(
            client
                .validate_bear_model_execution(
                    &pool,
                    bear_id,
                    MODEL,
                    &config.den_secret_encryption_key,
                    source,
                )
                .await,
            Kind::ModelMissing,
        );
    }
    assert_eq!(
        mock.count(),
        8,
        "all new-turn sources require fresh authentication"
    );
    mock.respond(503, PRIVATE);
    assert_failure(
        client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                MODEL,
                &config.den_secret_encryption_key,
                PrimaryModelSource::ConversationPin,
            )
            .await,
        Kind::ModelMissing,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_pinned_execution_can_continue_a_genuine_same_credential_outage(pool: PgPool) {
    let mock = Mock::new(200, LIVE);
    let config = mock.config();
    let client = BifrostClient::new(&config);
    let bear_id = bear(&pool, "execution-outage").await;
    key(&pool, bear_id, "sk-bf-secret", &config).await;
    client
        .refresh_bear_catalog_snapshot(&pool, bear_id, &config.den_secret_encryption_key)
        .await
        .unwrap();
    mock.respond(503, PRIVATE);
    let cached = client
        .validate_bear_model_execution(
            &pool,
            bear_id,
            MODEL,
            &config.den_secret_encryption_key,
            PrimaryModelSource::ConversationPin,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(cached.available);
    assert_eq!(cached.supports_responses_api, Some(true));
    let uncached = BifrostClient::new(&config);
    assert!(uncached
        .validate_bear_model_execution(
            &pool,
            bear_id,
            MODEL,
            &config.den_secret_encryption_key,
            PrimaryModelSource::ConversationPin
        )
        .await
        .unwrap()
        .is_none());
    assert_failure(
        client
            .validate_bear_model_selection(&pool, bear_id, MODEL, &config.den_secret_encryption_key)
            .await,
        Kind::CatalogUnavailable,
    );
    assert_failure(
        client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                MODEL,
                &config.den_secret_encryption_key,
                PrimaryModelSource::DeploymentDefault,
            )
            .await,
        Kind::CatalogUnavailable,
    );
    assert_eq!(mock.count(), 17);
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_execution_source_can_bypass_missing_or_rejected_keys(pool: PgPool) {
    let mock = Mock::new(200, LIVE);
    let config = mock.config();
    let client = BifrostClient::new(&config);
    let bear_id = bear(&pool, "execution-auth").await;
    for source in SOURCES {
        assert_failure(
            client
                .validate_bear_model_execution(
                    &pool,
                    bear_id,
                    MODEL,
                    &config.den_secret_encryption_key,
                    source,
                )
                .await,
            Kind::VirtualKeyMissing,
        );
    }
    assert_eq!(mock.count(), 0);
    key(&pool, bear_id, "sk-bf-secret", &config).await;
    for status in [401, 403] {
        for source in SOURCES {
            mock.respond(200, LIVE);
            client
                .refresh_bear_catalog_snapshot(&pool, bear_id, &config.den_secret_encryption_key)
                .await
                .unwrap();
            mock.respond(status, PRIVATE);
            assert_failure(
                client
                    .validate_bear_model_execution(
                        &pool,
                        bear_id,
                        MODEL,
                        &config.den_secret_encryption_key,
                        source,
                    )
                    .await,
                Kind::VirtualKeyRejected,
            );
            assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
        }
    }
    assert_eq!(mock.count(), 16);
}
