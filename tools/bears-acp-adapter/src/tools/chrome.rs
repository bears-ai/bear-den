use crate::ToolPolicy;
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::{atomic::{AtomicU64, Ordering}, Arc, OnceLock}};
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static CHROME_STATE: OnceLock<Arc<TokioMutex<ChromeState>>> = OnceLock::new();

#[derive(Default)]
struct ChromeState {
    active_ws_url: Option<String>,
    console_events: VecDeque<Value>,
    network_events: VecDeque<Value>,
    max_events: usize,
}

impl ChromeState {
    fn push_event(&mut self, event: Value) {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        if method.starts_with("Runtime.") || method.starts_with("Log.") {
            push_bounded(&mut self.console_events, event.clone(), self.max_events);
        }
        if method.starts_with("Network.") {
            push_bounded(&mut self.network_events, event, self.max_events);
        }
    }
}

fn chrome_state() -> Arc<TokioMutex<ChromeState>> {
    CHROME_STATE
        .get_or_init(|| Arc::new(TokioMutex::new(ChromeState { max_events: 500, ..Default::default() })))
        .clone()
}

fn push_bounded(queue: &mut VecDeque<Value>, value: Value, max: usize) {
    queue.push_back(value);
    while queue.len() > max {
        queue.pop_front();
    }
}

pub(crate) async fn handle_chrome_open(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let url = args.get("url").and_then(Value::as_str).ok_or_else(|| anyhow!("chrome_open args missing url"))?;
    let cdp = cdp_base_url()?;
    let endpoint = format!("{}/json/new?{}", cdp.trim_end_matches('/'), urlencoding::encode(url));
    let target: Value = reqwest::Client::new().put(&endpoint).send().await?.json().await?;
    if let Some(ws_url) = target.get("webSocketDebuggerUrl").and_then(Value::as_str) {
        chrome_state().lock().await.active_ws_url = Some(ws_url.to_string());
    }
    Ok(json!({ "ok": true, "url": url, "target": target, "content": format!("Opened Chrome target for {url}") }))
}

pub(crate) async fn handle_chrome_snapshot(_args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let tree = session.call("Accessibility.getFullAXTree", json!({})).await?;
    let nodes = tree.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    let text = nodes.iter().filter_map(ax_node_text).take(500).collect::<Vec<_>>().join("\n");
    Ok(json!({ "ok": true, "nodes": nodes.len(), "snapshot": text, "content": if text.is_empty() { "Chrome snapshot is empty.".to_string() } else { text } }))
}

pub(crate) async fn handle_chrome_console_messages(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let _ = session.call("Runtime.enable", json!({})).await?;
    let _ = session.call("Log.enable", json!({})).await?;
    session.pump_events_once().await?;
    let state = chrome_state();
    let entries = state.lock().await.console_events.iter().rev().take(limit).cloned().collect::<Vec<_>>();
    Ok(json!({ "ok": true, "messages": entries, "content": "Collected Chrome console event history." }))
}

pub(crate) async fn handle_chrome_network_requests(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let _ = session.call("Network.enable", json!({})).await?;
    session.pump_events_once().await?;
    let state = chrome_state();
    let entries = state.lock().await.network_events.iter().rev().take(limit).cloned().collect::<Vec<_>>();
    Ok(json!({ "ok": true, "requests": entries, "content": "Collected Chrome network event history." }))
}

pub(crate) async fn handle_chrome_screenshot(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let format = args.get("format").and_then(Value::as_str).unwrap_or("png");
    if !matches!(format, "png" | "jpeg") {
        return Err(anyhow!("chrome_screenshot format must be png or jpeg"));
    }
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let result = session.call("Page.captureScreenshot", json!({ "format": format })).await?;
    let data = result.get("data").and_then(Value::as_str).unwrap_or("");
    Ok(json!({ "ok": true, "format": format, "data_base64": data, "bytes_base64": data.len(), "content": format!("Captured Chrome screenshot ({} base64 bytes)", data.len()) }))
}

