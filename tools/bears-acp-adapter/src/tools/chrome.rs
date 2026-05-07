use crate::ToolPolicy;
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_tungstenite::{connect_async, tungstenite::Message};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) async fn handle_chrome_open(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let url = args.get("url").and_then(Value::as_str).ok_or_else(|| anyhow!("chrome_open args missing url"))?;
    let cdp = cdp_base_url()?;
    let endpoint = format!("{}/json/new?{}", cdp.trim_end_matches('/'), urlencoding::encode(url));
    let target: Value = reqwest::Client::new().put(&endpoint).send().await?.json().await?;
    Ok(json!({ "ok": true, "url": url, "target": target, "content": format!("Opened Chrome target for {url}") }))
}

pub(crate) async fn handle_chrome_snapshot(_args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let mut session = ChromeSession::connect_first_page().await?;
    let tree = session.call("Accessibility.getFullAXTree", json!({})).await?;
    let nodes = tree.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    let text = nodes.iter().filter_map(ax_node_text).take(500).collect::<Vec<_>>().join("\n");
    Ok(json!({ "ok": true, "nodes": nodes.len(), "snapshot": text, "content": if text.is_empty() { "Chrome snapshot is empty.".to_string() } else { text } }))
}

pub(crate) async fn handle_chrome_console_messages(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_first_page().await?;
    let _ = session.call("Runtime.enable", json!({})).await?;
    let entries = session.drain_events();
    Ok(json!({ "ok": true, "messages": entries.into_iter().take(limit).collect::<Vec<_>>(), "content": "Collected Chrome console event buffer." }))
}

pub(crate) async fn handle_chrome_network_requests(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_first_page().await?;
    let _ = session.call("Network.enable", json!({})).await?;
    let entries = session.drain_events();
    Ok(json!({ "ok": true, "requests": entries.into_iter().take(limit).collect::<Vec<_>>(), "content": "Collected Chrome network event buffer." }))
}

pub(crate) async fn handle_chrome_screenshot(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let format = args.get("format").and_then(Value::as_str).unwrap_or("png");
    if !matches!(format, "png" | "jpeg") {
        return Err(anyhow!("chrome_screenshot format must be png or jpeg"));
    }
    let mut session = ChromeSession::connect_first_page().await?;
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
    async fn connect_first_page() -> Result<Self> {
        let cdp = cdp_base_url()?;
        let list_url = format!("{}/json/list", cdp.trim_end_matches('/'));
        let targets: Vec<Value> = reqwest::Client::new().get(&list_url).send().await?.json().await?;
        let ws_url = targets.iter()
            .find(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|t| t.get("webSocketDebuggerUrl").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("No Chrome page target found at {list_url}"))?;
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
            self.events.push(value);
        }
        Err(anyhow!("Chrome CDP websocket closed while waiting for {method}"))
    }

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
    fn cdp_base_url_requires_env() {
        std::env::remove_var("BEARS_CHROME_CDP_URL");
        std::env::remove_var("BEARS_BROWSER_CDP_URL");
        assert!(cdp_base_url().is_err());
    }
}
