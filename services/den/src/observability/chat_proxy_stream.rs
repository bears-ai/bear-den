//! Wraps the Codepool `reqwest` byte stream with TTFB logging and terminal outcome metrics.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use futures::ready;
use futures::Stream;
use uuid::Uuid;

use super::metrics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    Ok,
    Empty,
    ProxyError,
}

/// Streams bytes from Codepool to the browser while recording observability.
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

fn rich_event_status_text(event: &serde_json::Value) -> Option<String> {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
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
        "client_tool_request" => "Waiting for client tool".to_string(),
        _ => return None,
    };
    Some(text)
}

fn bear_channel_event_to_deep_chat_sse(event: &serde_json::Value) -> Option<Bytes> {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mapped = match ty {
        "assistant_delta" => serde_json::json!({
            "message_type": "assistant_message",
            "content": event.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            "id": event.get("id").and_then(|v| v.as_str()),
        }),
        "reasoning_delta" => {
            let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");
            serde_json::json!({
                "message_type": "reasoning_message",
                "reasoning": text,
                "content": text,
                "id": event.get("id").and_then(|v| v.as_str()),
            })
        }
        "error" => serde_json::json!({
            "message_type": "error_message",
            "message": event.get("message").and_then(|v| v.as_str()).unwrap_or("Upstream error"),
            "detail": event.get("detail").and_then(|v| v.as_str()),
            "error_type": event.get("error_type").and_then(|v| v.as_str()),
            "support_ref": event.get("request_id").and_then(|v| v.as_str()),
            "context": event.get("context"),
        }),
        "conversation_resolved" => serde_json::json!({
            "message_type": "conversation_resolved",
            "conversation_id": event.get("conversation_id").and_then(|v| v.as_str()),
        }),
        // `done` is terminal control metadata, not user-visible status.
        "done" => return None,
        "server_tool_started"
        | "server_tool_finished"
        | "subagent_started"
        | "subagent_finished"
        | "memory_update_recorded"
        | "client_tool_request" => {
            let text = rich_event_status_text(event)?;
            serde_json::json!({
                "message_type": "status_message",
                "content": text,
                "status_type": ty,
            })
        }
        _ => return None,
    };
    Some(Bytes::from(format!("data: {}\n\n", mapped)))
}

pub(crate) fn map_bear_channel_sse_frame(frame: &[u8]) -> Vec<Bytes> {
    let text = String::from_utf8_lossy(frame);
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(bytes) = bear_channel_event_to_deep_chat_sse(&value) {
                out.push(bytes);
            }
        }
    }
    out
}

/// Streams `bear_channel` SSE from Codepool to the browser after translating channel events
/// into the existing Deep Chat / Letta-shaped SSE payloads consumed by `bear_chat.html`.
pub struct BearChannelSseProxyStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    request_id: Uuid,
    user_id: i32,
    bear_id: Uuid,
    conversation_id: String,
    started_at: Instant,
    first_byte_at: Option<Instant>,
    total_bytes: usize,
    terminal: Option<Terminal>,
    buffer: Vec<u8>,
    pending: VecDeque<Bytes>,
}

impl BearChannelSseProxyStream {
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
            buffer: Vec::new(),
            pending: VecDeque::new(),
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
                elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                "chat_send bear_channel stream ended with zero browser-compatible bytes"
            ),
            Terminal::ProxyError => {}
        }
    }

    fn queue_mapped_frames(&mut self) {
        while let Some(pos) = self.buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = self.buffer.drain(..pos + 2).collect();
            for bytes in map_bear_channel_sse_frame(&frame) {
                self.pending.push_back(bytes);
            }
        }
    }
}

impl Drop for BearChannelSseProxyStream {
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
            "chat_send bear_channel proxy stream dropped before terminal poll (client disconnect or task cancelled)"
        );
    }
}

impl Stream for BearChannelSseProxyStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if let Some(bytes) = this.pending.pop_front() {
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
                            "chat_send first bear_channel byte from Codepool"
                        );
                    }
                    this.buffer.extend_from_slice(&chunk);
                    this.queue_mapped_frames();
                    if let Some(bytes) = this.pending.pop_front() {
                        this.total_bytes += bytes.len();
                        return Poll::Ready(Some(Ok(bytes)));
                    }
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
                        "chat_send bear_channel proxy chunk error from Codepool"
                    );
                    return Poll::Ready(Some(Err(std::io::Error::other(e.to_string()))));
                }
                None => {
                    if !this.buffer.is_empty() {
                        let frame = std::mem::take(&mut this.buffer);
                        for bytes in map_bear_channel_sse_frame(&frame) {
                            this.pending.push_back(bytes);
                        }
                        if let Some(bytes) = this.pending.pop_front() {
                            this.total_bytes += bytes.len();
                            return Poll::Ready(Some(Ok(bytes)));
                        }
                    }
                    if this.terminal.is_some() {
                        return Poll::Ready(None);
                    }
                    let outcome = if this.total_bytes == 0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped_text(frame: &str) -> String {
        map_bear_channel_sse_frame(frame.as_bytes())
            .into_iter()
            .map(|b| String::from_utf8(b.to_vec()).expect("utf8"))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn maps_assistant_delta_to_deep_chat_sse() {
        let out =
            mapped_text("data: {\"type\":\"assistant_delta\",\"text\":\"Hi\",\"id\":\"a1\"}\n\n");
        assert!(out.starts_with("data: "));
        assert!(out.contains("\"message_type\":\"assistant_message\""));
        assert!(out.contains("\"content\":\"Hi\""));
    }

    #[test]
    fn maps_reasoning_delta_to_deep_chat_sse() {
        let out = mapped_text("data: {\"type\":\"reasoning_delta\",\"text\":\"Thinking\"}\n\n");
        assert!(out.contains("\"message_type\":\"reasoning_message\""));
        assert!(out.contains("\"reasoning\":\"Thinking\""));
    }

    #[test]
    fn maps_error_to_deep_chat_sse() {
        let out = mapped_text("data: {\"type\":\"error\",\"message\":\"Nope\",\"detail\":\"More\",\"request_id\":\"r1\",\"context\":{\"upstream_error\":[{\"param\":\"tools[15].name\"}]}}\n\n");
        assert!(out.contains("\"message_type\":\"error_message\""));
        assert!(out.contains("\"message\":\"Nope\""));
        assert!(out.contains("\"support_ref\":\"r1\""));
        assert!(out.contains("\"upstream_error\""));
    }

    #[test]
    fn maps_rich_events_to_status_messages() {
        let out =
            mapped_text("data: {\"type\":\"server_tool_started\",\"tool\":\"cabinet.search\"}\n\n");
        assert!(out.contains("\"message_type\":\"status_message\""));
        assert!(out.contains("Started cabinet.search"));
    }

    #[test]
    fn drops_done_control_event() {
        let out = mapped_text("data: {\"type\":\"done\",\"outcome\":\"ok\"}\n\n");
        assert!(out.is_empty());
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
                        "chat_send first byte from Codepool"
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
                    "chat_send sse proxy chunk error from Codepool"
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
