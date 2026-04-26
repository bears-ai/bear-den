//! Map `GET /v1/agents/{id}` JSON into Den admin "new bear" form defaults.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentBearPrefill {
    pub suggested_slug: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub default_model: String,
    pub letta_agent_type: String,
    pub letta_tool_ids: Vec<String>,
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

fn tool_ids_from_agent(v: &Value) -> Vec<String> {
    let Some(arr) = v.get("tools").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for t in arr {
        let id = t
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(id) = id {
            out.push(id.to_string());
        }
    }
    out
}

fn suggest_slug(name: &str, agent_id: &str) -> String {
    let slugish: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_whitespace() || matches!(c, '-' | '_') {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|&c| c != '\0')
        .collect();
    let mut s = slugish;
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_string();
    if s.is_empty() {
        let tail: String = agent_id
            .trim_start_matches("agent-")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(16)
            .collect();
        let tail = if tail.is_empty() {
            "import".to_string()
        } else {
            tail
        };
        format!("bear-{tail}")
    } else {
        s.chars().take(120).collect()
    }
}

impl AgentBearPrefill {
    pub fn from_agent_json(v: &Value) -> Self {
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let name = pick_str(v, &["name"]).unwrap_or_else(|| id.clone());
        let description = pick_str(v, &["description"]).unwrap_or_default();
        let system_prompt = v
            .get("system")
            .or_else(|| v.get("system_prompt"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let default_model = model_field(v).unwrap_or_default();
        let letta_agent_type = pick_str(v, &["agent_type"]).unwrap_or_default();
        let letta_tool_ids = tool_ids_from_agent(v);
        let suggested_slug = suggest_slug(&name, &id);

        Self {
            suggested_slug,
            name,
            description,
            system_prompt,
            default_model,
            letta_agent_type,
            letta_tool_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefill_reads_core_fields() {
        let v = json!({
            "id": "agent-abc",
            "name": "My Bot",
            "description": "desc",
            "system": "You are helpful",
            "agent_type": "memgpt_agent",
            "model": "openai/gpt-4o",
            "tools": [{"id": "tool-1"}, {"id": "tool-2"}]
        });
        let p = AgentBearPrefill::from_agent_json(&v);
        assert_eq!(p.suggested_slug, "my-bot");
        assert_eq!(p.name, "My Bot");
        assert_eq!(p.description, "desc");
        assert_eq!(p.system_prompt, "You are helpful");
        assert_eq!(p.default_model, "openai/gpt-4o");
        assert_eq!(p.letta_agent_type, "memgpt_agent");
        assert_eq!(p.letta_tool_ids, vec!["tool-1", "tool-2"]);
    }

    #[test]
    fn slug_falls_back_when_name_has_no_letters() {
        let v = json!({
            "id": "agent-xyz12",
            "name": "!!!",
            "system": "x"
        });
        let p = AgentBearPrefill::from_agent_json(&v);
        assert_eq!(p.suggested_slug, "bear-xyz12");
    }
}
