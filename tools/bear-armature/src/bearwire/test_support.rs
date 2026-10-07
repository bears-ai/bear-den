use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

pub(crate) struct OpenControl {
    pub entered: Notify,
    pub release: Notify,
}

impl Default for OpenControl {
    fn default() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
        }
    }
}

pub(crate) struct MockDenOptions {
    pub initialize: Value,
    pub checkout: Value,
    pub session: Value,
    pub open_control: Option<Arc<OpenControl>>,
}

impl Default for MockDenOptions {
    fn default() -> Self {
        let conversation_id = Uuid::new_v4();
        Self {
            initialize: json!({
                "protocol": "bearwire",
                "version": 1,
                "capabilities": { "expected_work_source": true, "session_access": true },
            }),
            checkout: Value::Null,
            session: json!({
                "conversation_id": conversation_id,
                "resolved_conversation_id": conversation_id,
                "history_conversation_id": conversation_id,
                "access": { "state": "executable", "may_select_hat": false },
            }),
            open_control: None,
        }
    }
}

struct MockDenState {
    options: MockDenOptions,
    requests: Arc<Mutex<Vec<Value>>>,
}

pub(crate) struct MockDen {
    pub config: crate::Config,
    pub requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockDen {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockDen {
    pub async fn start(options: MockDenOptions) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(MockDenState {
            options,
            requests: requests.clone(),
        });
        let app = Router::new()
            .route("/bearwire/v1/rpc", post(rpc_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Den");
        let addr = listener.local_addr().expect("mock address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock Den");
        });
        Self {
            config: crate::Config {
                api_url: format!("http://{addr}"),
                bear: "test-bear".to_string(),
                token: "test-token".to_string(),
                client: "test-armature".to_string(),
            },
            requests,
            task,
        }
    }

    pub async fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .await
            .iter()
            .map(|request| request["method"].as_str().expect("RPC method").to_string())
            .collect()
    }
}

async fn rpc_handler(
    State(state): State<Arc<MockDenState>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    state.requests.lock().await.push(request.clone());
    let result = match request["method"].as_str().expect("RPC method") {
        "initialize" => state.options.initialize.clone(),
        "session.state" => json!({ "kind": "collection", "sessions": [] }),
        "work.checkout" => state.options.checkout.clone(),
        "session.open" => {
            if let Some(control) = &state.options.open_control {
                control.entered.notify_one();
                control.release.notified().await;
            }
            let mut session = state.options.session.clone();
            session["client_session_id"] = request["params"]["session_id"].clone();
            json!({ "ok": true, "session": session })
        }
        "session.model.get" => json!({}),
        "work.report" => json!({ "ok": true }),
        "run.start" => {
            return Json(json!({
                "jsonrpc": "2.0", "id": request["id"],
                "error": { "code": -32000, "message": "test stop after run.start" },
            }));
        }
        method => panic!("unexpected mock Den method: {method}"),
    };
    Json(json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }))
}

pub(crate) fn allowed_checkout(work_run_id: Uuid, attempt_id: Uuid, fence_epoch: i64) -> Value {
    json!({
        "ok": true,
        "work_run_id": work_run_id,
        "gate": {
            "status": "allowed",
            "task_id": Uuid::new_v4(),
            "binding": { "kind": "work_run", "work_run_id": work_run_id, "job_run_id": Uuid::new_v4() },
        },
        "execution_attempt_id": attempt_id,
        "execution_attempt_fence_epoch": fence_epoch,
        "prompt": "Den-rendered Work prompt",
        "deadline_secs": null,
    })
}