fn cdp_base_url() -> Result<String> {
    let value = std::env::var("BEARS_CHROME_CDP_URL")
        .or_else(|_| std::env::var("BEARS_BROWSER_CDP_URL"))
        .map_err(|_| anyhow!("Chrome tools require BEARS_CHROME_CDP_URL or BEARS_BROWSER_CDP_URL"))?;
    let value = value.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        return Err(anyhow!("Chrome tools require BEARS_CHROME_CDP_URL or BEARS_BROWSER_CDP_URL"));
    }
    Ok(value)
}

struct ChromeSession {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    events: Vec<Value>,
}

impl ChromeSession {
    async fn connect_active_or_first_page() -> Result<Self> {
        if let Some(ws_url) = chrome_state().lock().await.active_ws_url.clone() {
            if let Ok(session) = Self::connect_ws(&ws_url).await {
                return Ok(session);
            }
        }
        Self::connect_first_page().await
    }

    async fn connect_first_page() -> Result<Self> {
        let cdp = cdp_base_url()?;
        let list_url = format!("{}/json/list", cdp.trim_end_matches('/'));
        let targets: Vec<Value> = reqwest::Client::new().get(&list_url).send().await?.json().await?;
        let ws_url = targets.iter()
            .find(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|t| t.get("webSocketDebuggerUrl").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("No Chrome page target found at {list_url}"))?;
        chrome_state().lock().await.active_ws_url = Some(ws_url.to_string());
        Self::connect_ws(ws_url).await
    }

    async fn connect_ws(ws_url: &str) -> Result<Self> {
        let _ = Url::parse(ws_url)?;
        let (ws, _) = connect_async(ws_url).await?;
        Ok(Self { ws, events: Vec::new() })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        self.ws.send(Message::Text(json!({ "id": id, "method": method, "params": params }).to_string())).await?;
        while let Some(msg) = self.ws.next().await {
            let msg = msg?;
            let Message::Text(text) = msg else { continue; };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(anyhow!("Chrome CDP {method} failed: {error}"));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            chrome_state().lock().await.push_event(value.clone());
            self.events.push(value);
        }
        Err(anyhow!("Chrome CDP websocket closed while waiting for {method}"))
    }

    async fn pump_events_once(&mut self) -> Result<()> {
        if let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(std::time::Duration::from_millis(25), self.ws.next()).await {
            let value: Value = serde_json::from_str(&text)?;
            chrome_state().lock().await.push_event(value.clone());
            self.events.push(value);
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn drain_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
}

fn ax_node_text(node: &Value) -> Option<String> {
    let role = node.pointer("/role/value").and_then(Value::as_str).unwrap_or("");
    let name = node.pointer("/name/value").and_then(Value::as_str).unwrap_or("");
    if role.is_empty() && name.is_empty() { return None; }
    Some(format!("{role}: {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_state_buffers_console_and_network_events() {
        let mut state = ChromeState { max_events: 2, ..Default::default() };
        state.push_event(json!({ "method": "Runtime.consoleAPICalled", "params": { "n": 1 } }));
        state.push_event(json!({ "method": "Network.requestWillBeSent", "params": { "url": "https://example.com" } }));
        state.push_event(json!({ "method": "Runtime.exceptionThrown", "params": { "n": 2 } }));
        state.push_event(json!({ "method": "Runtime.consoleAPICalled", "params": { "n": 3 } }));
        assert_eq!(state.console_events.len(), 2);
        assert_eq!(state.network_events.len(), 1);
        assert_eq!(state.console_events.back().unwrap()["params"]["n"], 3);
    }

    #[test]
    fn cdp_base_url_requires_env() {
        std::env::remove_var("BEARS_CHROME_CDP_URL");
        std::env::remove_var("BEARS_BROWSER_CDP_URL");
        assert!(cdp_base_url().is_err());
    }
}
