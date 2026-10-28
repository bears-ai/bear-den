//! Wraps a chat SSE byte stream with TTFB logging and terminal outcome metrics.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use futures::ready;
use futures::{Future, Stream};
use sqlx::PgPool;
use uuid::Uuid;

use den_service::conversation::persistence::{append_message, ensure_conversation_for_external_id};

use super::metrics;

/// Ephemeral run-progress lines shown during live streaming; must not be persisted or coalesced into history.
pub(crate) const EPHEMERAL_PROGRESS_STATUSES: &[&str] = &[
    "Thinking…",
    "Still working…",
    "Running tools…",
    "Continuing…",
];

pub(crate) fn is_ephemeral_progress_status(text: &str) -> bool {
    EPHEMERAL_PROGRESS_STATUSES.contains(&text)
}

/// Strips trailing ephemeral status suffixes from polluted persisted rows (e.g. `HelloThinking…`).
///
/// The loop terminates because each successful iteration removes one known non-empty suffix; once
/// no suffix matches the function returns the current text.
pub(crate) fn strip_ephemeral_status_suffixes(text: &str) -> String {
    let mut out = text.to_string();
    loop {
        let trimmed = out.trim_end();
        let mut stripped = false;
        for suffix in EPHEMERAL_PROGRESS_STATUSES {
            if let Some(prefix) = trimmed.strip_suffix(suffix) {
                out = prefix.trim_end().to_string();
                stripped = true;
                break;
            }
        }
        if !stripped {
            return out;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    Ok,
    Empty,
    ProxyError,
}

/// Streams bytes to the browser while recording observability.
pub struct ChatSseProxyStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    request_id: Uuid,
    user_id: i32,
    bear_id: Uuid,
    conversation_id: String,
    started_at: Instant,
    first_byte_at: Option<Instant>,
    total_bytes: usize,
    terminal: Option<Terminal>,
}

impl ChatSseProxyStream {
    pub fn new(
        inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        request_id: Uuid,
        user_id: i32,
        bear_id: Uuid,
        conversation_id: String,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            request_id,
            user_id,
            bear_id,
            conversation_id,
            started_at: Instant::now(),
            first_byte_at: None,
            total_bytes: 0,
            terminal: None,
        }
    }

    fn record_terminal(&mut self, t: Terminal) {
        if self.terminal.is_some() {
            return;
        }
        self.terminal = Some(t);
        match t {
            Terminal::Ok => metrics::chat_send_finished_ok(),
            Terminal::Empty => metrics::chat_send_finished_empty(),
            Terminal::ProxyError => metrics::chat_send_finished_proxy_error(),
        }
    }

    fn log_and_record_finish(&mut self, t: Terminal) {
        self.record_terminal(t);
        let elapsed = self.started_at.elapsed();
        let ttfb_ms = self.first_byte_at.map(|fb| {
            fb.duration_since(self.started_at)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        });
        match t {
            Terminal::Ok => {
                tracing::info!(
                    request_id = %self.request_id,
                    user_id = %self.user_id,
                    bear_id = %self.bear_id,
                    conversation_id = %self.conversation_id,
                    total_bytes = self.total_bytes,
                    ttfb_ms,
                    elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                    "chat_send sse stream finished ok"
                );
            }
            Terminal::Empty => {
                tracing::warn!(
                    request_id = %self.request_id,
                    user_id = %self.user_id,
                    bear_id = %self.bear_id,
                    conversation_id = %self.conversation_id,
                    elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                    "chat_send sse stream ended with zero bytes from upstream"
                );
            }
            Terminal::ProxyError => {}
        }
    }
}

impl Drop for ChatSseProxyStream {
    fn drop(&mut self) {
        if self.terminal.is_some() {
            return;
        }
        metrics::chat_send_finished_proxy_error();
        tracing::warn!(
            request_id = %self.request_id,
            user_id = %self.user_id,
            bear_id = %self.bear_id,
            conversation_id = %self.conversation_id,
            total_bytes = self.total_bytes,
            "chat_send sse proxy stream dropped before terminal poll (client disconnect or task cancelled)"
        );
    }
}

fn stream_event_inner(event: &serde_json::Value) -> &serde_json::Value {
    match event.get("contents") {
        Some(contents) if contents.get("message_type").is_some() => contents,
        _ => event,
    }
}

