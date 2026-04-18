//! Compare Den bear registry fields to a Letta `GET /v1/agents/{id}` snapshot.

use serde::Serialize;
use serde_json::Value;

use super::model::Bear;
use crate::core::letta::{unwrap_letta_agent_document, AgentSummary, LettaAgentDiagnostics};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LettaDriftFlags {
    /// True if any tracked field differs between Den and Letta.
    pub drift_any: bool,
    pub system_prompt: bool,
    pub model: bool,
    pub agent_type: bool,
    pub tools: bool,
}

fn norm_opt_trim(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn sorted_tool_ids(ids: &[String]) -> Vec<String> {
    let mut v: Vec<String> = ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Normalize newlines and outer whitespace so Den vs Letta compares fairly.
fn normalize_drift_text(s: &str) -> String {
    s.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn block_str_from_value(b: &Value) -> Option<String> {
    let v = b.get("value").or_else(|| b.get("content"))?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    None
}

/// When Letta omits top-level `system`, MemGPT-style agents often store instructions in blocks.
fn fallback_system_from_blocks(v: &Value) -> Option<String> {
    let blocks = v
        .get("blocks")
        .and_then(|x| x.as_array())
        .or_else(|| {
            v.get("memory")
                .and_then(|m| m.get("blocks"))
                .and_then(|x| x.as_array())
        })?;
    let preferred = [
        "persona",
        "human",
        "system",
        "instructions",
        "core_memory",
    ];
    for label in preferred {
        for b in blocks {
            let lbl = b
                .get("label")
                .or_else(|| b.get("name"))
                .and_then(|x| x.as_str())
                .map(str::trim);
            if lbl.is_some_and(|l| l.eq_ignore_ascii_case(label)) {
                if let Some(s) = block_str_from_value(b) {
                    return Some(s);
                }
            }
        }
    }
    None
}

/// Best-effort Letta instruction text for drift (top-level `system`, else common blocks).
fn letta_instruction_text_for_drift(
    summary: Option<&AgentSummary>,
    raw_agent_json: Option<&Value>,
) -> String {
    let from_summary = summary
        .and_then(|s| s.system.as_deref())
        .map(|s| normalize_drift_text(s));

    if let Some(t) = from_summary.filter(|s| !s.is_empty()) {
        return t;
    }

    if let Some(v) = raw_agent_json {
        let root = unwrap_letta_agent_document(v);
        if let Some(s) = fallback_system_from_blocks(root) {
            return normalize_drift_text(&s);
        }
    }

    String::new()
}

/// Returns `None` when there is no linked agent or Letta data is unavailable (fetch failed).
///
/// `raw_agent_json` should be the raw `GET /v1/agents/{id}` body when the fetch succeeded, so we
/// can compare against block-based instructions if top-level `system` is absent.
pub fn compute_letta_drift(
    bear: &Bear,
    summary: Option<&AgentSummary>,
    diagnostics: Option<&LettaAgentDiagnostics>,
    raw_agent_json: Option<&Value>,
) -> Option<LettaDriftFlags> {
    if bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return None;
    }
    let summary = summary?;
    let diagnostics = diagnostics?;

    let db_sys = normalize_drift_text(&bear.system_prompt);
    let letta_sys = letta_instruction_text_for_drift(Some(summary), raw_agent_json);
    let system_prompt = db_sys != letta_sys;

    let model = norm_opt_trim(bear.default_model.as_deref()) != norm_opt_trim(summary.model.as_deref());

    let db_at = norm_opt_trim(bear.letta_agent_type.as_deref());
    let letta_at = norm_opt_trim(summary.agent_type.as_deref());
    let agent_type = db_at != letta_at;

    let db_tools = sorted_tool_ids(&bear.letta_tool_ids.0);
    let letta_tools: Vec<String> = sorted_tool_ids(
        &diagnostics
            .tools
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>(),
    );
    let tools = db_tools != letta_tools;

    let drift_any = system_prompt || model || agent_type || tools;
    Some(LettaDriftFlags {
        drift_any,
        system_prompt,
        model,
        agent_type,
        tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bears::model::Bear;
    use crate::core::letta::{AgentSummary, LettaAgentDiagnostics};
    use serde_json::json;
    use sqlx::types::Json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn sample_bear() -> Bear {
        Bear {
            id: Uuid::nil(),
            slug: "test-bear".into(),
            name: "Test".into(),
            description: String::new(),
            system_prompt: "hello".into(),
            default_model: Some("gpt-4o".into()),
            tools_enabled: None,
            letta_agent_type: Some("memgpt_agent".into()),
            letta_tool_ids: Json(vec!["t1".into()]),
            letta_agent_id: Some("agent-1".into()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn drift_none_when_no_agent() {
        let mut b = sample_bear();
        b.letta_agent_id = None;
        let v = json!({"id": "agent-1", "system": "hello", "model": "gpt-4o", "agent_type": "memgpt_agent", "tools": [{"id": "t1"}], "blocks": []});
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        assert!(compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).is_none());
    }

    #[test]
    fn drift_detects_system_mismatch() {
        let b = sample_bear();
        let v = json!({"id": "agent-1", "system": "other", "model": "gpt-4o", "agent_type": "memgpt_agent", "tools": [{"id": "t1"}], "blocks": []});
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(flags.drift_any);
        assert!(flags.system_prompt);
        assert!(!flags.model);
        assert!(!flags.agent_type);
        assert!(!flags.tools);
    }

    #[test]
    fn drift_nested_agent_envelope() {
        let b = sample_bear();
        let inner = json!({"id": "agent-1", "system": "other", "model": "gpt-4o", "agent_type": "memgpt_agent", "tools": [{"id": "t1"}], "blocks": []});
        let v = json!({"agent": inner});
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(flags.system_prompt);
    }

    #[test]
    fn drift_persona_block_when_no_top_level_system() {
        let b = sample_bear();
        let v = json!({
            "id": "agent-1",
            "model": "gpt-4o",
            "agent_type": "memgpt_agent",
            "tools": [{"id": "t1"}],
            "blocks": [{"label": "persona", "value": "edited in letta only"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(flags.system_prompt);
    }
}
