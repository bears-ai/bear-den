use crate::ToolPolicy;
use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{json, Map, Value};
use std::{
    collections::VecDeque,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::{
    net::TcpStream,
    process::{Child, Command},
    sync::Mutex as TokioMutex,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static CHROME_STATE: OnceLock<Arc<TokioMutex<ChromeState>>> = OnceLock::new();
static CHROME_CAPABILITY: OnceLock<ChromeCapability> = OnceLock::new();

#[derive(Clone, Debug)]
pub(crate) enum ChromeCapability {
    Unavailable {
        reason: String,
    },
    ExternalCdp {
        base_url: String,
        source: &'static str,
    },
    ManagedLaunchable {
        executable: PathBuf,
    },
}

impl ChromeCapability {
    pub(crate) fn detect() -> &'static Self {
        CHROME_CAPABILITY.get_or_init(detect_chrome_capability)
    }

    pub(crate) fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable { .. })
    }

    pub(crate) fn status_line(&self) -> String {
        match self {
            Self::Unavailable { reason } => format!("unavailable ({reason})"),
            Self::ExternalCdp { base_url, source } => {
                format!("available via {source}={base_url}")
            }
            Self::ManagedLaunchable { executable } => {
                format!(
                    "available via managed local Chrome at {}",
                    executable.display()
                )
            }
        }
    }
}

struct ManagedChrome {
    child: Child,
    base_url: String,
    user_data_dir: PathBuf,
}

#[derive(Default)]
struct ChromeState {
    active_ws_url: Option<String>,
    console_events: VecDeque<Value>,
    network_events: VecDeque<Value>,
    max_events: usize,
    managed: Option<ManagedChrome>,
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

impl Drop for ManagedChrome {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.user_data_dir);
    }
}

fn chrome_state() -> Arc<TokioMutex<ChromeState>> {
    CHROME_STATE
        .get_or_init(|| {
            Arc::new(TokioMutex::new(ChromeState {
                max_events: 500,
                ..Default::default()
            }))
        })
        .clone()
}

fn push_bounded(queue: &mut VecDeque<Value>, value: Value, max: usize) {
    queue.push_back(value);
    while queue.len() > max {
        queue.pop_front();
    }
}

pub(crate) fn chrome_tools_available() -> bool {
    ChromeCapability::detect().is_available()
}

pub(crate) fn chrome_capability_status_line() -> String {
    ChromeCapability::detect().status_line()
}

pub(crate) async fn handle_chrome_open(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("chrome_open args missing url"))?;
    validate_chrome_open_url(url)?;
    let cdp = ensure_cdp_base_url().await?;
    let endpoint = format!(
        "{}/json/new?{}",
        cdp.trim_end_matches('/'),
        urlencoding::encode(url)
    );
    let target: Value = reqwest::Client::new()
        .put(&endpoint)
        .send()
        .await?
        .json()
        .await?;
    if let Some(ws_url) = target.get("webSocketDebuggerUrl").and_then(Value::as_str) {
        chrome_state().lock().await.active_ws_url = Some(ws_url.to_string());
    }
    Ok(
        json!({ "ok": true, "url": url, "target": target, "content": format!("Opened Chrome target for {url}") }),
    )
}

pub(crate) async fn handle_chrome_snapshot(_args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let tree = session
        .call("Accessibility.getFullAXTree", json!({}))
        .await?;
    let nodes = tree
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text = nodes
        .iter()
        .filter_map(ax_node_text)
        .take(500)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(
        json!({ "ok": true, "nodes": nodes.len(), "snapshot": text, "content": if text.is_empty() { "Chrome snapshot is empty.".to_string() } else { text } }),
    )
}

pub(crate) async fn handle_chrome_console_messages(
    args: &Value,
    _policy: &ToolPolicy,
) -> Result<Value> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let _ = session.call("Runtime.enable", json!({})).await?;
    let _ = session.call("Log.enable", json!({})).await?;
    session.pump_events_once().await?;
    let state = chrome_state();
    let entries = state
        .lock()
        .await
        .console_events
        .iter()
        .rev()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let content = format_chrome_events("Chrome console messages", &entries);
    Ok(json!({ "ok": true, "messages": entries, "content": content }))
}

pub(crate) async fn handle_chrome_network_requests(
    args: &Value,
    _policy: &ToolPolicy,
) -> Result<Value> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let _ = session.call("Network.enable", json!({})).await?;
    session.pump_events_once().await?;
    let state = chrome_state();
    let entries = state
        .lock()
        .await
        .network_events
        .iter()
        .rev()
        .take(limit)
        .cloned()
        .map(redact_network_event)
        .collect::<Vec<_>>();
    let content = format_chrome_events("Chrome network requests", &entries);
    Ok(json!({ "ok": true, "requests": entries, "content": content }))
}