fn stream_event_kind(event: &serde_json::Value) -> &str {
    let inner = stream_event_inner(event);
    inner
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| inner.get("message_type").and_then(|v| v.as_str()))
        .or_else(|| event.get("type").and_then(|v| v.as_str()))
        .or_else(|| event.get("message_type").and_then(|v| v.as_str()))
        .unwrap_or("")
}

fn content_value_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| match part {
                    serde_json::Value::String(s) => Some(s.as_str()),
                    serde_json::Value::Object(obj) => obj.get("text").and_then(|v| v.as_str()),
                    _ => None,
                })
                .collect::<String>();
            Some(text).filter(|s| !s.is_empty())
        }
        serde_json::Value::Object(obj) => {
            obj.get("text").and_then(|v| v.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

fn stream_event_text(event: &serde_json::Value) -> Option<String> {
    event
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            event
                .get("reasoning")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| event.get("content").and_then(content_value_text))
}

fn rich_event_status_text(event: &serde_json::Value) -> Option<String> {
    let ty = stream_event_kind(event);
    let tool = event.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
    let name = event
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("sub-agent");
    let summary = event.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    let text = match ty {
        "server_tool_started" => format!("Started {tool}"),
        "server_tool_finished" => {
            if summary.is_empty() {
                format!("Finished {tool}")
            } else {
                format!("Finished {tool}: {summary}")
            }
        }
        "subagent_started" => format!("Started sub-agent {name}"),
        "subagent_finished" => {
            if summary.is_empty() {
                format!("Finished sub-agent {name}")
            } else {
                format!("Finished sub-agent {name}: {summary}")
            }
        }
        "memory_update_recorded" => {
            if summary.is_empty() {
                "Recorded memory update".to_string()
            } else {
                format!("Recorded memory update: {summary}")
            }
        }
        _ => return None,
    };
    Some(text)
}

#[derive(Default)]
struct PendingConversationPersistence {
    assistant_text: String,
    /// User-visible status lines (tool started/finished, progress) shown during the live stream.
    status_text: String,
    error_text: String,
    reasoning_text: String,
    resolved_conversation_id: Option<String>,
    workflow_events: Vec<serde_json::Value>,
}

impl PendingConversationPersistence {
    fn ingest(&mut self, event: &serde_json::Value) {
        let inner = stream_event_inner(event);
        match stream_event_kind(event) {
            "assistant_delta" | "assistant_message" => {
                if let Some(text) = stream_event_text(inner) {
                    self.assistant_text.push_str(&text);
                }
            }
            "status_progress" => {}
            "reasoning_delta" | "reasoning_message" => {
                if let Some(text) = stream_event_text(inner) {
                    self.reasoning_text.push_str(&text);
                }
            }
            "error" | "error_message" => {
                if let Some(message) = inner.get("message").and_then(|v| v.as_str()) {
                    if !self.error_text.is_empty() {
                        self.error_text.push_str("\n\n");
                    }
                    self.error_text.push_str(message);
                }
            }
            "server_tool_started"
            | "server_tool_finished"
            | "subagent_started"
            | "subagent_finished"
            | "memory_update_recorded" => {
                if let Some(text) = rich_event_status_text(inner) {
                    if !self.status_text.is_empty() {
                        self.status_text.push('\n');
                    }
                    self.status_text.push_str(&text);
                }
            }
            "conversation_resolved" => {
                if let Some(conversation_id) = inner.get("conversation_id").and_then(|v| v.as_str())
                {
                    self.resolved_conversation_id = Some(conversation_id.to_string());
                }
                self.workflow_events.push(inner.clone());
            }
            _ => {}
        }
    }

    fn has_flushable_content(&self) -> bool {
        !self.assistant_text.trim().is_empty()
            || !self.status_text.trim().is_empty()
            || !self.error_text.trim().is_empty()
            || !self.reasoning_text.trim().is_empty()
            || !self.workflow_events.is_empty()
    }

    async fn flush(
        self,
        pool: &PgPool,
        bear_id: Uuid,
        user_id: i32,
        conversation_id: &str,
        request_id: Uuid,
        interrupted: bool,
    ) {
        let external_conversation_id = self
            .resolved_conversation_id
            .as_deref()
            .unwrap_or(conversation_id);
        let canonical = match ensure_conversation_for_external_id(
            pool,
            bear_id,
            Some(user_id),
            external_conversation_id,
            None,
            None,
        )
        .await
        {
            Ok(conversation) => conversation,
            Err(err) => {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to ensure canonical web chat conversation during flush");
                return;
            }
        };

        if !self.reasoning_text.trim().is_empty() {
            if let Err(err) = append_message(
                pool,
                canonical.id,
                &den_service::conversation::message_types::ConversationMessageWrite::assistant_turn(
                    &self.reasoning_text,
                    serde_json::json!({
                        "type": "reasoning_delta_coalesced",
                        "text": self.reasoning_text,
                        "request_id": request_id.to_string(),
                        "interrupted": interrupted,
                    }),
                )
                .with_source_event_id(Some(format!("web-chat-reasoning:{request_id}"))),
            )
            .await
            {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to append canonical web chat reasoning");
            }
        }

        if !self.status_text.trim().is_empty() {
            if let Err(err) = append_message(
                pool,
                canonical.id,
                &den_service::conversation::message_types::ConversationMessageWrite::assistant_turn(
                    &self.status_text,
                    serde_json::json!({
                        "type": "status_message_coalesced",
                        "text": self.status_text,
                        "request_id": request_id.to_string(),
                        "interrupted": interrupted,
                    }),
                )
                .with_source_event_id(Some(format!("web-chat-status:{request_id}"))),
            )
            .await
            {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to append canonical web chat status");
            }
        }

        if !self.assistant_text.trim().is_empty() {
            if let Err(err) = append_message(
                pool,
                canonical.id,
                &den_service::conversation::message_types::ConversationMessageWrite::assistant_turn(
                    &self.assistant_text,
                    serde_json::json!({
                        "type": "assistant_delta_coalesced",
                        "text": self.assistant_text,
                        "request_id": request_id.to_string(),
                        "interrupted": interrupted,
                    }),
                )
                .with_source_event_id(Some(format!("web-chat-assistant:{request_id}"))),
            )
            .await
            {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to append canonical web chat assistant output");
            }
        }

        if !self.error_text.trim().is_empty() {
            if let Err(err) = append_message(
                pool,
                canonical.id,
                &den_service::conversation::message_types::ConversationMessageWrite::assistant_turn(
                    &self.error_text,
                    serde_json::json!({
                        "type": "error_message_coalesced",
                        "text": self.error_text,
                        "request_id": request_id.to_string(),
                        "interrupted": interrupted,
                    }),
                )
                .with_source_event_id(Some(format!("web-chat-error:{request_id}"))),
            )
            .await
            {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to append canonical web chat error");
            }
        }

        for event in self.workflow_events {
            if let Err(err) = append_message(
                pool,
                canonical.id,
                &den_service::conversation::message_types::ConversationMessageWrite::workflow_diagnostic(
                    "Conversation resolved",
                    event,
                ),
            )
            .await
            {
                tracing::warn!(bear_id = %bear_id, conversation_id = conversation_id, resolved_conversation_id = external_conversation_id, error = %err, "failed to append canonical web chat workflow event");
            }
        }
    }
}

pub(crate) fn bear_channel_sse_bytes(event: &serde_json::Value) -> Bytes {
    Bytes::from(format!("data: {}\n\n", event))
}

fn empty_terminal_bear_channel_error(request_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "type": "error",
        "message": "No response received",
        "detail": "The assistant did not return any text. You can retry, or share the reference below if the problem continues.",
        "error_type": "stream_empty_terminal",
        "request_id": request_id.to_string(),
    })
}

