use super::*;
use den_core::ModelAvailabilityFailureKind;
use den_http::errors::CustomError;
use den_runtime::{bearwire_events, turn_runs};
use den_service::client_sessions;

async fn pin(pool: &PgPool, fixture: &OrdinaryFixture) {
    persistence::set_conversation_model_state(
        pool,
        fixture.source.conversation.id,
        "explicit",
        Some("openai/gpt-4.1"),
        Some("openai/gpt-4.1"),
        None,
    )
    .await
    .unwrap();
}

async fn session(pool: &PgPool, fixture: &OrdinaryFixture) {
    client_sessions::upsert_session(
        pool,
        client_sessions::UpsertClientSession {
            user_id: fixture.user,
            bear_id: fixture.bear.id,
            bear_slug: fixture.bear.slug.clone(),
            client_session_id: fixture.session.clone(),
            runtime_session_id: "original-runtime".into(),
            conversation_id: fixture.external_id().into(),
            resolved_conversation_id: Some(fixture.external_id().into()),
            client: "original-client".into(),
            cwd: Some("/original".into()),
            current_mode: None,
        },
    )
    .await
    .unwrap();
}

async fn snapshot(pool: &PgPool, fixture: &OrdinaryFixture) -> serde_json::Value {
    let count = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM turn_runs WHERE session_id = $1",
        fixture.session,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    json!({
        "run_count": count,
        "active": format!("{:?}", turn_runs::active_run_for_session(pool, &fixture.session).await.unwrap()),
        "session": client_sessions::find_for_user_bear_session_id(pool, fixture.user, fixture.bear.id, &fixture.session).await.unwrap(),
        "model": format!("{:?}", persistence::get_conversation_model_state(pool, fixture.source.conversation.id).await.unwrap()),
        "messages": format!("{:?}", persistence::list_messages_page(pool, fixture.source.conversation.id, None, 100).await.unwrap()),
        "events": format!("{:?}", bearwire_events::list_bearwire_events_after(pool, &fixture.session, None, 100).await.unwrap()),
    })
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_model_availability_pin_denial_precedes_run_creation_supersession_and_writes(
    pool: PgPool,
) {
    for active in [false, true] {
        for (status, kind) in [
            (0, ModelAvailabilityFailureKind::VirtualKeyMissing),
            (401, ModelAvailabilityFailureKind::VirtualKeyRejected),
            (403, ModelAvailabilityFailureKind::VirtualKeyRejected),
            (200, ModelAvailabilityFailureKind::ModelMissing),
        ] {
            let fixture = OrdinaryFixture::new(&pool).await;
            let (state, catalog) =
                helpers::model_ready_state_for_bear(&pool, fixture.bear_id(), &["openai/gpt-4.1"])
                    .await;
            pin(&pool, &fixture).await;
            fixture.preflight(&state).await.unwrap(); // Populate a credential-matched positive cache.
            session(&pool, &fixture).await;
            if active {
                let id = format!("run_{}", Uuid::new_v4().simple());
                turn_runs::create_run(&pool, &id, &fixture.session, fixture.bear.id, fixture.user)
                    .await
                    .unwrap();
                turn_runs::transition_run(&pool, &id, turn_runs::TurnRunState::Running, None)
                    .await
                    .unwrap();
            }
            if status == 0 {
                bears_db::set_bear_bifrost_virtual_key(
                    &pool,
                    fixture.bear.id,
                    None,
                    None,
                    None,
                    &state.config.den_secret_encryption_key,
                )
                .await
                .unwrap();
            } else if status == 200 {
                catalog.set_models(&[]); // A successful live catalog must never trigger continuity.
            } else {
                catalog.set_status(status);
            }
            let before = snapshot(&pool, &fixture).await;
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", fixture.token.raw_token)
                    .parse()
                    .unwrap(),
            );
            let error = crate::methods::run::run_start_result(&state, &headers, &json!({
                "bear_slug": fixture.bear.slug, "session_id": fixture.session,
                "conversation_id": fixture.external_id(), "prompt": "Must not persist or infer",
                "supersede_active_run": true, "client": "replacement-client", "cwd": "/replacement",
            })).await.unwrap_err();
            let CustomError::ModelAvailability(failure) = error else {
                panic!("model failure lost its typed category: {error:?}")
            };
            assert_eq!(failure.kind, kind);
            assert_eq!(failure.model.as_ref().unwrap().as_str(), "openai/gpt-4.1");
            let public = failure.to_string();
            assert!(!public.contains("PRIVATE_PROVIDER_RESPONSE"));
            assert!(!public.contains("private.test"));
            assert_eq!(
                snapshot(&pool, &fixture).await,
                before,
                "model denial created/superseded a run or changed session/model/transcript/events"
            );
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearwire_model_availability_genuine_outage_retains_only_canonical_pin_and_exact_transport(
    pool: PgPool,
) {
    for cached in [false, true] {
        let fixture = OrdinaryFixture::new(&pool).await;
        let (verified, catalog) =
            helpers::model_ready_state_for_bear(&pool, fixture.bear_id(), &["openai/gpt-4.1"])
                .await;
        let state = if cached {
            verified
        } else {
            DenState::new(
                pool.clone(),
                verified.config.clone(),
                std::sync::Arc::new(den_service::bifrost::BifrostClient::new(
                    verified.config.as_ref(),
                )),
                den_memory::MemoryStoreManager::new(verified.config.as_ref()),
            )
        };
        pin(&pool, &fixture).await;
        catalog.set_status(503);
        let before = snapshot(&pool, &fixture).await;
        let retained = fixture.preflight(&state).await.unwrap();
        assert_eq!(retained.handle, "openai/gpt-4.1");
        assert_eq!(
            retained.primary_source,
            configurations::PrimaryModelSource::ConversationPin
        );
        assert_eq!(
            retained.api_style,
            if cached {
                den_llm::LlmApiStyle::ChatCompletionsStream
            } else {
                den_llm::LlmApiStyle::ResponsesStream
            }
        );
        assert_eq!(snapshot(&pool, &fixture).await, before);
        persistence::set_conversation_model_state(
            &pool,
            fixture.source.conversation.id,
            "auto",
            None,
            Some("openai/gpt-4.1"),
            None,
        )
        .await
        .unwrap();
        let error = fixture
            .preflight(&state)
            .await
            .err()
            .expect("automatic diagnostics are not pins");
        assert!(matches!(error, CustomError::ModelAvailability(ref failure)
            if failure.kind == ModelAvailabilityFailureKind::CatalogUnavailable));
    }
}
