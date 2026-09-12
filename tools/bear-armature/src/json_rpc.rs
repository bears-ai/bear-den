use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
#[cfg(test)]
use std::{future::Future, sync::Mutex as StdMutex};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::{
    io::{self, AsyncWriteExt},
    sync::Mutex as TokioMutex,
};
use uuid::Uuid;

static JSON_WRITE_LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();

#[cfg(test)]
static JSON_OUTPUT_CAPTURE: OnceLock<StdMutex<Option<Arc<TokioMutex<Vec<Value>>>>>> =
    OnceLock::new();
#[cfg(test)]
static JSON_OUTPUT_CAPTURE_LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();

#[derive(Debug)]
struct PendingResponse {
    method: String,
    started_at: Instant,
    timeout: Duration,
    tx: tokio::sync::oneshot::Sender<Value>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingRequestSnapshot {
    pub(crate) id: String,
    pub(crate) method: String,
    pub(crate) elapsed_ms: u128,
    pub(crate) timeout_ms: u128,
}

#[derive(Clone, Debug)]
pub(crate) struct TimedOutRequestSnapshot {
    pub(crate) id: String,
    pub(crate) method: String,
    pub(crate) elapsed_ms: u128,
    pub(crate) timeout_ms: u128,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct JsonRpcTransportDiagnostics {
    pub(crate) pending: Vec<PendingRequestSnapshot>,
    pub(crate) recent_timeouts: Vec<TimedOutRequestSnapshot>,
}

#[derive(Clone, Default)]
pub(crate) struct JsonRpcTransport {
    pending_responses: Arc<TokioMutex<HashMap<String, PendingResponse>>>,
    recent_timeouts: Arc<TokioMutex<VecDeque<TimedOutRequestSnapshot>>>,
}

impl JsonRpcTransport {
    #[cfg(test)]
    pub(crate) async fn insert_pending_response_for_test(
        &self,
        id: Value,
        tx: tokio::sync::oneshot::Sender<Value>,
    ) {
        self.pending_responses.lock().await.insert(
            id_key(&id),
            PendingResponse {
                method: "test".to_string(),
                started_at: Instant::now(),
                timeout: Duration::from_secs(1),
                tx,
            },
        );
    }

    pub(crate) async fn diagnostics(&self) -> JsonRpcTransportDiagnostics {
        let now = Instant::now();
        let pending = self
            .pending_responses
            .lock()
            .await
            .iter()
            .map(|(id, pending)| PendingRequestSnapshot {
                id: id.clone(),
                method: pending.method.clone(),
                elapsed_ms: now.duration_since(pending.started_at).as_millis(),
                timeout_ms: pending.timeout.as_millis(),
            })
            .collect();
        let recent_timeouts = self.recent_timeouts.lock().await.iter().cloned().collect();
        JsonRpcTransportDiagnostics {
            pending,
            recent_timeouts,
        }
    }