fn incomplete_terminal_bear_channel_error(request_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "type": "error",
        "message": "Reply incomplete",
        "detail": "The assistant was still working but did not finish a reply. You can retry, or share the reference below when asking for help.",
        "error_type": "stream_incomplete_terminal",
        "request_id": request_id.to_string(),
    })
}

fn browser_terminal_error(_request_id: Uuid, event: &serde_json::Value) -> Bytes {
    let mapped = serde_json::json!({
        "message_type": "error_message",
        "message": event.get("message").and_then(|v| v.as_str()).unwrap_or("Upstream error"),
        "detail": event.get("detail").and_then(|v| v.as_str()),
        "error_type": event.get("error_type").and_then(|v| v.as_str()),
        "support_ref": event.get("request_id").and_then(|v| v.as_str()),
    });
    bear_channel_sse_bytes(&mapped)
}

fn browser_empty_terminal_error(request_id: Uuid) -> Bytes {
    browser_terminal_error(request_id, &empty_terminal_bear_channel_error(request_id))
}

fn browser_incomplete_terminal_error(request_id: Uuid) -> Bytes {
    browser_terminal_error(
        request_id,
        &incomplete_terminal_bear_channel_error(request_id),
    )
}

fn persistence_has_substantive_reply(persistence: &PendingConversationPersistence) -> bool {
    !persistence.assistant_text.trim().is_empty() || !persistence.error_text.trim().is_empty()
}

