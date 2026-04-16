//! Read-only rows from `GET /v1/agents/{id}` for the operator edit page.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct LettaBlockRow {
    pub id: Option<String>,
    pub label: Option<String>,
    /// Rough size of the block payload for overview tables (characters).
    pub char_count: Option<usize>,
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

impl LettaAgentDiagnostics {
    pub fn from_agent_json(v: &Value) -> Self {
        let raw_json = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());

        let mut blocks = Vec::new();
        if let Some(arr) = v.get("blocks").and_then(|x| x.as_array()) {
            for b in arr {
                let char_count = b
                    .get("value")
                    .map(block_value_char_count)
                    .or_else(|| {
                        b.get("content")
                            .map(block_value_char_count)
                    });
                blocks.push(LettaBlockRow {
                    id: pick_str(b, &["id"]),
                    label: pick_str(b, &["label", "name"]),
                    char_count,
                });
            }
        }

        let mut tools = Vec::new();
        if let Some(arr) = v.get("tools").and_then(|x| x.as_array()) {
            for t in arr {
                let id = pick_str(t, &["id"]).unwrap_or_default();
                if id.is_empty() {
                    continue;
                }
                tools.push(LettaToolRow {
                    id,
                    name: pick_str(t, &["name", "tool_name"]),
                });
            }
        }

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
        assert_eq!(d.tools.len(), 1);
        assert_eq!(d.tools[0].id, "tool-1");
    }
}
