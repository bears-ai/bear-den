use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    io::{self, AsyncWriteExt},
    sync::Mutex as TokioMutex,
};
use uuid::Uuid;

#[derive(Clone, Default)]
pub(crate) struct JsonRpcTransport {
    pending_responses: Arc<TokioMutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>>,
}

impl JsonRpcTransport {
    #[cfg(test)]
    pub(crate) async fn insert_pending_response_for_test(
        &self,
        id: Value,
        tx: tokio::sync::oneshot::Sender<Value>,
    ) {
        self.pending_responses.lock().await.insert(id_key(&id), tx);
    }

    pub(crate) async fn route_response(&self, id: &Value, value: Value) -> bool {
        if let Some(tx) = self.pending_responses.lock().await.remove(&id_key(id)) {
            let _ = tx.send(value);
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
        self.pending_responses.lock().await.insert(key.clone(), tx);
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
    let mut stdout = io::stdout();
    let line = serde_json::to_string(&value)?;
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
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
}