fn persistence_has_stream_chrome(
    persistence: &PendingConversationPersistence,
    browser_bytes: usize,
) -> bool {
    browser_bytes > 0
        || !persistence.status_text.trim().is_empty()
        || !persistence.reasoning_text.trim().is_empty()
}

fn bear_channel_event_to_deep_chat_sse(event: &serde_json::Value) -> Option<Bytes> {
    let inner = stream_event_inner(event);
    let ty = stream_event_kind(event);
    let mapped = match ty {
        "assistant_delta" | "assistant_message" => {
            let raw = stream_event_text(inner).unwrap_or_default();
            let content = strip_ephemeral_status_suffixes(&raw);
            if content.trim().is_empty() {
                return None;
            }
            serde_json::json!({
                "message_type": "assistant_message",
                "content": content,
                "id": inner.get("id").and_then(|v| v.as_str()),
            })
        }
        "reasoning_delta" | "reasoning_message" => {
            let text = stream_event_text(inner).unwrap_or_default();
            serde_json::json!({
                "message_type": "reasoning_message",
                "reasoning": text,
                "content": text,
                "id": inner.get("id").and_then(|v| v.as_str()),
            })
        }
        "status_progress" => {
            let text = stream_event_text(inner).unwrap_or_default();
            serde_json::json!({
                "message_type": "status_message",
                "content": text,
                "status_type": "run_progress",
            })
        }
        "status_message" => serde_json::json!({
            "message_type": "status_message",
            "content": stream_event_text(inner).unwrap_or_default(),
            "status_type": inner.get("status_type").and_then(|v| v.as_str()),
        }),
        "error" | "error_message" => serde_json::json!({
            "message_type": "error_message",
            "message": inner.get("message").and_then(|v| v.as_str()).unwrap_or("Upstream error"),
            "detail": inner.get("detail").and_then(|v| v.as_str()),
            "error_type": inner.get("error_type").and_then(|v| v.as_str()),
            "support_ref": inner
                .get("support_ref")
                .or_else(|| inner.get("request_id"))
                .and_then(|v| v.as_str()),
            "context": inner.get("context"),
        }),
        "conversation_resolved" => serde_json::json!({
            "message_type": "conversation_resolved",
            "conversation_id": inner.get("conversation_id").and_then(|v| v.as_str()),
        }),
        // `done` / `stop_reason` are terminal control metadata, not user-visible status.
        "done" | "stop_reason" => return None,
        "runtime_card" => serde_json::json!({
            "message_type": "runtime_card",
            "card_kind": inner.get("card_kind").and_then(|v| v.as_str()).unwrap_or("runtime_lifecycle"),
            "label": inner.get("label").and_then(|v| v.as_str()).unwrap_or("Den runtime activity"),
            "source": inner.get("source").and_then(|v| v.as_str()).unwrap_or("den_runtime"),
            "tool": inner.get("tool").cloned(),
            "run_id": inner.get("run_id").cloned(),
            "invocation_id": inner.get("invocation_id").cloned(),
            "ordering": inner.get("ordering").cloned(),
            "delivery": inner.get("delivery").cloned(),
            "redaction": inner.get("redaction").cloned(),
            "payload": inner.get("payload").cloned(),
        }),
        "server_tool_started"
        | "server_tool_finished"
        | "subagent_started"
        | "subagent_finished"
        | "memory_update_recorded" => {
            let text = rich_event_status_text(inner)?;
            serde_json::json!({
                "message_type": "status_message",
                "content": text,
                "status_type": ty,
            })
        }
        _ => return None,
    };
    Some(bear_channel_sse_bytes(&mapped))
}

