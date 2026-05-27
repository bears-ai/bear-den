use crate::{
    core::{acp_letta_events::AcpGatewayEvent, letta::normalize_display_status_text},
};

fn acp_max_thought_bytes_per_turn() -> usize {
    std::env::var("BEARS_ACP_MAX_THOUGHT_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1024, 1024 * 1024))
        .unwrap_or(128 * 1024)
}

#[derive(Default)]
pub(super) struct AcpTextChunker {
    assistant: String,
    reasoning: String,
    max_chars: usize,
    max_reasoning_bytes: usize,
    emitted_reasoning_bytes: usize,
    reasoning_limit_reached: bool,
}

impl AcpTextChunker {
    pub(super) fn new(max_chars: usize) -> Self {
        Self::new_with_reasoning_limit(max_chars, acp_max_thought_bytes_per_turn())
    }

    pub(super) fn new_with_reasoning_limit(max_chars: usize, max_reasoning_bytes: usize) -> Self {
        Self {
            assistant: String::new(),
            reasoning: String::new(),
            max_chars,
            max_reasoning_bytes,
            emitted_reasoning_bytes: 0,
            reasoning_limit_reached: false,
        }
    }

    pub(super) fn push(&mut self, event: AcpGatewayEvent) -> Vec<AcpGatewayEvent> {
        match event {
            AcpGatewayEvent::AssistantTextDelta { text } => {
                self.assistant.push_str(&text);
                if should_flush_text(&self.assistant, self.max_chars) {
                    self.flush_assistant().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            AcpGatewayEvent::StatusText { text } => {
                if self.reasoning_limit_reached {
                    return Vec::new();
                }
                let first_status_for_turn =
                    self.emitted_reasoning_bytes == 0 && self.reasoning.is_empty();
                self.reasoning.push_str(&text);
                if first_status_for_turn || should_flush_text(&self.reasoning, self.max_chars) {
                    self.flush_reasoning().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            other => {
                let mut events = self.flush_all();
                events.push(other);
                events
            }
        }
    }

    pub(super) fn flush_assistant(&mut self) -> Option<AcpGatewayEvent> {
        if self.assistant.is_empty() {
            None
        } else {
            Some(AcpGatewayEvent::AssistantTextDelta {
                text: std::mem::take(&mut self.assistant),
            })
        }
    }

    pub(super) fn flush_reasoning(&mut self) -> Option<AcpGatewayEvent> {
        if self.reasoning.is_empty() || self.reasoning_limit_reached {
            self.reasoning.clear();
            return None;
        }
        let remaining = self
            .max_reasoning_bytes
            .saturating_sub(self.emitted_reasoning_bytes);
        if remaining == 0 {
            self.reasoning.clear();
            self.reasoning_limit_reached = true;
            return Some(AcpGatewayEvent::StatusText {
                text: normalize_display_status_text(
                    "BEARS suppressed additional thinking/status output for this turn because it exceeded the safety limit",
                ),
            });
        }

        let mut text = std::mem::take(&mut self.reasoning);
        if text.len() > remaining {
            text = truncate_utf8_boundary(&text, remaining).to_string();
            self.reasoning_limit_reached = true;
            text.push('\n');
            text.push_str(&normalize_display_status_text(
                "BEARS suppressed additional thinking/status output for this turn because it exceeded the safety limit",
            ));
        }
        self.emitted_reasoning_bytes = self.emitted_reasoning_bytes.saturating_add(text.len());
        Some(AcpGatewayEvent::StatusText { text })
    }

    pub(super) fn flush_all(&mut self) -> Vec<AcpGatewayEvent> {
        self.flush_assistant()
            .into_iter()
            .chain(self.flush_reasoning())
            .collect()
    }
}

pub(super) fn should_flush_text(buffer: &str, max_chars: usize) -> bool {
    buffer.chars().count() >= max_chars
        || buffer.ends_with('\n')
        || buffer.ends_with(". ")
        || buffer.ends_with("! ")
        || buffer.ends_with("? ")
}

pub(super) fn truncate_utf8_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
