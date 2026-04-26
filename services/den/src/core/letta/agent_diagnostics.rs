//! Read-only rows from `GET /v1/agents/{id}` for the operator edit page.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct LettaBlockRow {
    pub id: Option<String>,
    pub label: Option<String>,
    /// Rough size of the block payload for overview tables (characters).
    pub char_count: Option<usize>,
    /// Block body from Letta (`value` / `content`), for read-only UI.
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LettaToolRow {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LettaAgentDiagnostics {
    pub blocks: Vec<LettaBlockRow>,
    pub tools: Vec<LettaToolRow>,
    pub raw_json: String,
}

fn block_value_char_count(v: &Value) -> usize {
    if let Some(s) = v.as_str() {
        return s.chars().count();
    }
    serde_json::to_string(v)
        .map(|s| s.chars().count())
        .unwrap_or(0)
}

/// Text or JSON-serialized body from `value` / `content` on a Letta block object.
fn block_body_text(b: &Value) -> Option<String> {
    fn from_field(block: &Value, key: &str) -> Option<String> {
        let v = block.get(key)?;
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
        if v.is_null() {
            return None;
        }
        serde_json::to_string_pretty(v).ok()
    }
    from_field(b, "value").or_else(|| from_field(b, "content"))
}

fn pick_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn tool_rows_from_array(arr: &[Value]) -> Vec<LettaToolRow> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for t in arr {
        let (id, name) = if let Some(s) = t.as_str() {
            (s.trim().to_string(), None)
        } else {
            (
                pick_str(t, &["id", "tool_id"]).unwrap_or_default(),
                pick_str(t, &["name", "tool_name"]),
            )
        };
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        out.push(LettaToolRow { id, name });
    }
    out
}

/// Top-level `blocks`, or deprecated `memory.blocks` on older agent payloads.
fn agent_blocks_array(v: &Value) -> Option<&Vec<Value>> {
    v.get("blocks").and_then(|x| x.as_array()).or_else(|| {
        v.get("memory")
            .and_then(|m| m.get("blocks"))
            .and_then(|x| x.as_array())
    })
}

impl LettaAgentDiagnostics {
    pub fn from_agent_json(v: &Value) -> Self {
        let v = super::unwrap_letta_agent_document(v);
        let raw_json = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());

        let mut blocks = Vec::new();
        if let Some(arr) = agent_blocks_array(v) {
            for b in arr {
                let char_count = b
                    .get("value")
                    .map(block_value_char_count)
                    .or_else(|| b.get("content").map(block_value_char_count));
                let content = block_body_text(b);
                blocks.push(LettaBlockRow {
                    id: pick_str(b, &["id"]),
                    label: pick_str(b, &["label", "name"]),
                    char_count,
                    content,
                });
            }
        }

        let tools = if let Some(arr) = v.get("tools").and_then(|x| x.as_array()) {
            tool_rows_from_array(arr)
        } else if let Some(arr) = v.get("tool_ids").and_then(|x| x.as_array()) {
            tool_rows_from_array(arr)
        } else if let Some(arr) = v.get("tool_rules").and_then(|x| x.as_array()) {
            tool_rows_from_array(arr)
        } else {
            Vec::new()
        };

        Self {
            blocks,
            tools,
            raw_json,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_blocks_and_tools() {
        let v = json!({
            "id": "agent-x",
            "blocks": [{"id": "b1", "label": "human", "value": "abc"}],
            "tools": [{"id": "tool-1", "name": "grep"}]
        });
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.blocks[0].label.as_deref(), Some("human"));
        assert_eq!(d.blocks[0].char_count, Some(3));
        assert_eq!(d.blocks[0].content.as_deref(), Some("abc"));
        assert_eq!(d.tools.len(), 1);
        assert_eq!(d.tools[0].id, "tool-1");
    }

    #[test]
    fn parses_blocks_from_deprecated_memory_field() {
        let v = json!({
            "id": "agent-x",
            "memory": {"blocks": [{"id": "b2", "label": "persona", "value": "x"}]},
            "tools": []
        });
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.blocks[0].label.as_deref(), Some("persona"));
        assert_eq!(d.blocks[0].content.as_deref(), Some("x"));
    }

    #[test]
    fn parses_non_string_block_value_as_json() {
        let v = json!({
            "id": "agent-x",
            "blocks": [{"id": "b1", "label": "cfg", "value": {"k": 1}}],
            "tools": []
        });
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        assert!(d.blocks[0].content.as_ref().unwrap().contains("\"k\""));
    }

    #[test]
    fn parses_tool_ids_fallback_and_string_tools() {
        let v = json!({
            "id": "agent-x",
            "tools": ["tool-1", {"tool_id": "tool-2", "tool_name": "grep"}, "tool-1"]
        });
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        assert_eq!(d.tools.len(), 2);
        assert_eq!(d.tools[0].id, "tool-1");
        assert_eq!(d.tools[1].id, "tool-2");
        assert_eq!(d.tools[1].name.as_deref(), Some("grep"));
    }
}
