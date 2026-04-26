//! Human-facing titles for Letta `conv-*` threads: `summary` when usable, else derived from
//! the first meaningful **human-entered** user message, else a generic label. Used by
//! `/v1/chat/conversations`.
//!
//! Harness/model-injected text (e.g. `<system-reminder>…</system-reminder>`) is stripped before
//! title derivation; structured `role: system` user rows are skipped when present.

use serde_json::Value;

/// Generic UI label when nothing better is available (not persisted to Letta).
pub const UNTITLED_THREAD: &str = "Untitled thread";

/// Returns true when `summary` is suitable to show as the thread title and to treat as canonical.
pub fn is_meaningful_conversation_title(summary: Option<&str>, conversation_id: &str) -> bool {
    let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if looks_like_machine_or_opaque_title(s, conversation_id) {
        return false;
    }
    if is_generic_thread_placeholder(s) {
        return false;
    }
    true
}

/// Whether text derived from a user message is safe to show (still rejects UUID-like / opaque ids).
pub fn is_acceptable_derived_title(s: &str, conversation_id: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    !looks_like_machine_or_opaque_title(s, conversation_id) && !looks_like_json_blob(s)
}

/// Choose a display title: meaningful Letta summary, else derived from messages, else [`UNTITLED_THREAD`].
pub fn display_conversation_title(
    summary: Option<&str>,
    conversation_id: &str,
    first_user_message_text: Option<&str>,
) -> String {
    if is_meaningful_conversation_title(summary, conversation_id) {
        return summary.unwrap().trim().to_string();
    }
    if let Some(t) = first_user_message_text {
        if let Some(d) = derive_title_from_user_message(t) {
            if is_acceptable_derived_title(&d, conversation_id) {
                return d;
            }
        }
    }
    UNTITLED_THREAD.to_string()
}

/// Walk an ascending-order `GET …/messages` body and return text from the first user message
/// that can yield a non-empty derived title candidate (after light cleaning).
pub fn first_user_message_text_for_title(messages_body: &Value) -> Option<String> {
    for msg in messages_array(messages_body) {
        let inner = letta_inner(msg);
        let mt = message_type(msg, inner);
        if mt != "user_message" {
            continue;
        }
        if !user_message_role_is_human(inner, msg) {
            continue;
        }
        let text = message_text(inner).or_else(|| message_text(msg))?;
        let without_harness = super::strip_letta_harness_for_user(&text);
        let cleaned = strip_noise_for_title_source(&without_harness);
        if cleaned.trim().is_empty() {
            continue;
        }
        if derive_title_from_user_message(&cleaned).is_some() {
            return Some(cleaned);
        }
    }
    None
}

/// Derive a short title from raw user text (markdown-ish noise stripped, truncated).
pub fn derive_title_from_user_message(raw: &str) -> Option<String> {
    let mut s = strip_noise_for_title_source(raw);
    s = s.trim().to_string();
    if s.is_empty() {
        return None;
    }
    // Drop a leading code fence if stripping missed edge cases.
    if s.starts_with('`') {
        s = s.trim_start_matches('`').trim().to_string();
    }
    if s.is_empty() {
        return None;
    }
    if looks_like_json_blob(&s) {
        return None;
    }

    // First line / first sentence-ish chunk.
    let mut line: String = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() {
        return None;
    }
    // If the first line is still huge, cut at punctuation or hard limit.
    if line.len() > 120 {
        line = truncate_at_word_boundary(&line, 120);
    }

    line = truncate_to_title_length(&line);
    line = trim_trailing_punct(line);
    line = line.trim().to_string();
    if line.is_empty() {
        return None;
    }
    Some(line)
}

fn messages_array<'a>(body: &'a Value) -> &'a [Value] {
    if let Some(a) = body.as_array() {
        return a.as_slice();
    }
    if let Some(a) = body.get("messages").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = body.get("data").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    if let Some(a) = body.get("items").and_then(|x| x.as_array()) {
        return a.as_slice();
    }
    &[]
}

fn letta_inner<'a>(msg: &'a Value) -> &'a Value {
    match msg.get("contents") {
        Some(c) if c.get("message_type").is_some() => c,
        _ => msg,
    }
}

fn message_type<'a>(msg: &'a Value, inner: &'a Value) -> &'a str {
    inner
        .get("message_type")
        .and_then(|x| x.as_str())
        .or_else(|| msg.get("message_type").and_then(|x| x.as_str()))
        .unwrap_or("")
}

/// Skip structured rows that are not end-user input (when Letta exposes `role` on the message).
fn user_message_role_is_human(inner: &Value, msg: &Value) -> bool {
    for v in [inner, msg] {
        let Some(role) = v.get("role").and_then(|x| x.as_str()) else {
            continue;
        };
        let r = role.trim();
        if r.eq_ignore_ascii_case("system") || r.eq_ignore_ascii_case("developer") {
            return false;
        }
    }
    true
}