pub(crate) async fn handle_chrome_screenshot(args: &Value, _policy: &ToolPolicy) -> Result<Value> {
    let format = args.get("format").and_then(Value::as_str).unwrap_or("png");
    if !matches!(format, "png" | "jpeg") {
        return Err(anyhow!("chrome_screenshot format must be png or jpeg"));
    }
    let mut session = ChromeSession::connect_active_or_first_page().await?;
    let result = session
        .call("Page.captureScreenshot", json!({ "format": format }))
        .await?;
    let data = result.get("data").and_then(Value::as_str).unwrap_or("");
    Ok(
        json!({ "ok": true, "format": format, "data_base64": data, "bytes_base64": data.len(), "content": format!("Captured Chrome screenshot ({} base64 bytes)", data.len()) }),
    )
}

fn validate_chrome_open_url(url: &str) -> Result<()> {
    let parsed = Url::parse(url).with_context(|| format!("invalid chrome_open url {url:?}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(anyhow!(
            "chrome_open only allows http and https URLs; rejected scheme {scheme:?}"
        )),
    }
}

fn redact_network_event(event: Value) -> Value {
    let mut event = event;
    redact_sensitive_headers_in_value(&mut event);
    event
}

fn redact_sensitive_headers_in_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            redact_headers_map_if_present(map, "headers");
            redact_headers_map_if_present(map, "requestHeaders");
            redact_headers_map_if_present(map, "responseHeaders");
            for child in map.values_mut() {
                redact_sensitive_headers_in_value(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_sensitive_headers_in_value(child);
            }
        }
        _ => {}
    }
}

fn redact_headers_map_if_present(map: &mut Map<String, Value>, key: &str) {
    let Some(Value::Object(headers)) = map.get_mut(key) else {
        return;
    };
    for (name, value) in headers.iter_mut() {
        if should_redact_header(name) {
            *value = Value::String("<redacted>".to_string());
        }
    }
}

#[cfg(test)]
pub(crate) fn test_redact_network_event(event: Value) -> Value {
    redact_network_event(event)
}

fn should_redact_header(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "set-cookie" | "x-api-key" | "proxy-authorization"
    )
}

fn format_chrome_events(label: &str, entries: &[Value]) -> String {
    if entries.is_empty() {
        return format!("{label}: no events collected.");
    }
    let mut lines = vec![format!("{label} (latest {}):", entries.len())];
    for entry in entries.iter().take(25) {
        let method = entry
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let params = entry.get("params").unwrap_or(entry);
        let detail = params
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| params.pointer("/request/url").and_then(Value::as_str))
            .or_else(|| params.get("url").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| params.to_string());
        lines.push(format!("- {method}: {}", truncate_detail(&detail, 240)));
    }
    if entries.len() > 25 {
        lines.push(format!("... {} more events omitted", entries.len() - 25));
    }
    lines.join("\n")
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    if value.chars().count() > max_chars {
        format!(
            "{}…",
            value
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        value.to_string()
    }
}

fn detect_chrome_capability() -> ChromeCapability {
    if let Ok(value) = std::env::var("BEARS_CHROME_CDP_URL") {
        let base_url = value.trim().trim_end_matches('/').to_string();
        if !base_url.is_empty() {
            return ChromeCapability::ExternalCdp {
                base_url,
                source: "BEARS_CHROME_CDP_URL",
            };
        }
    }
    if let Ok(value) = std::env::var("BEARS_BROWSER_CDP_URL") {
        let base_url = value.trim().trim_end_matches('/').to_string();
        if !base_url.is_empty() {
            return ChromeCapability::ExternalCdp {
                base_url,
                source: "BEARS_BROWSER_CDP_URL",
            };
        }
    }
    if let Some(executable) = detect_local_chrome_executable() {
        return ChromeCapability::ManagedLaunchable { executable };
    }
    ChromeCapability::Unavailable {
        reason: "no BEARS_CHROME_CDP_URL/BEARS_BROWSER_CDP_URL configured and no local Chrome executable found".to_string(),
    }
}

fn detect_local_chrome_executable() -> Option<PathBuf> {
    let candidates = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    for candidate in candidates {
        if let Some(path) = resolve_executable(candidate) {
            return Some(path);
        }
    }
    None
}

fn resolve_executable(candidate: &str) -> Option<PathBuf> {
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path.exists().then_some(path);
    }
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let full = dir.join(candidate);
        if full.exists() {
            return Some(full);
        }
    }
    None
}

