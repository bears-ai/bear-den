//! HTTP catalog fixture exercising the production shared client and encrypted Bear keys.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use den_core::config::Config;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const ENCRYPTION_KEY: &str = "web-fixture-encryption-key-not-production";
pub(crate) const VIRTUAL_KEY: &str = "vk-web-fixture-secret-CANARY";
pub(crate) const PROVIDER_BODY: &str = "provider-password-secret-CANARY";

#[derive(Clone)]
pub(crate) struct MockBifrost {
    pub(crate) url: String,
    state: Arc<Mutex<CatalogState>>,
}

struct CatalogState {
    status: StatusCode,
    models: Vec<Value>,
    calls: usize,
    management_calls: usize,
    quota_calls: usize,
    provider_calls: usize,
    key_name: String,
    creation_pool: Option<PgPool>,
    staged_bears: Vec<Uuid>,
    preserve_staged_work: bool,
}

async fn catalog(State(state): State<Arc<Mutex<CatalogState>>>, headers: HeaderMap) -> Response {
    let mut state = state.lock().unwrap();
    state.calls += 1;
    assert!(
        !headers.contains_key("authorization"),
        "model checks must not use global auth"
    );
    if headers.get("x-bf-vk").and_then(|key| key.to_str().ok()) != Some(VIRTUAL_KEY) {
        return (StatusCode::UNAUTHORIZED, PROVIDER_BODY).into_response();
    }
    if !state.status.is_success() {
        return (state.status, PROVIDER_BODY).into_response();
    }
    Json(json!({"data": state.models, "next_page_token": null})).into_response()
}

impl MockBifrost {
    pub(crate) async fn start(models: &[&str]) -> Self {
        let state = Arc::new(Mutex::new(CatalogState {
            status: StatusCode::OK,
            models: Vec::new(),
            calls: 0,
            management_calls: 0,
            quota_calls: 0,
            provider_calls: 0,
            key_name: String::new(),
            creation_pool: None,
            staged_bears: Vec::new(),
            preserve_staged_work: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/v1/models", get(catalog))
            .route("/v1/chat/completions", post(provider))
            .route("/v1/responses", post(provider))
            .route("/api/session/login", post(management_login))
            .route("/api/governance/virtual-keys", post(create_key))
            .route("/api/governance/virtual-keys/quota", get(quota))
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let fixture = Self { url, state };
        fixture.set_models(models);
        fixture
    }

    pub(crate) async fn standard() -> Self {
        Self::start(&[
            "openai/gpt-4.1",
            "openai/gpt-5",
            "openai/gpt-5-mini",
            "openai/gpt-5-nano",
        ])
        .await
    }

    pub(crate) fn config(&self) -> Config {
        let mut config = Config::test_stub();
        config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
        config.llm_api_url = self.url.clone();
        config.den_secret_encryption_key = ENCRYPTION_KEY.into();
        // A global credential is present deliberately: Bear checks must never use it.
        config.llm_api_key = "global-api-key-secret-CANARY".into();
        config
    }

    pub(crate) fn creation_config(&self, pool: &PgPool) -> Config {
        self.state.lock().unwrap().creation_pool = Some(pool.clone());
        let mut config = self.config();
        config.bifrost_management_url = format!("{}/api", self.url.strip_suffix("/v1").unwrap());
        config.bifrost_admin_username = "web-fixture-admin".into();
        config.bifrost_admin_password = "management-password-secret-CANARY".into();
        config
    }

    pub(crate) fn setup_calls(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.management_calls,
            state.quota_calls,
            state.provider_calls,
        )
    }

    pub(crate) fn staged_bears(&self) -> Vec<Uuid> {
        self.state.lock().unwrap().staged_bears.clone()
    }

    pub(crate) fn preserve_staged_work(&self) {
        self.state.lock().unwrap().preserve_staged_work = true;
    }

    pub(crate) fn set_models(&self, models: &[&str]) {
        self.state.lock().unwrap().models = models.iter().map(|model| json!({
            "id": model, "normalized_name": "Gateway label must not replace Den metadata",
            "context_length": 128000, "max_output_tokens": 16000,
            "supported_parameters": ["tools", "reasoning_effort"], "supported_methods": ["chat_completion"],
        })).collect();
    }

    pub(crate) fn set_status(&self, status: StatusCode) {
        self.state.lock().unwrap().status = status;
    }

    pub(crate) fn calls(&self) -> usize {
        self.state.lock().unwrap().calls
    }
}

async fn management_login(State(state): State<Arc<Mutex<CatalogState>>>) -> Json<Value> {
    state.lock().unwrap().management_calls += 1;
    Json(json!({"token": "management-token-secret-CANARY"}))
}

async fn create_key(
    State(state): State<Arc<Mutex<CatalogState>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(
        headers["authorization"],
        "Bearer management-token-secret-CANARY"
    );
    let (pool, preserve) = {
        let mut state = state.lock().unwrap();
        state.management_calls += 1;
        state.key_name = body["name"].as_str().unwrap().into();
        (
            state.creation_pool.clone().unwrap(),
            state.preserve_staged_work,
        )
    };
    for bear in den_service::bears::db::list_bears(&pool).await.unwrap() {
        // Only this staged fixture is new; existing Bear rows are deliberately preserved.
        if bear.slug.starts_with("staged-model-")
            && !state.lock().unwrap().staged_bears.contains(&bear.id)
        {
            assert!(
                bear.default_model.is_none(),
                "an active default was saved before key validation"
            );
            assert!(
                den_service::bears::model_configurations::list(&pool, bear.id.into())
                    .await
                    .unwrap()
                    .is_empty()
            );
            state.lock().unwrap().staged_bears.push(bear.id);
            if preserve {
                den_service::conversation::persistence::ensure_conversation_for_external_id(
                    &pool,
                    bear.id,
                    None,
                    "conv-intervening-work",
                    None,
                    None,
                )
                .await
                .unwrap();
            }
        }
    }
    Json(
        json!({"virtual_key": {"id": "web-fixture-key", "name": body["name"], "value": VIRTUAL_KEY}}),
    )
}

async fn quota(State(state): State<Arc<Mutex<CatalogState>>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["x-bf-vk"], VIRTUAL_KEY);
    let mut state = state.lock().unwrap();
    state.quota_calls += 1;
    Json(json!({"name": state.key_name, "is_active": true}))
}

async fn provider(State(state): State<Arc<Mutex<CatalogState>>>) -> StatusCode {
    state.lock().unwrap().provider_calls += 1;
    StatusCode::INTERNAL_SERVER_ERROR
}

pub(crate) async fn seed_den_model(pool: &PgPool, handle: &str, support: Option<bool>) {
    sqlx::query!(
        "INSERT INTO model_selection_options (handle, display_name, selectable, metadata_json) VALUES ($1, 'UI test model', TRUE, $2)",
        handle,
        json!({"supports_reasoning_effort": support}),
    ).execute(pool).await.unwrap();
}

pub(crate) async fn seed_key(pool: &PgPool, bear: Uuid) {
    den_service::bears::db::set_bear_bifrost_virtual_key(
        pool,
        bear,
        Some("web-fixture-key"),
        Some("Web fixture"),
        Some(VIRTUAL_KEY),
        ENCRYPTION_KEY,
    )
    .await
    .unwrap();
}