fn message_text(inner: &Value) -> Option<String> {
    let content = inner.get("content")?;
    if content.is_null() {
        return None;
    }
    if let Some(s) = content.as_str() {
        let s = s.trim();
        return if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
    }
    if let Some(obj) = content.as_object() {
        if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
            let t = t.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    for p in parts {
        let ty = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if matches!(
            ty,
            "text" | "Text" | "text_delta" | "reasoning_text" | "output_text"
        ) {
            if let Some(t) = p.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
            }
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn strip_leading_code_fences(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        if !t.starts_with("```") {
            return t;
        }
        let after_first_line = t.split_once('\n').map(|x| x.1).unwrap_or("");
        if let Some(pos) = after_first_line.find("\n```") {
            s = after_first_line[pos + 4..].trim_start();
            continue;
        }
        if let Some(pos) = after_first_line.find("```") {
            s = after_first_line[pos + 3..].trim_start();
            continue;
        }
        return after_first_line.trim_start();
    }
}

fn strip_noise_for_title_source(raw: &str) -> String {
    let mut s = strip_leading_code_fences(raw).to_string();
    // Blockquote lines
    let mut lines: Vec<&str> = Vec::new();
    for line in s.lines() {
        let t = line.trim_start();
        let t = t.strip_prefix('>').unwrap_or(t).trim_start();
        if !t.is_empty() {
            lines.push(t);
        }
    }
    s = lines.join(" ");
    // Inline backticks (collapse)
    let mut out = String::with_capacity(s.len());
    let mut in_tick = false;
    for ch in s.chars() {
        if ch == '`' {
            in_tick = !in_tick;
            continue;
        }
        if !in_tick {
            out.push(ch);
        }
    }
    s = out.split_whitespace().collect::<Vec<_>>().join(" ");
    s
}

fn looks_like_json_blob(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with('{') && t.ends_with('}') && t.len() > 40)
        || (t.starts_with('[') && t.ends_with(']') && t.len() > 40)
}

fn truncate_at_word_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let slice = &s[..max];
    if let Some(pos) = slice.rfind(|c: char| c.is_whitespace()) {
        if pos > 10 {
            return slice[..pos].trim().to_string();
        }
    }
    slice.trim().to_string()
}

fn truncate_to_title_length(s: &str) -> String {
    const MAX_CHARS: usize = 60;
    const MAX_WORDS: usize = 8;
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let n = words.len().min(MAX_WORDS);
    for w in words.iter().take(n) {
        if !out.is_empty() {
            out.push(' ');
        }
        if out.len() + w.len() + 1 > MAX_CHARS {
            break;
        }
        out.push_str(w);
    }
    if out.is_empty() {
        // First word alone exceeds MAX_CHARS — hard cut.
        let mut t = s.chars().take(MAX_CHARS).collect::<String>();
        if let Some(pos) = t.rfind(|c: char| c.is_whitespace()) {
            if pos > 5 {
                t.truncate(pos);
            }
        }
        return t.trim().to_string();
    }
    out
}

fn trim_trailing_punct(mut s: String) -> String {
    while matches!(
        s.chars().last(),
        Some('.') | Some(',') | Some(';') | Some(':') | Some('!')
    ) {
        s.pop();
    }
    s
}

fn is_uuid_like(s: &str) -> bool {
    let t = s.trim();
    // 8-4-4-4-12
    if t.len() == 36 && t.chars().filter(|c| *c == '-').count() == 4 {
        return t.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    }
    // 32 hex (no hyphens)
    if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

fn looks_like_machine_or_opaque_title(s: &str, conversation_id: &str) -> bool {
    let t = s.trim();
    if t == conversation_id {
        return true;
    }
    if let Some(rest) = t.strip_prefix("conv-") {
        if is_uuid_like(rest) {
            return true;
        }
    }
    if is_uuid_like(t) {
        return true;
    }
    if conversation_id.starts_with("conv-") {
        let suf = conversation_id
            .strip_prefix("conv-")
            .unwrap_or(conversation_id);
        if t == suf {
            return true;
        }
    }
    // Legacy UI fallback: "Chat (uuid-fragment)"
    if let Some(inner) = t.strip_prefix("Chat (").and_then(|x| x.strip_suffix(')')) {
        let inner = inner.trim();
        if is_uuid_like(inner) || inner == conversation_id.strip_prefix("conv-").unwrap_or("") {
            return true;
        }
    }
    // Long hex-only tokens (opaque ids)
    if t.len() >= 24 && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        let hexish = t.chars().filter(|c| c.is_ascii_hexdigit()).count();
        if hexish * 10 >= t.len() * 7 {
            return true;
        }
    }
    false
}

