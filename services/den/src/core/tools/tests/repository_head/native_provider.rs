//! Loopback provider and gateway fixtures. Secrets exist only in the injected
//! resolver and authenticated provider boundary, never in model scripts.
use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use den_core::tools::repository::{RepositoryError, RepositorySurfaceId};
use den_repository::{CredentialLease, CredentialRequest, ExternalCredentialResolver};
use secrecy::SecretString;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub(super) const CANARY: &str = "ghp_NATIVE_RUNTIME_CREDENTIAL_CANARY";
pub(super) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
pub(super) const CALL: &str = "repository-native-call";

pub(super) struct Resolver {
    pub expected: CredentialRequest,
    pub calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ExternalCredentialResolver for Resolver {
    async fn resolve(
        &self,
        request: &CredentialRequest,
    ) -> Result<CredentialLease, RepositoryError> {
        if request != &self.expected {
            return Err(RepositoryError::CredentialScopeMismatch);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        CredentialLease::new(
            request.clone(),
            SecretString::from(CANARY.to_owned()),
            Duration::from_secs(30),
        )
    }
    async fn validate(&self, lease: &CredentialLease) -> Result<(), RepositoryError> {
        lease.check(&self.expected)
    }
}

#[derive(Clone)]
pub(super) struct Provider {
    pub authenticated: Arc<AtomicBool>,
    pub calls: Arc<AtomicUsize>,
}
async fn repository(State(state): State<Provider>, headers: HeaderMap) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let valid = headers
        .get("authorization")
        .is_some_and(|header| header == format!("Bearer {CANARY}").as_str());
    state.authenticated.store(valid, Ordering::SeqCst);
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (
        [("x-provider-echo", CANARY)],
        Json(
            json!({"ref":"refs/heads/main","object":{"type":"commit","sha":SHA},"message":CANARY}),
        ),
    )
        .into_response()
}

pub(super) struct Server {
    pub address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(app: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { address, task }
}
pub(super) async fn repository_server() -> (Server, Provider) {
    let state = Provider {
        authenticated: Arc::new(AtomicBool::new(false)),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let server = serve(
        Router::new()
            .route("/repos/acme/widget/git/ref/heads/main", get(repository))
            .with_state(state.clone()),
    )
    .await;
    (server, state)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Api {
    Chat,
    Responses,
}
impl Api {
    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
        }
    }
}
#[derive(Clone)]
pub(super) struct Gateway {
    api: Api,
    pub requests: Arc<Mutex<Vec<Value>>>,
    surface: Arc<Mutex<Option<RepositorySurfaceId>>>,
}
impl Gateway {
    pub fn set_surface(&self, surface: RepositorySurfaceId) {
        *self.surface.lock().unwrap() = Some(surface);
    }
}
async fn models(State(state): State<Gateway>) -> Json<Value> {
    let methods = match state.api {
        Api::Chat => vec!["chat_completion"],
        Api::Responses => vec!["responses"],
    };
    Json(
        json!({"data":[{"id":"openai/gpt-4.1","owned_by":"openai","context_length":128000,"supported_parameters":["tools"],"supported_methods":methods}],"next_page_token":null}),
    )
}
fn frame(value: Value) -> String {
    format!("data: {value}\n\n")
}
async fn inference(
    State(state): State<Gateway>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert!(!body.to_string().contains(CANARY));
    assert!(headers.values().all(|value| !value
        .as_bytes()
        .windows(CANARY.len())
        .any(|part| part == CANARY.as_bytes())));
    let index = {
        let mut requests = state.requests.lock().unwrap();
        let index = requests.len();
        requests.push(body);
        index
    };
    let mut reply = String::new();
    if index == 0 {
        let surface = state
            .surface
            .lock()
            .unwrap()
            .expect("canonical fixture before turn start");
        let args = json!({"work_surface_id":surface}).to_string();
        match state.api {
            Api::Chat => {
                reply.push_str(&frame(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":CALL,"type":"function","function":{"name":"repository_head","arguments":args}}]},"finish_reason":null}]})));
                reply.push_str(&frame(
                    json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                ));
            }
            Api::Responses => {
                reply.push_str(&frame(json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"native-function","call_id":CALL,"name":"repository_head","arguments":""}})));
                reply.push_str(&frame(json!({"type":"response.function_call_arguments.done","item_id":"native-function","output_index":0,"arguments":args})));
            }
        }
    } else {
        match state.api {
            Api::Chat => reply.push_str(&frame(
                json!({"choices":[{"delta":{"content":"Finished."},"finish_reason":"stop"}]}),
            )),
            Api::Responses => {
                reply.push_str(&frame(
                    json!({"type":"response.output_text.delta","delta":"Finished."}),
                ));
                reply.push_str(&frame(
                    json!({"type":"response.completed","response":{"id":"native-completed"}}),
                ));
            }
        }
    }
    ([("content-type", "text/event-stream")], reply).into_response()
}
pub(super) async fn gateway(api: Api) -> (Server, Gateway) {
    let state = Gateway {
        api,
        requests: Arc::new(Mutex::new(vec![])),
        surface: Arc::new(Mutex::new(None)),
    };
    let server = serve(
        Router::new()
            .route("/v1/models", get(models))
            .route(api.path(), post(inference))
            .with_state(state.clone()),
    )
    .await;
    (server, state)
}