/// Maps a single assistant text payload into Deep Chat SSE bytes for direct fast-path responses
/// (capabilities list, title set, etc.) that bypass [`BearChannelSseProxyStream`].
pub(crate) fn deep_chat_sse_body_for_assistant_text(text: &str) -> String {
    let event = serde_json::json!({
        "type": "assistant_delta",
        "text": text,
    });
    bear_channel_event_to_deep_chat_sse(&event)
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap_or_default())
        .unwrap_or_default()
}

/// Streams `bear_channel` SSE from the native runtime upstream to the browser after translating channel events
/// into the existing Deep Chat / runtime-shaped SSE payloads consumed by `bear_chat.html`.
pub struct BearChannelSseProxyStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    request_id: Uuid,
    user_id: i32,
    bear_id: Uuid,
    conversation_id: String,
    pool: PgPool,
    started_at: Instant,
    first_byte_at: Option<Instant>,
    total_bytes: usize,
    terminal: Option<Terminal>,
    buffer: Vec<u8>,
    pending: VecDeque<Bytes>,
    persistence: PendingConversationPersistence,
    flush_started: bool,
    flush_task: Option<tokio::task::JoinHandle<()>>,
    empty_terminal_error_emitted: bool,
    browser_visible_assistant_emitted: bool,
    source_lease: Option<Arc<den_service::tool_turns::ConversationTurnLease>>,
}

impl BearChannelSseProxyStream {
    pub fn new(
        inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        request_id: Uuid,
        user_id: i32,
        bear_id: Uuid,
        conversation_id: String,
        pool: PgPool,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            request_id,
            user_id,
            bear_id,
            conversation_id,
            pool,
            started_at: Instant::now(),
            first_byte_at: None,
            total_bytes: 0,
            terminal: None,
            buffer: Vec::new(),
            pending: VecDeque::new(),
            persistence: PendingConversationPersistence::default(),
            flush_started: false,
            flush_task: None,
            empty_terminal_error_emitted: false,
            browser_visible_assistant_emitted: false,
            source_lease: None,
        }
    }

    pub(crate) fn with_source_lease(
        mut self,
        lease: Arc<den_service::tool_turns::ConversationTurnLease>,
    ) -> Self {
        self.source_lease = Some(lease);
        self
    }

    fn queue_terminal_error_if_needed(&mut self) -> bool {
        if self.empty_terminal_error_emitted
            || self.browser_visible_assistant_emitted
            || persistence_has_substantive_reply(&self.persistence)
        {
            return false;
        }
        self.empty_terminal_error_emitted = true;
        let chrome_only = persistence_has_stream_chrome(&self.persistence, self.total_bytes);
        let error_event = if chrome_only {
            incomplete_terminal_bear_channel_error(self.request_id)
        } else {
            empty_terminal_bear_channel_error(self.request_id)
        };
        self.persistence.ingest(&error_event);
        self.pending.push_back(if chrome_only {
            browser_incomplete_terminal_error(self.request_id)
        } else {
            browser_empty_terminal_error(self.request_id)
        });
        true
    }

    /// Persist browser-visible stream content. Runs on normal stream end and on abrupt drop.
    fn schedule_persistence_flush(&mut self, interrupted: bool) {
        if self.flush_started || !self.persistence.has_flushable_content() {
            return;
        }
        self.flush_started = true;
        let persistence = std::mem::take(&mut self.persistence);
        let pool = self.pool.clone();
        let bear_id = self.bear_id;
        let user_id = self.user_id;
        let conversation_id = self.conversation_id.clone();
        let request_id = self.request_id;
        let source_lease = self.source_lease.clone();
        self.flush_task = Some(tokio::spawn(async move {
            // A disconnected browser may drop this proxy while the flush is
            // still running. Retain exclusion until its last canonical write.
            let _source_lease = source_lease;
            persistence
                .flush(
                    &pool,
                    bear_id,
                    user_id,
                    &conversation_id,
                    request_id,
                    interrupted,
                )
                .await;
        }));
    }

    fn record_terminal(&mut self, t: Terminal) {
        if self.terminal.is_some() {
            return;
        }
        self.terminal = Some(t);
        match t {
            Terminal::Ok => metrics::chat_send_finished_ok(),
            Terminal::Empty => metrics::chat_send_finished_empty(),
            Terminal::ProxyError => metrics::chat_send_finished_proxy_error(),
        }
    }

    fn log_and_record_finish(&mut self, t: Terminal) {
        self.record_terminal(t);
        let elapsed = self.started_at.elapsed();
        let ttfb_ms = self.first_byte_at.map(|fb| {
            fb.duration_since(self.started_at)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        });
        match t {
            Terminal::Ok => tracing::info!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                bear_id = %self.bear_id,
                conversation_id = %self.conversation_id,
                total_bytes = self.total_bytes,
                ttfb_ms,
                elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                "chat_send bear_channel stream finished ok"
            ),
            Terminal::Empty => tracing::warn!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                bear_id = %self.bear_id,
                conversation_id = %self.conversation_id,
                browser_bytes = self.total_bytes,
                empty_terminal_error_emitted = self.empty_terminal_error_emitted,
                elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                "chat_send bear_channel stream ended with zero browser-compatible bytes"
            ),
            Terminal::ProxyError => {}
        }
    }
}

