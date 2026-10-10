use super::access_tests::{app_with_config, bound_conversation, login, request, seed};
use super::chat_model_access_tests::{assert_json_error, raw_request};
use super::*;
use crate::{
    test_bifrost::{self, MockBifrost, PROVIDER_BODY, VIRTUAL_KEY},
    web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream},
};
use den_core::{ModelAvailabilityFailure, ModelAvailabilityFailureKind as Kind};
use den_service::bears::model_configurations as models;
use sqlx::PgPool;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const MODEL: &str = "openai/gpt-6-sol";
const EXTERNAL: &str = "conv-gateway-selection";

#[derive(Default)]
struct UncalledRuntime(AtomicUsize);
impl WebChatRuntime for UncalledRuntime {
    fn stream_chat(
        &self,
        _: &AppState,
        _: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(CustomError::System("unexpected runtime invocation".into())) })
    }
}

fn assert_safe(body: &Value) {
    let body = body.to_string();
    for private in [VIRTUAL_KEY, PROVIDER_BODY, "global-api-key-secret-CANARY"] {
        assert!(!body.contains(private), "secret escaped: {body}");
    }
}

async fn no_messages(pool: &PgPool, conversation: Uuid) {
    let count = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM conversation_messages WHERE conversation_id = $1",
        conversation,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_den_known_gateway_absent_cannot_pin_and_gateway_drift_blocks_send(
    pool: PgPool,
) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let canonical = bound_conversation(&pool, bear, owner, EXTERNAL).await;
    let configured = models::create(&pool, bear.into(), "Chosen primary", MODEL, None)
        .await
        .unwrap();
    models::set_default(&pool, bear.into(), Some(configured.id))
        .await
        .unwrap();
    let gateway = MockBifrost::start(&[MODEL, "openai/gpt-4.1"]).await;
    let runtime = Arc::new(UncalledRuntime::default());
    let app = app_with_config(&pool, runtime.clone(), gateway.config()).await;
    let cookie = login(&app, owner).await;
    let get_uri = format!("/v1/chat/model?bear_id={bear}&conversation_id={EXTERNAL}");
    let (_, preview) = request(&app, &cookie, "GET", &get_uri, Value::Null).await;
    assert_eq!(preview["effective_model"], MODEL);
    let pin = json!({"bear_id": bear, "conversation_id": EXTERNAL, "selection_mode": "explicit", "model": MODEL}).to_string();
    let (status, pinned) = request(
        &app,
        &cookie,
        "PATCH",
        "/v1/chat/model",
        serde_json::from_str(&pin).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pinned["selected_model"], MODEL);
    let stored = conversation_persistence::get_conversation_model_state(&pool, canonical)
        .await
        .unwrap()
        .unwrap();

    // A warm positive snapshot must not authorize a later write or turn.
    gateway.set_models(&["openai/gpt-4.1"]);
    let failure = assert_json_error(
        raw_request(&app, &cookie, "PATCH", "/v1/chat/model", &pin).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(failure["code"], "model_missing");
    assert_eq!(failure["model"], MODEL);
    assert!(failure["error"].as_str().unwrap().contains("Bear → Models"));
    assert_safe(&failure);
    let current = conversation_persistence::get_conversation_model_state(&pool, canonical)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.selection_mode, stored.selection_mode);
    assert_eq!(current.requested_model, stored.requested_model);
    assert_eq!(current.selected_reason, stored.selected_reason);
    assert_eq!(current.metadata_json, stored.metadata_json);
    assert_eq!(current.selected_model, stored.selected_model);
    let (_, preview) = request(&app, &cookie, "GET", &get_uri, Value::Null).await;
    assert_eq!(preview["error_code"], "model_missing");
    assert_eq!(preview["unavailable_model"], MODEL);
    assert_eq!(preview["selected_model"], MODEL);
    assert_eq!(preview["configuration_id"], Value::Null); // pin replaces the configuration
    assert!(preview["effective_model"].is_null());
    assert!(!preview["model_options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|option| option["handle"] == MODEL));
    assert!(preview["model_options"]
        .as_array()
        .unwrap()
        .iter()
        .all(|option| option["label"] != "Gateway label must not replace Den metadata"));
    assert_safe(&preview);
    let send = json!({"bear_id": bear, "conversation_id": EXTERNAL, "message": "must not persist"})
        .to_string();
    let failure = assert_json_error(
        raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(failure["code"], "model_missing");
    assert_eq!(failure["model"], MODEL);
    assert_safe(&failure);
    no_messages(&pool, canonical).await;
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);

    // Clearing the pin still succeeds while the inherited model is unavailable.
    let (status, cleared) = request(
        &app,
        &cookie,
        "PATCH",
        "/v1/chat/model",
        json!({"bear_id": bear, "conversation_id": EXTERNAL, "selection_mode": "auto"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["selection_mode"], "auto");
    assert_eq!(cleared["error_code"], "model_missing");
    assert_eq!(cleared["configuration_id"], json!(configured.id));
    assert_eq!(
        models::get(&pool, bear.into(), configured.id)
            .await
            .unwrap()
            .unwrap()
            .model_handle
            .as_str(),
        MODEL
    );
    assert_eq!(
        models::default_configuration_id(&pool, bear.into())
            .await
            .unwrap(),
        Some(configured.id)
    );
    assert!(gateway.calls() >= 6);
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_http_key_failures_outage_and_missing_key_are_not_membership_or_login_failures(
    pool: PgPool,
) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    let canonical = bound_conversation(&pool, bear, owner, EXTERNAL).await;
    let gateway = MockBifrost::standard().await;
    let runtime = Arc::new(UncalledRuntime::default());
    let app = app_with_config(&pool, runtime.clone(), gateway.config()).await;
    let cookie = login(&app, owner).await;
    let send = json!({"bear_id": bear, "conversation_id": EXTERNAL, "message": "must not persist"})
        .to_string();
    for (upstream, expected, code) in [
        (
            StatusCode::UNAUTHORIZED,
            StatusCode::CONFLICT,
            "virtual_key_rejected",
        ),
        (
            StatusCode::FORBIDDEN,
            StatusCode::CONFLICT,
            "virtual_key_rejected",
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
        ),
    ] {
        gateway.set_status(upstream);
        let failure = assert_json_error(
            raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await,
            expected,
        )
        .await;
        assert_eq!(failure["code"], code);
        assert_eq!(failure["model"], "openai/gpt-4.1");
        assert!(failure["recovery"].is_string());
        assert_safe(&failure);
        no_messages(&pool, canonical).await;
    }
    den_service::bears::db::set_bear_bifrost_virtual_key(
        &pool,
        bear,
        None,
        None,
        None,
        &crate::config::Config::test_stub().den_secret_encryption_key,
    )
    .await
    .unwrap();
    let calls = gateway.calls();
    let failure = assert_json_error(
        raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(failure["code"], "virtual_key_missing");
    assert_safe(&failure);
    assert_eq!(gateway.calls(), calls);
    no_messages(&pool, canonical).await;
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct PinAttemptRuntime(Arc<std::sync::Mutex<Vec<String>>>);
impl WebChatRuntime for PinAttemptRuntime {
    fn stream_chat(
        &self,
        state: &AppState,
        request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        let pool = state.sqlx_pool().clone();
        let default_model = state.config.default_llm_model.clone();
        let attempts = self.0.clone();
        Box::pin(async move {
            let conversation = conversation_persistence::get_conversation_for_external_id(
                &pool,
                request.bear_id,
                &request.conversation_id,
            )
            .await?
            .unwrap();
            let primary = den_service::model_selection::resolve_conversation_primary_model(
                &pool,
                request.bear_id.into(),
                conversation.id,
                &default_model,
            )
            .await?;
            assert_eq!(primary.source, models::PrimaryModelSource::ConversationPin);
            attempts.lock().unwrap().push(primary.model_handle);
            Ok(Box::pin(futures::stream::iter([
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::AssistantTextDelta {
                        text: "Same-pin attempt".into(),
                    },
                )),
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnCompleted { turn: None },
                )),
            ])) as WebChatRuntimeStream)
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_existing_canonical_pin_outage_attempts_same_model_without_rewriting_selection(
    pool: PgPool,
) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let canonical = bound_conversation(&pool, bear, owner, EXTERNAL).await;
    let gateway = MockBifrost::start(&[MODEL, "openai/gpt-4.1"]).await;
    let runtime = Arc::new(PinAttemptRuntime::default());
    let app = app_with_config(&pool, runtime.clone(), gateway.config()).await;
    let cookie = login(&app, owner).await;
    let pin = json!({"bear_id":bear,"conversation_id":EXTERNAL,"selection_mode":"explicit","model":MODEL}).to_string();
    assert_eq!(
        raw_request(&app, &cookie, "PATCH", "/v1/chat/model", &pin)
            .await
            .status(),
        StatusCode::OK
    );
    let stored = serde_json::to_value(
        conversation_persistence::get_conversation_model_state(&pool, canonical)
            .await
            .unwrap(),
    )
    .unwrap();
    gateway.set_status(StatusCode::SERVICE_UNAVAILABLE);
    let model_uri = format!("/v1/chat/model?bear_id={bear}&conversation_id={EXTERNAL}");
    let (_, preview) = request(&app, &cookie, "GET", &model_uri, Value::Null).await;
    assert_eq!(preview["selected_model"], MODEL);
    assert_eq!(preview["effective_model"], MODEL);
    assert_eq!(preview["availability"], "unverified");
    assert_eq!(preview["error_code"], "catalog_unavailable");
    assert!(preview["error"].as_str().unwrap().contains("same model"));
    assert!(preview["model_options"].as_array().unwrap().is_empty());
    assert_safe(&preview);
    let failure = assert_json_error(
        raw_request(&app, &cookie, "PATCH", "/v1/chat/model", &pin).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(failure["code"], "catalog_unavailable");
    let send =
        json!({"bear_id":bear,"conversation_id":EXTERNAL,"message":"Attempt my existing pin"})
            .to_string();
    // Warm matching catalog continuity and a cold/no-cache runtime both retain the pin.
    for app in [
        app,
        app_with_config(&pool, runtime.clone(), gateway.config()).await,
    ] {
        let cookie = login(&app, owner).await;
        let response = raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await;
        assert_eq!(response.status(), StatusCode::OK);
        use http_body_util::BodyExt;
        response.into_body().collect().await.unwrap();
        assert_eq!(
            serde_json::to_value(
                conversation_persistence::get_conversation_model_state(&pool, canonical)
                    .await
                    .unwrap()
            )
            .unwrap(),
            stored
        );
    }
    assert_eq!(*runtime.0.lock().unwrap(), [MODEL, MODEL]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_existing_pin_cannot_bypass_missing_or_rejected_key_or_live_model_denial(
    pool: PgPool,
) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let canonical = bound_conversation(&pool, bear, owner, EXTERNAL).await;
    let gateway = MockBifrost::start(&[MODEL]).await;
    let runtime = Arc::new(UncalledRuntime::default());
    let app = app_with_config(&pool, runtime.clone(), gateway.config()).await;
    let cookie = login(&app, owner).await;
    let pin = json!({"bear_id":bear,"conversation_id":EXTERNAL,"selection_mode":"explicit","model":MODEL}).to_string();
    assert_eq!(
        raw_request(&app, &cookie, "PATCH", "/v1/chat/model", &pin)
            .await
            .status(),
        StatusCode::OK
    );
    let send =
        json!({"bear_id":bear,"conversation_id":EXTERNAL,"message":"Do not fall back"}).to_string();
    for (status, models, code, expected) in [
        (
            StatusCode::OK,
            vec!["openai/gpt-4.1"],
            "model_missing",
            StatusCode::BAD_REQUEST,
        ),
        (
            StatusCode::UNAUTHORIZED,
            vec![MODEL],
            "virtual_key_rejected",
            StatusCode::CONFLICT,
        ),
    ] {
        gateway.set_status(status);
        gateway.set_models(&models);
        let body = assert_json_error(
            raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await,
            expected,
        )
        .await;
        assert_eq!(body["code"], code);
        assert_safe(&body);
    }
    den_service::bears::db::set_bear_bifrost_virtual_key(
        &pool,
        bear,
        None,
        None,
        None,
        &gateway.config().den_secret_encryption_key,
    )
    .await
    .unwrap();
    gateway.set_status(StatusCode::SERVICE_UNAVAILABLE);
    let body = assert_json_error(
        raw_request(&app, &cookie, "POST", "/v1/chat/send", &send).await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(body["code"], "virtual_key_missing");
    assert_eq!(
        conversation_persistence::get_conversation_model_state(&pool, canonical)
            .await
            .unwrap()
            .unwrap()
            .selected_model
            .as_deref(),
        Some(MODEL)
    );
    no_messages(&pool, canonical).await;
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn model_availability_json_descriptors_have_safe_models_recovery_and_exact_request_references(
) {
    for (kind, status, code) in [
        (Kind::ModelMissing, StatusCode::BAD_REQUEST, "model_missing"),
        (
            Kind::ModelUnavailable,
            StatusCode::BAD_REQUEST,
            "model_unavailable",
        ),
        (
            Kind::VirtualKeyMissing,
            StatusCode::CONFLICT,
            "virtual_key_missing",
        ),
        (
            Kind::VirtualKeyRejected,
            StatusCode::CONFLICT,
            "virtual_key_rejected",
        ),
        (
            Kind::CatalogUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
        ),
    ] {
        let reference = Uuid::new_v4();
        let failure = ModelAvailabilityFailure::new(kind, Some(MODEL));
        let body = assert_json_error(
            ChatApiError::from(CustomError::ModelAvailability(failure)).response(reference),
            status,
        )
        .await;
        assert_eq!(body["code"], code);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["request_id"], reference.to_string());
        assert!(body["recovery"].is_string());
        assert_safe(&body);
        let failure = ModelAvailabilityFailure::new(
            kind,
            Some("https://user:password@gateway?key=provider-password-secret-CANARY"),
        );
        let body = assert_json_error(
            ChatApiError::from(DenError::ModelAvailability(failure)).into_response(),
            status,
        )
        .await;
        assert!(body.get("model").is_none());
        assert_safe(&body);
    }
}