    pub(crate) async fn route_response(&self, id: &Value, value: Value) -> bool {
        if let Some(pending) = self.pending_responses.lock().await.remove(&id_key(id)) {
            let _ = pending.tx.send(value);
            true
        } else {
            false
        }
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        let id = json!(format!("req-{}", Uuid::new_v4()));
        let key = id_key(&id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let started_at = Instant::now();
        self.pending_responses.lock().await.insert(
            key.clone(),
            PendingResponse {
                method: method.to_string(),
                started_at,
                timeout,
                tx,
            },
        );
        if crate::bear_debug_verbose() {
            eprintln!(
                "bear-armature: JSON-RPC client request sent method={} id={} timeout_ms={}",
                method,
                key,
                timeout.as_millis()
            );
        }
        write_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(anyhow!(
                "client response channel closed for {method} id={key}"
            )),
            Err(_) => {
                self.pending_responses.lock().await.remove(&key);
                let timeout_snapshot = TimedOutRequestSnapshot {
                    id: key.clone(),
                    method: method.to_string(),
                    elapsed_ms: started_at.elapsed().as_millis(),
                    timeout_ms: timeout.as_millis(),
                };
                let mut recent = self.recent_timeouts.lock().await;
                recent.push_back(timeout_snapshot);
                while recent.len() > 20 {
                    recent.pop_front();
                }
                Err(anyhow!(
                    "timed out waiting for client response to {method} id={key}"
                ))
            }
        }
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<()> {
        write_json(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }
}

pub(crate) fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        _ => id.to_string(),
    }
}

pub(crate) async fn write_json(value: Value) -> Result<()> {
    #[cfg(test)]
    {
        let captured = JSON_OUTPUT_CAPTURE
            .get_or_init(|| StdMutex::new(None))
            .lock()
            .expect("json output capture lock")
            .clone();
        if let Some(buffer) = captured {
            buffer.lock().await.push(value);
            return Ok(());
        }
    }

    let line = serde_json::to_string(&value)?;
    let _write_guard = JSON_WRITE_LOCK
        .get_or_init(|| TokioMutex::new(()))
        .lock()
        .await;
    let mut stdout = io::stdout();
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

#[cfg(test)]
pub(crate) async fn capture_json_output_for_test<F, Fut, T>(f: F) -> (T, Vec<Value>)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let _guard = JSON_OUTPUT_CAPTURE_LOCK
        .get_or_init(|| TokioMutex::new(()))
        .lock()
        .await;
    let buffer = Arc::new(TokioMutex::new(Vec::new()));
    {
        let mut capture = JSON_OUTPUT_CAPTURE
            .get_or_init(|| StdMutex::new(None))
            .lock()
            .expect("json output capture lock");
        assert!(
            capture.is_none(),
            "nested JSON output capture is unsupported"
        );
        *capture = Some(buffer.clone());
    }

    let result = f().await;

    {
        let mut capture = JSON_OUTPUT_CAPTURE
            .get_or_init(|| StdMutex::new(None))
            .lock()
            .expect("json output capture lock");
        *capture = None;
    }
    let output = buffer.lock().await.clone();
    (result, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_matching_response() {
        let transport = JsonRpcTransport::default();
        let id = json!("req-test");
        let (tx, rx) = tokio::sync::oneshot::channel();
        transport
            .insert_pending_response_for_test(id.clone(), tx)
            .await;
        assert!(
            transport
                .route_response(&id, json!({ "id": "req-test", "result": { "ok": true } }))
                .await
        );
        let routed = rx.await.unwrap();
        assert_eq!(routed["result"]["ok"], true);
    }

    #[tokio::test]
    async fn reports_unmatched_response() {
        let transport = JsonRpcTransport::default();
        assert!(
            !transport
                .route_response(&json!("missing"), json!({ "id": "missing" }))
                .await
        );
    }

    #[tokio::test]
    async fn concurrent_notifications_are_distinct_json_rpc_messages() {
        let (_result, output) = capture_json_output_for_test(|| async {
            let left = tokio::spawn(async {
                JsonRpcTransport::default()
                    .notify("session/update", json!({ "side": "left" }))
                    .await
            });
            let right = tokio::spawn(async {
                JsonRpcTransport::default()
                    .notify("session/update", json!({ "side": "right" }))
                    .await
            });
            left.await.unwrap().unwrap();
            right.await.unwrap().unwrap();
        })
        .await;

        let notifications = output
            .iter()
            .filter(|value| {
                matches!(
                    value.pointer("/params/side").and_then(Value::as_str),
                    Some("left" | "right")
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(notifications.len(), 2, "captured output: {output:?}");
        assert!(notifications.iter().all(|value| {
            value.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && value.get("method").and_then(Value::as_str) == Some("session/update")
        }));
        assert!(notifications
            .iter()
            .any(|value| value.pointer("/params/side") == Some(&json!("left"))));
        assert!(notifications
            .iter()
            .any(|value| value.pointer("/params/side") == Some(&json!("right"))));
    }
}