impl Drop for BearChannelSseProxyStream {
    fn drop(&mut self) {
        if self.terminal.is_none() {
            self.schedule_persistence_flush(true);
        }
        if self.terminal.is_some() {
            return;
        }
        metrics::chat_send_finished_proxy_error();
        metrics::record_chat_send_dropped(self.total_bytes > 0);
        let elapsed_ms = self
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let ttfb_ms = self.first_byte_at.map(|fb| {
            fb.duration_since(self.started_at)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        });
        tracing::warn!(
            request_id = %self.request_id,
            user_id = %self.user_id,
            bear_id = %self.bear_id,
            conversation_id = %self.conversation_id,
            browser_bytes = self.total_bytes,
            ttfb_ms,
            elapsed_ms,
            empty_terminal_error_emitted = self.empty_terminal_error_emitted,
            flush_scheduled = self.flush_started,
            "chat_send bear_channel proxy stream dropped before terminal poll (client disconnect or task cancelled)"
        );
    }
}

impl Stream for BearChannelSseProxyStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if let Some(bytes) = this.pending.pop_front() {
            if this.total_bytes == 0 {
                let ttfb_ms = this
                    .started_at
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                metrics::record_chat_send_ttfb_ms(ttfb_ms);
                if this.first_byte_at.is_none() {
                    this.first_byte_at = Some(Instant::now());
                    tracing::info!(
                        request_id = %this.request_id,
                        user_id = %this.user_id,
                        bear_id = %this.bear_id,
                        conversation_id = %this.conversation_id,
                        ttfb_ms,
                        "chat_send first browser-visible byte"
                    );
                }
            }
            this.total_bytes += bytes.len();
            return Poll::Ready(Some(Ok(bytes)));
        }

        loop {
            match ready!(this.inner.as_mut().poll_next(cx)) {
                Some(Ok(chunk)) => {
                    if this.first_byte_at.is_none() {
                        this.first_byte_at = Some(Instant::now());
                        let ttfb = this.started_at.elapsed();
                        tracing::info!(
                            request_id = %this.request_id,
                            user_id = %this.user_id,
                            bear_id = %this.bear_id,
                            conversation_id = %this.conversation_id,
                            ttfb_ms = ttfb.as_millis().min(u128::from(u64::MAX)) as u64,
                            "chat_send first bear_channel byte from native upstream"
                        );
                    }
                    this.buffer.extend_from_slice(&chunk);
                    while let Some(pos) = this.buffer.windows(2).position(|w| w == b"\n\n") {
                        let frame: Vec<u8> = this.buffer.drain(..pos + 2).collect();
                        let text = String::from_utf8_lossy(&frame);
                        if text
                            .lines()
                            .all(|line| line.is_empty() || line.starts_with(':'))
                        {
                            this.pending.push_back(Bytes::from(frame));
                            continue;
                        }
                        for line in text.lines() {
                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data.is_empty() || data == "[DONE]" {
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                                this.persistence.ingest(&value);
                                if let Some(bytes) = bear_channel_event_to_deep_chat_sse(&value) {
                                    if matches!(
                                        stream_event_kind(&value),
                                        "assistant_delta" | "assistant_message"
                                    ) {
                                        this.browser_visible_assistant_emitted = true;
                                    }
                                    this.pending.push_back(bytes);
                                }
                            }
                        }
                    }
                    if let Some(bytes) = this.pending.pop_front() {
                        this.total_bytes += bytes.len();
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Some(Err(e)) => {
                    this.schedule_persistence_flush(true);
                    this.log_and_record_finish(Terminal::ProxyError);
                    tracing::error!(
                        request_id = %this.request_id,
                        user_id = %this.user_id,
                        bear_id = %this.bear_id,
                        conversation_id = %this.conversation_id,
                        error = %e,
                        total_bytes = this.total_bytes,
                        "chat_send bear_channel proxy chunk error from native upstream"
                    );
                    return Poll::Ready(Some(Err(std::io::Error::other(e.to_string()))));
                }
                None => {
                    if !this.buffer.is_empty() {
                        let frame = std::mem::take(&mut this.buffer);
                        let text = String::from_utf8_lossy(&frame);
                        for line in text.lines() {
                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data.is_empty() || data == "[DONE]" {
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                                this.persistence.ingest(&value);
                                if let Some(bytes) = bear_channel_event_to_deep_chat_sse(&value) {
                                    if matches!(
                                        stream_event_kind(&value),
                                        "assistant_delta" | "assistant_message"
                                    ) {
                                        this.browser_visible_assistant_emitted = true;
                                    }
                                    this.pending.push_back(bytes);
                                }
                            }
                        }
                        if let Some(bytes) = this.pending.pop_front() {
                            this.total_bytes += bytes.len();
                            return Poll::Ready(Some(Ok(bytes)));
                        }
                    }
                    if this.terminal.is_some() {
                        return Poll::Ready(None);
                    }
                    if this.queue_terminal_error_if_needed() {
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    if !this.pending.is_empty() {
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    if !this.flush_started {
                        this.schedule_persistence_flush(false);
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    if let Some(task) = this.flush_task.as_mut() {
                        match Pin::new(task).poll(cx) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(Ok(())) | Poll::Ready(Err(_)) => {
                                this.flush_task = None;
                            }
                        }
                    }
                    let outcome = if this.empty_terminal_error_emitted
                        || !persistence_has_substantive_reply(&this.persistence)
                            && this.total_bytes == 0
                    {
                        Terminal::Empty
                    } else {
                        Terminal::Ok
                    };
                    this.log_and_record_finish(outcome);
                    return Poll::Ready(None);
                }
            }
        }
    }
}

impl Stream for ChatSseProxyStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        match ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(chunk)) => {
                if this.first_byte_at.is_none() {
                    this.first_byte_at = Some(Instant::now());
                    let ttfb = this.started_at.elapsed();
                    tracing::info!(
                        request_id = %this.request_id,
                        user_id = %this.user_id,
                        bear_id = %this.bear_id,
                        conversation_id = %this.conversation_id,
                        ttfb_ms = ttfb.as_millis().min(u128::from(u64::MAX)) as u64,
                        "chat_send first byte from upstream"
                    );
                }
                this.total_bytes += chunk.len();
                Poll::Ready(Some(Ok(chunk)))
            }
            Some(Err(e)) => {
                this.log_and_record_finish(Terminal::ProxyError);
                tracing::error!(
                    request_id = %this.request_id,
                    user_id = %this.user_id,
                    bear_id = %this.bear_id,
                    conversation_id = %this.conversation_id,
                    error = %e,
                    total_bytes = this.total_bytes,
                    "chat_send sse proxy chunk error from upstream"
                );
                Poll::Ready(Some(Err(std::io::Error::other(e.to_string()))))
            }
            None => {
                if this.terminal.is_some() {
                    // Inner may emit Err then None; terminal already recorded.
                    return Poll::Ready(None);
                }
                let outcome = if this.total_bytes == 0 {
                    Terminal::Empty
                } else {
                    Terminal::Ok
                };
                this.log_and_record_finish(outcome);
                Poll::Ready(None)
            }
        }
    }
}

#[cfg(test)]
mod tests;
