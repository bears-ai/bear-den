//! Present Letta `GET /v1/agents/{id}` JSON in the admin UI without pinning the full schema.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub memory_block_count: Option<usize>,
    pub tool_count: Option<usize>,
    /// Full top-level `system` / `system_prompt` from Letta when present.
    pub system: Option<String>,
    /// Short excerpt when Letta exposes a top-level system string.
    pub system_excerpt: Option<String>,
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

fn model_field(v: &Value) -> Option<String> {
    let m = v.get("model")?;
    if let Some(s) = m.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = m.as_object() {
        if let Some(s) = obj.get("model").and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
        return Some(m.to_string());
    }
    None
}

fn excerpt(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    format!("{}…", t.chars().take(max).collect::<String>())
}

fn array_len(v: &Value, key: &str) -> Option<usize> {
    v.get(key).and_then(|x| x.as_array()).map(|a| a.len())
}

fn memory_block_count(v: &Value) -> Option<usize> {
    array_len(v, "blocks").or_else(|| {
        v.get("memory")
            .and_then(|m| m.get("blocks"))
            .and_then(|x| x.as_array())
            .map(|a| a.len())
    })
}

impl AgentSummary {
    pub fn from_letta_agent_state(v: &Value) -> Self {
        let v = super::unwrap_letta_agent_document(v);
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("(missing id)")
            .to_string();

        let system = v
            .get("system")
            .or_else(|| v.get("system_prompt"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());

        let system_excerpt = system
            .as_deref()
            .map(|s| excerpt(s, 280))
            .filter(|s| !s.is_empty());

        Self {
            id,
            name: pick_str(v, &["name"]),
            agent_type: pick_str(v, &["agent_type"]),
            model: model_field(v),
            description: pick_str(v, &["description"]),
            created_at: pick_str(v, &["created_at"]),
            updated_at: pick_str(v, &["updated_at"]),
            memory_block_count: memory_block_count(v),
            tool_count: array_len(v, "tools"),
            system,
            system_excerpt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_extracts_common_fields() {
        let v = json!({
            "id": "agent-test",
            "name": "Helper",
            "agent_type": "memgpt_agent",
            "model": "gpt-4o",
            "system": "You are a test agent.",
            "created_at": "2025-01-01T00:00:00Z",
            "blocks": [{"id": "b1"}],
            "tools": [{"id": "t1"}, {"id": "t2"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        assert_eq!(s.id, "agent-test");
        assert_eq!(s.name.as_deref(), Some("Helper"));
        assert_eq!(s.agent_type.as_deref(), Some("memgpt_agent"));
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
        assert_eq!(s.memory_block_count, Some(1));
        assert_eq!(s.tool_count, Some(2));
        assert_eq!(s.system.as_deref(), Some("You are a test agent."));
    }

    #[test]
    fn summary_counts_blocks_under_deprecated_memory() {
        let v = json!({
            "id": "agent-test",
            "memory": {"blocks": [{"id": "b1"}, {"id": "b2"}]},
            "tools": []
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        assert_eq!(s.memory_block_count, Some(2));
    }
}
