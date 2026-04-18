//! Compare Den bear registry fields to a Letta `GET /v1/agents/{id}` snapshot.

use serde::Serialize;

use super::model::Bear;
use crate::core::letta::{AgentSummary, LettaAgentDiagnostics};

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
    let mut v: Vec<String> = ids.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    v.sort();
    v.dedup();
    v
}

/// Returns `None` when there is no linked agent or Letta data is unavailable (fetch failed).
pub fn compute_letta_drift(
    bear: &Bear,
    summary: Option<&AgentSummary>,
    diagnostics: Option<&LettaAgentDiagnostics>,
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

    let db_sys = bear.system_prompt.trim();
    let letta_sys = summary.system.as_deref().unwrap_or("").trim();
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
        assert!(compute_letta_drift(&b, Some(&s), Some(&d)).is_none());
    }

    #[test]
    fn drift_detects_system_mismatch() {
        let b = sample_bear();
        let v = json!({"id": "agent-1", "system": "other", "model": "gpt-4o", "agent_type": "memgpt_agent", "tools": [{"id": "t1"}], "blocks": []});
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d)).expect("drift");
        assert!(flags.drift_any);
        assert!(flags.system_prompt);
        assert!(!flags.model);
        assert!(!flags.agent_type);
        assert!(!flags.tools);
    }
}