async fn ensure_cdp_base_url() -> Result<String> {
    let capability = ChromeCapability::detect().clone();
    match capability {
        ChromeCapability::ExternalCdp { base_url, .. } => Ok(base_url),
        ChromeCapability::ManagedLaunchable { executable } => {
            ensure_managed_chrome(executable).await
        }
        ChromeCapability::Unavailable { reason } => {
            Err(anyhow!("Chrome tools unavailable: {reason}"))
        }
    }
}

async fn ensure_managed_chrome(executable: PathBuf) -> Result<String> {
    let state = chrome_state();
    {
        let mut guard = state.lock().await;
        if let Some(managed) = guard.managed.as_mut() {
            if let Some(status) = managed.child.try_wait()? {
                return Err(anyhow!(
                    "Managed Chrome exited unexpectedly with status {status}"
                ));
            }
            return Ok(managed.base_url.clone());
        }
    }

    let base_url = launch_managed_chrome(&executable).await?;
    Ok(base_url)
}

async fn launch_managed_chrome(executable: &Path) -> Result<String> {
    let port = choose_unused_port()?;
    let user_data_dir = std::env::temp_dir().join(format!(
        "bears-acp-chrome-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&user_data_dir)?;

    let mut command = Command::new(executable);
    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().with_context(|| {
        format!(
            "failed to launch Chrome executable {}",
            executable.display()
        )
    })?;
    let base_url = format!("http://127.0.0.1:{port}");
    wait_for_cdp_ready(&base_url, Duration::from_secs(5)).await?;

    let state = chrome_state();
    state.lock().await.managed = Some(ManagedChrome {
        child,
        base_url: base_url.clone(),
        user_data_dir,
    });
    Ok(base_url)
}

async fn wait_for_cdp_ready(base_url: &str, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    let version_url = format!("{}/json/version", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    loop {
        if start.elapsed() > timeout {
            return Err(anyhow!("Timed out waiting for Chrome CDP at {version_url}"));
        }
        if let Ok(resp) = client.get(&version_url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn choose_unused_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

struct ChromeSession {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
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
        let cdp = ensure_cdp_base_url().await?;
        let list_url = format!("{}/json/list", cdp.trim_end_matches('/'));
        let targets: Vec<Value> = reqwest::Client::new()
            .get(&list_url)
            .send()
            .await?
            .json()
            .await?;
        let ws_url = targets
            .iter()
            .find(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|t| t.get("webSocketDebuggerUrl").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("No Chrome page target found at {list_url}"))?;
        chrome_state().lock().await.active_ws_url = Some(ws_url.to_string());
        Self::connect_ws(ws_url).await
    }

    async fn connect_ws(ws_url: &str) -> Result<Self> {
        let _ = Url::parse(ws_url)?;
        let (ws, _) = connect_async(ws_url).await?;
        Ok(Self {
            ws,
            events: Vec::new(),
        })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        self.ws
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params }).to_string(),
            ))
            .await?;
        while let Some(msg) = self.ws.next().await {
            let msg = msg?;
            let Message::Text(text) = msg else {
                continue;
            };
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
        Err(anyhow!(
            "Chrome CDP websocket closed while waiting for {method}"
        ))
    }

    async fn pump_events_once(&mut self) -> Result<()> {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(25), self.ws.next()).await
        {
            let value: Value = serde_json::from_str(&text)?;
            chrome_state().lock().await.push_event(value.clone());
            self.events.push(value);
        }
        Ok(())
    }
}

fn ax_node_text(node: &Value) -> Option<String> {
    let role = node
        .pointer("/role/value")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = node
        .pointer("/name/value")
        .and_then(Value::as_str)
        .unwrap_or("");
    if role.is_empty() && name.is_empty() {
        return None;
    }
    Some(format!("{role}: {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_state_buffers_console_and_network_events() {
        let mut state = ChromeState {
            max_events: 2,
            ..Default::default()
        };
        state.push_event(json!({ "method": "Runtime.consoleAPICalled", "params": { "n": 1 } }));
        state.push_event(json!({ "method": "Network.requestWillBeSent", "params": { "url": "https://example.com" } }));
        state.push_event(json!({ "method": "Runtime.exceptionThrown", "params": { "n": 2 } }));
        state.push_event(json!({ "method": "Runtime.consoleAPICalled", "params": { "n": 3 } }));
        assert_eq!(state.console_events.len(), 2);
        assert_eq!(state.network_events.len(), 1);
        assert_eq!(state.console_events.back().unwrap()["params"]["n"], 3);
    }
}
