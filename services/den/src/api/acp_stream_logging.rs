use base64::Engine as _;
use serde_json::json;

use crate::api::acp::acp_debug_event_sample_chars;

const SSE_JSON_PREVIEW_MAX: usize = 192;

fn sha256_short(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(digest)
        .chars()
        .take(16)
        .collect()
}

fn preview_str_truncated(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

fn summarize_large_text_field(value: &str, allow_preview: bool) -> serde_json::Value {
    let mut summary = json!({
        "redacted": true,
        "bytes": value.len(),
        "chars": value.chars().count(),
        "sha256": sha256_short(value),
    });
    if allow_preview && !value.is_empty() {
        summary["preview"] = json!(preview_str_truncated(
            value,
            acp_debug_event_sample_chars().min(512)
        ));
        summary["truncated"] = json!(value.len() > acp_debug_event_sample_chars().min(512));
    }
    summary
}

fn summarize_tool_arguments(value: &str) -> serde_json::Value {
    let mut summary = summarize_large_text_field(value, false);
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(object) = parsed.as_object() {
            summary["json_keys"] = json!(object.keys().cloned().collect::<Vec<_>>());
            for key in [
                "path",
                "destination_path",
                "source_path",
                "root",
                "glob",
                "pattern",
                "query",
                "line",
                "limit",
                "recursive",
                "include_hidden",
                "command",
                "cwd",
            ] {
                if let Some(value) = object.get(key) {
                    summary[key] = value.clone();
                }
            }
        }
    }
    summary
}

pub(super) fn summarize_letta_event_for_log(value: &serde_json::Value) -> serde_json::Value {
    let mut event = value.clone();
    let allow_preview = cfg!(debug_assertions)
        || std::env::var("BEARS_ACP_DEBUG_EVENT_SAMPLES")
            .ok()
            .is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });
    if let Some(object) = event.as_object_mut() {
        if let Some(args) = object.get("args").and_then(serde_json::Value::as_str) {
            let summarized = summarize_tool_arguments(args);
            object.insert("args".to_string(), summarized);
        }
        for key in ["tool_return", "content", "reasoning", "text", "message"] {
            if let Some(value) = object.get(key).and_then(serde_json::Value::as_str) {
                if value.len() > 256 {
                    object.insert(
                        key.to_string(),
                        summarize_large_text_field(value, allow_preview),
                    );
                }
            }
        }
        if let Some(message) = object
            .get_mut("message")
            .and_then(serde_json::Value::as_object_mut)
        {
            for key in ["content", "reasoning", "text", "tool_return"] {
                if let Some(value) = message.get(key).and_then(serde_json::Value::as_str) {
                    if value.len() > 256 {
                        message.insert(
                            key.to_string(),
                            summarize_large_text_field(value, allow_preview),
                        );
                    }
                }
            }
        }
        let keys = object.keys().cloned().collect::<Vec<_>>();
        object.insert("keys".to_string(), json!(keys));
    }
    event
}

pub(super) fn summarize_json_parse_error_for_log(body: &str) -> serde_json::Value {
    json!({
        "utf8_len": body.len(),
        "sample": preview_str_truncated(body, SSE_JSON_PREVIEW_MAX),
    })
}