fn is_generic_thread_placeholder(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "new conversation" | "new chat" | "untitled" | "conversation"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meaningful_preserves_good_summary() {
        assert!(is_meaningful_conversation_title(
            Some("Research Letta thread titles"),
            "conv-550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[test]
    fn rejects_missing_summary() {
        assert!(!is_meaningful_conversation_title(
            None,
            "conv-550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(!is_meaningful_conversation_title(
            Some("   "),
            "conv-550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[test]
    fn rejects_summary_equals_conv_uuid() {
        let id = "conv-550e8400-e29b-41d4-a716-446655440000";
        assert!(!is_meaningful_conversation_title(Some(id), id));
    }

    #[test]
    fn rejects_plain_uuid_summary() {
        assert!(!is_meaningful_conversation_title(
            Some("550e8400-e29b-41d4-a716-446655440000"),
            "conv-550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[test]
    fn rejects_legacy_chat_paren_uuid_summary() {
        assert!(!is_meaningful_conversation_title(
            Some("Chat (550e8400-e29b-41d4-a716-446655440000)"),
            "conv-550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[test]
    fn rejects_new_conversation_placeholder_in_letta_summary() {
        assert!(!is_meaningful_conversation_title(
            Some("New conversation"),
            "conv-x"
        ));
    }

    #[test]
    fn derived_title_can_be_new_conversation_when_user_typed_it() {
        let id = "conv-550e8400-e29b-41d4-a716-446655440000";
        let d = display_conversation_title(Some(id), id, Some("New conversation about Rust"));
        assert_eq!(d, "New conversation about Rust");
    }

    #[test]
    fn derive_truncates_long_message() {
        let long = "word ".repeat(40);
        let t = derive_title_from_user_message(&long).expect("title");
        assert!(t.len() <= 60);
        assert!(t.split_whitespace().count() <= 8);
    }

    #[test]
    fn derive_strips_leading_code_fence() {
        let raw = "```rust\nfn main() {}\n```\n\nExplain ownership";
        let t = derive_title_from_user_message(raw).expect("title");
        assert!(t.to_ascii_lowercase().contains("explain"));
        assert!(!t.contains("fn main"));
    }

    #[test]
    fn derive_prefers_text_after_fence() {
        let raw = "```\nSELECT * FROM x;\n```\nHelp me tune this query";
        let t = derive_title_from_user_message(raw).expect("title");
        assert!(t.contains("Help"));
    }

    #[test]
    fn no_meaningful_user_message_yields_untitled_in_display() {
        let d = display_conversation_title(None, "conv-abc", None);
        assert_eq!(d, UNTITLED_THREAD);
    }

    #[test]
    fn display_falls_back_from_bad_summary_to_derived() {
        let id = "conv-550e8400-e29b-41d4-a716-446655440000";
        let d = display_conversation_title(Some(id), id, Some("Fix UUID thread summaries please"));
        assert_eq!(d, "Fix UUID thread summaries please");
    }

    #[test]
    fn json_blob_not_used_as_title() {
        let blob = r#"{"a":1,"b":2,"c":3,"d":4,"e":5,"f":6,"g":7}"#;
        assert!(derive_title_from_user_message(blob).is_none());
    }

    #[test]
    fn first_user_skips_system_only() {
        let body = serde_json::json!([
            {"id": "1", "message_type": "tool_call_message", "content": "{}"},
            {
                "id": "2",
                "date": "2025-01-02T00:00:00Z",
                "message_type": "user_message",
                "content": "Hello there"
            }
        ]);
        assert_eq!(
            first_user_message_text_for_title(&body).as_deref(),
            Some("Hello there")
        );
    }

    #[test]
    fn first_user_skips_harness_only_system_reminder_then_uses_human() {
        let body = serde_json::json!([
            {
                "id": "1",
                "message_type": "user_message",
                "content": "<system-reminder>The user has just initiated a new connection via the Letta Code CLI client.</system-reminder>"
            },
            {
                "id": "2",
                "message_type": "user_message",
                "content": "Plan the Q2 roadmap"
            }
        ]);
        assert_eq!(
            first_user_message_text_for_title(&body).as_deref(),
            Some("Plan the Q2 roadmap")
        );
    }

    #[test]
    fn first_user_strips_inline_reminder_keeps_human_text() {
        let body = serde_json::json!([
            {
                "id": "1",
                "message_type": "user_message",
                "content": "<system-reminder>context</system-reminder>\n\nSummarize this doc"
            }
        ]);
        assert_eq!(
            first_user_message_text_for_title(&body).as_deref(),
            Some("Summarize this doc")
        );
    }

    #[test]
    fn first_user_skips_role_system() {
        let body = serde_json::json!([
            {
                "id": "1",
                "role": "system",
                "message_type": "user_message",
                "content": "You are a helpful assistant."
            },
            {
                "id": "2",
                "message_type": "user_message",
                "content": "What is 2+2?"
            }
        ]);
        assert_eq!(
            first_user_message_text_for_title(&body).as_deref(),
            Some("What is 2+2?")
        );
    }
}
