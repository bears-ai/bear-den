use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use futures::Stream;
use reqwest::{
    header::{HeaderName, HeaderValue},
    RequestBuilder, Response, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use den_core::{
    config::Config, DenError, ProviderFailure, ProviderFailureKind, ProviderHttpStatus,
    ProviderTransportKind, ThinkingEffort,
};

#[path = "provider_failure.rs"]
pub mod provider_failure;
use provider_failure::{
    classify_stream_read_failure, classify_transport_failure, read_http_failure,
};

#[cfg(test)]
#[path = "client_failure_tests.rs"]
mod failure_tests;

#[cfg(test)]
use crate::model_registry;

const LLM_PROVIDER_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(54),
];

/// Canonical Den model handle used in product/UI/DB/policy state, e.g. `openai/gpt-5.5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenModelHandle(String);

impl DenModelHandle {
    pub fn normalize(model: &str) -> Self {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Self("openai/gpt-4o-mini".to_string());
        }
        if trimmed.contains('/') {
            Self(trimmed.to_string())
        } else {
            Self(format!("openai/{trimmed}"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for DenModelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Provider/Bifrost request model id, e.g. `gpt-5.5` for Den handle `openai/gpt-5.5`.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelId(String);

#[cfg(test)]
impl ProviderModelId {
    pub fn for_den_handle(handle: &DenModelHandle) -> Self {
        Self(
            model_registry::provider_model_id_for_handle(handle.as_str())
                .unwrap_or(handle.as_str())
                .to_string(),
        )
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl std::fmt::Display for ProviderModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bifrost expects `provider/model`; bare OpenAI-style ids get an `openai/` prefix.
pub fn normalize_llm_model_handle(model: &str) -> String {
    DenModelHandle::normalize(model).into_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

/// Content-free category of Den work that issued an LLM request.
///
/// This intentionally has a closed vocabulary: usage attribution must never
/// accidentally contain a prompt, tool argument, or other user-provided text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmOperation {
    AgentTurn,
    Compaction,
    Memory,
    BackgroundWork,
}

impl LlmOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentTurn => "agent_turn",
            Self::Compaction => "compaction",
            Self::Memory => "memory",
            Self::BackgroundWork => "background_work",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmRequestTelemetry {
    pub request_id: Option<String>,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub bear_id: Option<String>,
    pub stance: Option<String>,
    /// Stable, content-free category of Den work that issued this request.
    pub operation: Option<LlmOperation>,
    pub bifrost_virtual_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmApiStyle {
    ChatCompletionsStream,
    ResponsesStream,
}

impl LlmApiStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            LlmApiStyle::ChatCompletionsStream => "chat_completions_stream",
            LlmApiStyle::ResponsesStream => "responses_stream",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<LlmToolDefinition>,
    pub stream: bool,
    pub tool_choice: Option<Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub telemetry: Option<LlmRequestTelemetry>,
}

fn chat_message_chain_diagnostic(messages: &[ChatMessage]) -> Value {
    let mut assistant_tool_call_blocks = 0_u64;
    let mut tool_messages = 0_u64;
    let mut orphan_tool_messages = Vec::<String>::new();
    let mut unresolved_tool_call_ids = Vec::<String>::new();
    let mut duplicate_tool_result_ids = Vec::<String>::new();
    let mut role_tail = Vec::<String>::new();
    let mut index = 0_usize;

    while index < messages.len() {
        let message = &messages[index];
        if message.role == "assistant" {
            if let Some(calls) = message
                .tool_calls
                .as_ref()
                .filter(|calls| !calls.is_empty())
            {
                assistant_tool_call_blocks += 1;
                let required = calls.iter().map(|call| call.id.clone()).collect::<Vec<_>>();
                let mut seen = std::collections::HashSet::<String>::new();
                let mut scan = index + 1;
                while scan < messages.len() && messages[scan].role == "tool" {
                    tool_messages += 1;
                    let tool_call_id = messages[scan]
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| "<missing>".to_string());
                    if !required.iter().any(|id| id == &tool_call_id) {
                        orphan_tool_messages.push(tool_call_id);
                    } else if !seen.insert(tool_call_id.clone()) {
                        duplicate_tool_result_ids.push(tool_call_id);
                    }
                    scan += 1;
                }
                for required_id in required {
                    if !seen.contains(&required_id) {
                        unresolved_tool_call_ids.push(required_id);
                    }
                }
                index = scan;
                continue;
            }
        } else if message.role == "tool" {
            tool_messages += 1;
            orphan_tool_messages.push(
                message
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| "<missing>".to_string()),
            );
        }
        index += 1;
    }

    let tail_start = messages.len().saturating_sub(12);
    for message in &messages[tail_start..] {
        let role = if let Some(calls) = message
            .tool_calls
            .as_ref()
            .filter(|calls| !calls.is_empty())
        {
            format!("{}[tool_calls:{}]", message.role, calls.len())
        } else if message.role == "tool" {
            format!(
                "tool[{}]",
                message.tool_call_id.as_deref().unwrap_or("<missing>")
            )
        } else {
            message.role.clone()
        };
        role_tail.push(role);
    }

    let invalid = !orphan_tool_messages.is_empty()
        || !unresolved_tool_call_ids.is_empty()
        || !duplicate_tool_result_ids.is_empty();
    json!({
        "invalid": invalid,
        "message_count": messages.len(),
        "assistant_tool_call_blocks": assistant_tool_call_blocks,
        "tool_messages": tool_messages,
        "orphan_tool_messages": orphan_tool_messages,
        "unresolved_tool_call_ids": unresolved_tool_call_ids,
        "duplicate_tool_result_ids": duplicate_tool_result_ids,
        "role_tail": role_tail,
    })
}

impl LlmRequestTelemetry {
    fn field(&self, name: &str) -> Option<&str> {
        match name {
            "request_id" => self.request_id.as_deref(),
            "run_id" => self.run_id.as_deref(),
            "session_id" => self.session_id.as_deref(),
            "conversation_id" => self.conversation_id.as_deref(),
            "bear_id" => self.bear_id.as_deref(),
            "stance" => self.stance.as_deref(),
            "operation" => self.operation.map(LlmOperation::as_str),
            "bifrost_virtual_key" => self.bifrost_virtual_key.as_deref(),
            _ => None,
        }
    }
}

impl ChatCompletionRequest {
    pub fn to_body(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect();
        let mut body = json!({
            // Bifrost requires provider-qualified ids (`provider/model`) on both
            // `/v1/completions` and `/v1/responses`; normalize defensively.
            "model": normalize_llm_model_handle(&self.model),
            "messages": self.messages,
            "stream": self.stream,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        if let Some(tool_choice) = &self.tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = self.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(thinking_effort) = self.thinking_effort {
            body["reasoning_effort"] = json!(thinking_effort.as_str());
        }
        body
    }

    pub fn to_responses_body(&self) -> Value {
        let input: Vec<Value> = self
            .messages
            .iter()
            .flat_map(chat_message_to_responses_input_items)
            .collect();
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        // Bifrost's `/v1/responses` requires provider-qualified ids (`provider/model`),
        // same as `/v1/completions`; sending a bare id yields HTTP 400
        // "model should be in provider/model format".
        let mut body = json!({
            "model": normalize_llm_model_handle(&self.model),
            "input": input,
            "stream": self.stream,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        if let Some(tool_choice) = &self.tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = self.max_tokens {
            body["max_output_tokens"] = json!(max_tokens);
        }
        if let Some(thinking_effort) = self.thinking_effort {
            body["reasoning"] = json!({
                "effort": thinking_effort.as_str(),
            });
        }
        body
    }
}

fn chat_message_to_responses_input_items(message: &ChatMessage) -> Vec<Value> {
    let content = message.content.clone().unwrap_or_default();
    if message.role == "tool" {
        return vec![json!({
            "type": "function_call_output",
            "call_id": message.tool_call_id.clone().unwrap_or_default(),
            "output": content,
        })];
    }

    let mut items = Vec::new();
    if !content.is_empty() || message.tool_calls.is_none() {
        items.push(json!({
            "role": message.role,
            "content": content,
        }));
    }

    if let Some(tool_calls) = &message.tool_calls {
        for tool_call in tool_calls {
            items.push(json!({
                "type": "function_call",
                "call_id": &tool_call.id,
                "name": &tool_call.function.name,
                "arguments": &tool_call.function.arguments,
            }));
        }
    }

    items
}

pub fn preferred_api_style_for_model(model: &str) -> LlmApiStyle {
    preferred_api_style_for_model_with_catalog_support(model, None)
}

pub fn preferred_api_style_for_model_with_catalog_support(
    model: &str,
    supports_responses_api: Option<bool>,
) -> LlmApiStyle {
    if let Some(supports_responses_api) = supports_responses_api {
        return if supports_responses_api {
            LlmApiStyle::ResponsesStream
        } else {
            LlmApiStyle::ChatCompletionsStream
        };
    }
    let model = normalize_llm_model_handle(model);
    if model == "openai/gpt-5.5" || model.ends_with("/gpt-5.5") {
        LlmApiStyle::ResponsesStream
    } else {
        LlmApiStyle::ChatCompletionsStream
    }
}

fn apply_optional_header(
    req: reqwest::RequestBuilder,
    name: &'static str,
    value: Option<&str>,
) -> reqwest::RequestBuilder {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return req;
    };
    let Ok(mut value) = HeaderValue::from_str(value) else {
        tracing::warn!(header = name, "skipping invalid LLM telemetry header value");
        return req;
    };
    value.set_sensitive(true);
    req.header(HeaderName::from_static(name), value)
}

fn apply_bears_telemetry_headers(
    mut req: reqwest::RequestBuilder,
    telemetry: Option<&LlmRequestTelemetry>,
) -> reqwest::RequestBuilder {
    let Some(telemetry) = telemetry else {
        return req;
    };
    req = apply_optional_header(req, "x-bears-request-id", telemetry.field("request_id"));
    req = apply_optional_header(req, "x-bears-run-id", telemetry.field("run_id"));
    req = apply_optional_header(req, "x-bears-session-id", telemetry.field("session_id"));
    req = apply_optional_header(
        req,
        "x-bears-conversation-id",
        telemetry.field("conversation_id"),
    );
    req = apply_optional_header(req, "x-bears-bear-id", telemetry.field("bear_id"));
    req = apply_optional_header(req, "x-bears-stance", telemetry.field("stance"));
    apply_optional_header(req, "x-bears-operation", telemetry.field("operation"))
}

fn has_bifrost_virtual_key(telemetry: Option<&LlmRequestTelemetry>) -> bool {
    telemetry
        .and_then(|telemetry| telemetry.field("bifrost_virtual_key"))
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn require_bifrost_virtual_key(telemetry: Option<&LlmRequestTelemetry>) -> Result<(), DenError> {
    if has_bifrost_virtual_key(telemetry) {
        return Ok(());
    }
    let bear_id = telemetry
        .and_then(|telemetry| telemetry.bear_id.as_deref())
        .unwrap_or("unknown");
    let conversation_id = telemetry
        .and_then(|telemetry| telemetry.conversation_id.as_deref())
        .unwrap_or("unknown");
    Err(DenError::System(format!(
        "Bifrost virtual key is required for LLM inference, but none is configured for this request. Provision a Bear-scoped Bifrost virtual key before running this Bear. Context: bear_id={bear_id}, conversation_id={conversation_id}."
    )))
}

fn apply_bifrost_virtual_key_header(
    req: reqwest::RequestBuilder,
    telemetry: Option<&LlmRequestTelemetry>,
) -> reqwest::RequestBuilder {
    let Some(telemetry) = telemetry else {
        return req;
    };
    // Bifrost virtual keys are inference credentials. In Bifrost 1.6.x, use the
    // dedicated governance virtual-key header for inference; `x-api-key` can be
    // interpreted as a provider/API key path and may fail virtual-key lookup on
    // responses/chat requests even when management usage views work.
    apply_optional_header(req, "x-bf-vk", telemetry.field("bifrost_virtual_key"))
}

fn apply_bifrost_session_cache_headers(
    mut req: reqwest::RequestBuilder,
    telemetry: Option<&LlmRequestTelemetry>,
) -> reqwest::RequestBuilder {
    let Some(telemetry) = telemetry else {
        return req;
    };
    // Bifrost session stickiness: keep a conversation pinned to the same
    // upstream key via the documented x-bf-session-id header. Direct cache
    // uses the same partition key and is forced to hash-only mode; no
    // embeddings are required.
    req = apply_optional_header(req, "x-bf-session-id", telemetry.field("conversation_id"));
    req = apply_optional_header(req, "x-bf-cache-key", telemetry.field("conversation_id"));
    apply_optional_header(req, "x-bf-cache-type", Some("direct"))
}

/// Compatibility entry point for raw HTTP error bodies, not rendered application errors.
/// New callers should use the typed provider-failure classifiers.
pub fn bifrost_key_selection_error(body: &str) -> bool {
    provider_failure::classify_http_failure(
        ProviderHttpStatus::new(400).expect("constant HTTP status"),
        body.as_bytes(),
    )
    .kind()
        == ProviderFailureKind::KeySelection
}

fn llm_provider_error_retryable_status(status: StatusCode) -> bool {
    ProviderHttpStatus::new(status.as_u16()).is_some_and(ProviderHttpStatus::request_retryable)
}

fn log_provider_failure(api_style: LlmApiStyle, failure: &ProviderFailure, started: Instant) {
    tracing::warn!(
        api_style = api_style.as_str(),
        failure_kind = failure.kind().code(),
        transport_kind = failure.transport_kind().map(ProviderTransportKind::code),
        http_status = failure.http_status().map(ProviderHttpStatus::as_u16),
        attempt = failure.retry_attempts(),
        retry_exhausted = failure.retry_exhausted(),
        key_selection_fallback_status = failure
            .key_selection_fallback_status()
            .map(ProviderHttpStatus::as_u16),
        duration_ms = started.elapsed().as_millis(),
        "LLM provider request failed"
    );
}

async fn send_llm_provider_request_with_retries(
    build_request: impl FnMut() -> RequestBuilder,
    api_style: LlmApiStyle,
    started: Instant,
) -> Result<(Response, u32), DenError> {
    send_llm_provider_request_with_retry_delay(
        build_request,
        api_style,
        started,
        sleep_llm_provider_retry_delay,
    )
    .await
}

// The delay seam lets tests exercise the production retry loop without real backoff.
async fn send_llm_provider_request_with_retry_delay<F: std::future::Future<Output = ()>>(
    mut build_request: impl FnMut() -> RequestBuilder,
    api_style: LlmApiStyle,
    started: Instant,
    mut sleep: impl FnMut(Duration) -> F,
) -> Result<(Response, u32), DenError> {
    for attempt_index in 0..=LLM_PROVIDER_RETRY_DELAYS.len() {
        let attempt_number = attempt_index as u32 + 1;
        let failure = match build_request().send().await {
            Ok(resp) => {
                if resp.status().is_success() || !llm_provider_error_retryable_status(resp.status())
                {
                    return Ok((resp, attempt_number));
                }
                read_http_failure(resp).await
            }
            // Every send error retains the existing request retry budget, regardless
            // of whether the outer continuation policy considers its kind transient.
            Err(error) => classify_transport_failure(&error),
        };
        let Some(delay) = LLM_PROVIDER_RETRY_DELAYS.get(attempt_index).copied() else {
            let failure = failure.with_retry_exhausted(attempt_number);
            log_provider_failure(api_style, &failure, started);
            return Err(DenError::Provider(failure));
        };
        log_provider_failure(
            api_style,
            &failure.with_retry_attempts(attempt_number),
            started,
        );
        tracing::warn!(
            api_style = api_style.as_str(),
            attempt = attempt_number,
            retry_delay_ms = delay.as_millis(),
            "retrying LLM provider request"
        );
        sleep(delay).await;
    }
    unreachable!("bounded retry loop always returns")
}

async fn sleep_llm_provider_retry_delay(delay: Duration) {
    tokio::time::sleep(delay).await;
}

struct TimedLlmByteStream {
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    telemetry: Option<LlmRequestTelemetry>,
    model: String,
    api_style: LlmApiStyle,
    request_started_at: Instant,
    headers_received_at: Instant,
    first_byte_received_at: Option<Instant>,
    chunk_count: u64,
    byte_count: u64,
    completed: bool,
}

impl TimedLlmByteStream {
    fn new(
        response: Response,
        telemetry: Option<LlmRequestTelemetry>,
        model: String,
        api_style: LlmApiStyle,
        request_started_at: Instant,
        headers_received_at: Instant,
    ) -> Self {
        Self {
            inner: Box::pin(response.bytes_stream()),
            telemetry,
            model,
            api_style,
            request_started_at,
            headers_received_at,
            first_byte_received_at: None,
            chunk_count: 0,
            byte_count: 0,
            completed: false,
        }
    }

    fn log_summary(&self, status: &'static str, failure: Option<&ProviderFailure>) {
        tracing::trace!(
            status,
            failure_kind = failure.map(|failure| failure.kind().code()),
            transport_kind = failure.and_then(ProviderFailure::transport_kind).map(ProviderTransportKind::code),
            api_style = %self.api_style.as_str(),
            request_to_headers_ms = self.headers_received_at.duration_since(self.request_started_at).as_millis(),
            headers_to_first_byte_ms = self.first_byte_received_at.map(|at| at.duration_since(self.headers_received_at).as_millis()),
            request_to_first_byte_ms = self.first_byte_received_at.map(|at| at.duration_since(self.request_started_at).as_millis()),
            total_ms = self.headers_received_at.elapsed().as_millis() + self.headers_received_at.duration_since(self.request_started_at).as_millis(),
            chunk_count = self.chunk_count,
            byte_count = self.byte_count,
            "LLM stream timing summary"
        );
    }
}

impl Unpin for TimedLlmByteStream {}

impl Stream for TimedLlmByteStream {
    type Item = Result<bytes::Bytes, DenError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                if self.first_byte_received_at.is_none() {
                    let now = Instant::now();
                    self.first_byte_received_at = Some(now);
                    let telemetry = self.telemetry.as_ref();
                    tracing::trace!(
                        model = %self.model,
                        api_style = %self.api_style.as_str(),
                        request_id = telemetry.and_then(|t| t.request_id.as_deref()),
                        run_id = telemetry.and_then(|t| t.run_id.as_deref()),
                        session_id = telemetry.and_then(|t| t.session_id.as_deref()),
                        conversation_id = telemetry.and_then(|t| t.conversation_id.as_deref()),
                        headers_to_first_byte_ms = now.duration_since(self.headers_received_at).as_millis(),
                        request_to_first_byte_ms = now.duration_since(self.request_started_at).as_millis(),
                        "LLM first response byte received"
                    );
                }
                self.chunk_count = self.chunk_count.saturating_add(1);
                self.byte_count = self.byte_count.saturating_add(bytes.len() as u64);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                self.completed = true;
                let failure = classify_stream_read_failure(&err);
                self.log_summary("error", Some(&failure));
                Poll::Ready(Some(Err(DenError::Provider(failure))))
            }
            Poll::Ready(None) => {
                self.completed = true;
                self.log_summary("completed", None);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TimedLlmByteStream {
    fn drop(&mut self) {
        if !self.completed {
            self.log_summary("dropped", None);
        }
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    default_model: String,
}

impl LlmClient {
    pub fn new(config: &Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_mins(5))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client for llm");
        Self {
            http,
            base_url: config.llm_api_url.trim_end_matches('/').to_string(),
            default_model: normalize_llm_model_handle(&config.default_llm_model),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.base_url.is_empty()
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    pub fn resolve_model(&self, requested: Option<&str>) -> String {
        let raw = requested
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(&self.default_model);
        normalize_llm_model_handle(raw)
    }

    pub async fn chat_completions_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<Response, DenError> {
        if !self.is_enabled() {
            return Err(DenError::System(
                "LLM API is not configured (set LLM_API_URL or BIFROST_BASE_URL)".to_string(),
            ));
        }
        require_bifrost_virtual_key(request.telemetry.as_ref())?;
        let url = format!("{}/chat/completions", self.base_url);
        let message_chain_diagnostic = chat_message_chain_diagnostic(&request.messages);
        if message_chain_diagnostic
            .get("invalid")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            tracing::warn!(
                model = %request.model,
                request_id = request.telemetry.as_ref().and_then(|t| t.request_id.as_deref()),
                run_id = request.telemetry.as_ref().and_then(|t| t.run_id.as_deref()),
                session_id = request.telemetry.as_ref().and_then(|t| t.session_id.as_deref()),
                conversation_id = request.telemetry.as_ref().and_then(|t| t.conversation_id.as_deref()),
                diagnostic = %message_chain_diagnostic,
                "LLM chat/completions request preflight rejected invalid tool-call message chain"
            );
            return Err(DenError::System(format!(
                "LLM chat/completions request preflight rejected invalid tool-call message chain: {message_chain_diagnostic}"
            )));
        }
        tracing::trace!(
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            stream = request.stream,
            request_id = request.telemetry.as_ref().and_then(|t| t.request_id.as_deref()),
            run_id = request.telemetry.as_ref().and_then(|t| t.run_id.as_deref()),
            session_id = request.telemetry.as_ref().and_then(|t| t.session_id.as_deref()),
            conversation_id = request.telemetry.as_ref().and_then(|t| t.conversation_id.as_deref()),
            bear_id = request.telemetry.as_ref().and_then(|t| t.bear_id.as_deref()),
            stance = request.telemetry.as_ref().and_then(|t| t.stance.as_deref()),
            message_chain_diagnostic = %message_chain_diagnostic,
            "LLM chat/completions request starting"
        );
        let started = Instant::now();
        let body = request.to_body();
        let (resp, attempts) = send_llm_provider_request_with_retries(
            || {
                let mut req = self.http.post(&url).json(&body);
                req = apply_bears_telemetry_headers(req, request.telemetry.as_ref());
                req = apply_bifrost_virtual_key_header(req, request.telemetry.as_ref());
                apply_bifrost_session_cache_headers(req, request.telemetry.as_ref())
            },
            LlmApiStyle::ChatCompletionsStream,
            started,
        )
        .await?;
        let http_status = resp.status().as_u16();
        if !resp.status().is_success() {
            return self
                .handle_provider_http_failure(
                    resp,
                    attempts,
                    &url,
                    &body,
                    request.telemetry.as_ref(),
                    LlmApiStyle::ChatCompletionsStream,
                    started,
                )
                .await;
        }
        tracing::trace!(
            model = %request.model,
            http_status,
            duration_ms = started.elapsed().as_millis(),
            request_id = request.telemetry.as_ref().and_then(|t| t.request_id.as_deref()),
            run_id = request.telemetry.as_ref().and_then(|t| t.run_id.as_deref()),
            session_id = request.telemetry.as_ref().and_then(|t| t.session_id.as_deref()),
            conversation_id = request.telemetry.as_ref().and_then(|t| t.conversation_id.as_deref()),
            "LLM chat/completions response headers received"
        );
        Ok(resp)
    }

    pub async fn responses_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<Response, DenError> {
        if !self.is_enabled() {
            return Err(DenError::System(
                "LLM API is not configured (set LLM_API_URL or BIFROST_BASE_URL)".to_string(),
            ));
        }
        require_bifrost_virtual_key(request.telemetry.as_ref())?;
        let url = format!("{}/responses", self.base_url);
        let model_handle = DenModelHandle::normalize(&request.model);
        tracing::trace!(
            model_handle = %model_handle,
            api_style = %LlmApiStyle::ResponsesStream.as_str(),
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            stream = request.stream,
            request_id = request.telemetry.as_ref().and_then(|t| t.request_id.as_deref()),
            run_id = request.telemetry.as_ref().and_then(|t| t.run_id.as_deref()),
            session_id = request.telemetry.as_ref().and_then(|t| t.session_id.as_deref()),
            conversation_id = request.telemetry.as_ref().and_then(|t| t.conversation_id.as_deref()),
            "LLM responses request starting"
        );
        let started = Instant::now();
        let body = request.to_responses_body();
        let (resp, attempts) = send_llm_provider_request_with_retries(
            || {
                let mut req = self.http.post(&url).json(&body);
                req = apply_bears_telemetry_headers(req, request.telemetry.as_ref());
                req = apply_bifrost_virtual_key_header(req, request.telemetry.as_ref());
                apply_bifrost_session_cache_headers(req, request.telemetry.as_ref())
            },
            LlmApiStyle::ResponsesStream,
            started,
        )
        .await?;
        let http_status = resp.status().as_u16();
        if !resp.status().is_success() {
            return self
                .handle_provider_http_failure(
                    resp,
                    attempts,
                    &url,
                    &body,
                    request.telemetry.as_ref(),
                    LlmApiStyle::ResponsesStream,
                    started,
                )
                .await;
        }
        tracing::trace!(
            model_handle = %model_handle,
            api_style = %LlmApiStyle::ResponsesStream.as_str(),
            http_status,
            duration_ms = started.elapsed().as_millis(),
            request_id = request.telemetry.as_ref().and_then(|t| t.request_id.as_deref()),
            run_id = request.telemetry.as_ref().and_then(|t| t.run_id.as_deref()),
            session_id = request.telemetry.as_ref().and_then(|t| t.session_id.as_deref()),
            conversation_id = request.telemetry.as_ref().and_then(|t| t.conversation_id.as_deref()),
            "LLM responses headers received"
        );
        Ok(resp)
    }

    async fn handle_provider_http_failure(
        &self,
        response: Response,
        attempts: u32,
        url: &str,
        body: &Value,
        telemetry: Option<&LlmRequestTelemetry>,
        api_style: LlmApiStyle,
        started: Instant,
    ) -> Result<Response, DenError> {
        let failure = read_http_failure(response)
            .await
            .with_retry_attempts(attempts);
        log_provider_failure(api_style, &failure, started);
        if failure.kind() != ProviderFailureKind::KeySelection {
            return Err(DenError::Provider(failure));
        }
        let original_status = failure.http_status().expect("HTTP failure has a status");
        tracing::warn!(
            api_style = api_style.as_str(),
            http_status = original_status.as_u16(),
            attempt = attempts + 1,
            "retrying LLM provider request without Bifrost session/cache headers"
        );
        let mut retry = self.http.post(url).json(body);
        retry = apply_bears_telemetry_headers(retry, telemetry);
        retry = apply_bifrost_virtual_key_header(retry, telemetry);
        let final_failure = match retry.send().await {
            Ok(response) if response.status().is_success() => {
                tracing::debug!(
                    api_style = api_style.as_str(),
                    http_status = response.status().as_u16(),
                    attempt = attempts + 1,
                    "LLM provider request without Bifrost session/cache headers succeeded"
                );
                return Ok(response);
            }
            Ok(response) => read_http_failure(response).await,
            Err(error) => classify_transport_failure(&error),
        }
        .with_retry_attempts(attempts + 1)
        .with_key_selection_fallback(original_status);
        log_provider_failure(api_style, &final_failure, started);
        Err(DenError::Provider(final_failure))
    }

    async fn byte_stream_for_api_style(
        &self,
        request: &ChatCompletionRequest,
        api_style: LlmApiStyle,
    ) -> Result<impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin, DenError> {
        let started = Instant::now();
        let resp = match api_style {
            LlmApiStyle::ChatCompletionsStream => self.chat_completions_stream(request).await?,
            LlmApiStyle::ResponsesStream => self.responses_stream(request).await?,
        };
        let headers_received_at = Instant::now();
        Ok(TimedLlmByteStream::new(
            resp,
            request.telemetry.clone(),
            request.model.clone(),
            api_style,
            started,
            headers_received_at,
        ))
    }

    pub async fn chat_completions_byte_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin, DenError> {
        self.byte_stream_for_api_style(request, LlmApiStyle::ChatCompletionsStream)
            .await
    }

    pub async fn responses_byte_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin, DenError> {
        self.byte_stream_for_api_style(request, LlmApiStyle::ResponsesStream)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(id: &str) -> ChatToolCall {
        ChatToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: ChatToolCallFunction {
                name: "memory_read".to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    #[test]
    fn model_identity_wrappers_separate_den_handle_from_provider_id() {
        let handle = DenModelHandle::normalize("gpt-5.5");
        let provider = ProviderModelId::for_den_handle(&handle);

        assert_eq!(handle.as_str(), "openai/gpt-5.5");
        assert_eq!(provider.as_str(), "gpt-5.5");
    }

    #[test]
    fn telemetry_exposes_content_free_operation() {
        let telemetry = LlmRequestTelemetry {
            request_id: None,
            run_id: None,
            session_id: None,
            conversation_id: None,
            bear_id: None,
            stance: None,
            operation: Some(LlmOperation::AgentTurn),
            bifrost_virtual_key: None,
        };

        assert_eq!(telemetry.field("operation"), Some("agent_turn"));
        assert_eq!(telemetry.field("unknown"), None);
    }

    #[test]
    fn telemetry_transport_emits_only_the_allowlisted_operation_label() {
        let telemetry = LlmRequestTelemetry {
            operation: Some(LlmOperation::BackgroundWork),
            ..Default::default()
        };
        let request = apply_bears_telemetry_headers(
            reqwest::Client::new().post("http://example.invalid/v1/responses"),
            Some(&telemetry),
        )
        .build()
        .expect("valid local request");

        assert_eq!(
            request
                .headers()
                .get("x-bears-operation")
                .and_then(|v| v.to_str().ok()),
            Some("background_work")
        );
    }
    #[test]
    fn preferred_api_style_uses_explicit_catalog_support() {
        assert_eq!(
            preferred_api_style_for_model_with_catalog_support("vendor/style-yes", Some(true)),
            LlmApiStyle::ResponsesStream
        );
        assert_eq!(
            preferred_api_style_for_model_with_catalog_support("vendor/style-no", Some(false)),
            LlmApiStyle::ChatCompletionsStream
        );
    }

    #[test]
    fn preferred_api_style_falls_back_to_literal_before_catalog() {
        // A handle the catalog has not advertised falls back to the static rule.
        assert_eq!(
            preferred_api_style_for_model("vendor/never-observed-5.5-like"),
            LlmApiStyle::ChatCompletionsStream
        );
        assert_eq!(
            preferred_api_style_for_model("gpt-5.5"),
            LlmApiStyle::ResponsesStream
        );
    }

    #[test]
    fn responses_body_sends_provider_qualified_handle() {
        let request = ChatCompletionRequest {
            model: "openai/gpt-5.5".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some("ping".to_string()),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }],
            tools: Vec::new(),
            stream: true,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            thinking_effort: None,
            telemetry: None,
        };

        let body = request.to_responses_body();

        // Bifrost `/v1/responses` requires the provider-qualified handle, not a bare id.
        assert_eq!(body["model"], "openai/gpt-5.5");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn thinking_effort_serializes_for_chat_and_responses_requests() {
        let request = ChatCompletionRequest {
            model: "openai/gpt-5.5".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some("ping".to_string()),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }],
            tools: Vec::new(),
            stream: true,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            thinking_effort: Some(ThinkingEffort::High),
            telemetry: None,
        };

        let chat_body = request.to_body();
        assert_eq!(chat_body["reasoning_effort"], "high");

        let responses_body = request.to_responses_body();
        assert_eq!(responses_body["reasoning"]["effort"], "high");
    }

    #[test]
    fn retries_provider_hiccup_statuses() {
        assert!(llm_provider_error_retryable_status(
            StatusCode::REQUEST_TIMEOUT
        ));
        assert!(llm_provider_error_retryable_status(
            StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(llm_provider_error_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!llm_provider_error_retryable_status(
            StatusCode::BAD_REQUEST
        ));
    }

    #[test]
    fn chat_message_chain_diagnostic_accepts_complete_tool_chain() {
        let diagnostic = chat_message_chain_diagnostic(&[
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![tool_call("call_1")]),
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("{}".to_string()),
                tool_call_id: Some("call_1".to_string()),
                name: Some("memory_read".to_string()),
                tool_calls: None,
            },
        ]);

        assert_eq!(diagnostic["invalid"], false);
        assert_eq!(diagnostic["assistant_tool_call_blocks"], 1);
        assert_eq!(diagnostic["tool_messages"], 1);
    }

    #[test]
    fn chat_message_chain_diagnostic_rejects_unresolved_tool_call() {
        let diagnostic = chat_message_chain_diagnostic(&[ChatMessage {
            role: "assistant".to_string(),
            content: None,
            tool_call_id: None,
            name: None,
            tool_calls: Some(vec![tool_call("call_1")]),
        }]);

        assert_eq!(diagnostic["invalid"], true);
        assert_eq!(diagnostic["unresolved_tool_call_ids"][0], "call_1");
    }

    #[test]
    fn chat_message_chain_diagnostic_rejects_orphan_tool_message() {
        let diagnostic = chat_message_chain_diagnostic(&[ChatMessage {
            role: "tool".to_string(),
            content: Some("{}".to_string()),
            tool_call_id: Some("call_orphan".to_string()),
            name: Some("memory_read".to_string()),
            tool_calls: None,
        }]);

        assert_eq!(diagnostic["invalid"], true);
        assert_eq!(diagnostic["orphan_tool_messages"][0], "call_orphan");
    }
}
