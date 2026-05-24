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

fn normalize_drift_text_collapsed(s: &str) -> String {
    normalize_drift_text(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn drift_text_matches_den_prompt(den_prompt: &str, letta_prompt: &str) -> bool {
    let den = normalize_drift_text(den_prompt);
    let letta = normalize_drift_text(letta_prompt);
    if den == letta {
        return true;
    }

    let den_collapsed = normalize_drift_text_collapsed(&den);
    if den_collapsed.is_empty() {
        return letta.is_empty();
    }
    let letta_collapsed = normalize_drift_text_collapsed(&letta);

    // Current Letta versions may expose the materialized/compiled system prompt from
    // `GET /v1/agents/{id}` after `/recompile`, not just the source `system` text Den patched.
    // Treat an embedded Den source prompt as in-sync so generated wrappers, tool sections, or
    // date headers do not make the details page permanently report drift immediately after sync.
    letta_collapsed.contains(&den_collapsed)
}

fn block_str_from_value(b: &Value) -> Option<String> {
    let v = b.get("value").or_else(|| b.get("content"))?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    None
}

/// When Letta omits top-level `system`, some payloads keep instructions in blocks.
fn fallback_system_from_blocks(v: &Value) -> Option<String> {
    let blocks = v.get("blocks").and_then(|x| x.as_array()).or_else(|| {
        v.get("memory")
            .and_then(|m| m.get("blocks"))
            .and_then(|x| x.as_array())
    })?;
    let preferred = ["system", "system_prompt", "instructions", "prompt"];
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

/// Best-effort Letta instruction text for drift.
///
/// Returns `None` when we cannot reliably identify Letta's instruction text.
fn letta_instruction_text_for_drift(
    summary: Option<&AgentSummary>,
    raw_agent_json: Option<&Value>,
) -> Option<String> {
    let from_summary = summary
        .and_then(|s| s.system.as_deref())
        .map(normalize_drift_text);

    if let Some(t) = from_summary.filter(|s| !s.is_empty()) {
        return Some(t);
    }

    if let Some(v) = raw_agent_json {
        let root = unwrap_letta_agent_document(v);
        let from_root = ["system", "system_prompt", "instructions", "instruction"]
            .into_iter()
            .find_map(|k| root.get(k).and_then(|x| x.as_str()))
            .map(normalize_drift_text)
            .filter(|s| !s.is_empty());
        if from_root.is_some() {
            return from_root;
        }
        if let Some(s) = fallback_system_from_blocks(root) {
            return Some(normalize_drift_text(&s));
        }
    }

    None
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
    compute_letta_drift_with_expected_tool_ids(bear, summary, diagnostics, raw_agent_json, None)
}

/// Same as [`compute_letta_drift`], but lets callers pass the exact tool ids Den attempted to
/// materialize in Letta. This matters because sync filters out legacy memory tools before PATCHing,
/// while the bear row keeps the operator's original selection for backwards-compatible forms.
pub fn compute_letta_drift_with_expected_tool_ids(
    bear: &Bear,
    summary: Option<&AgentSummary>,
    diagnostics: Option<&LettaAgentDiagnostics>,
    raw_agent_json: Option<&Value>,
    expected_tool_ids: Option<&[String]>,
) -> Option<LettaDriftFlags> {
    let summary = summary?;
    let diagnostics = diagnostics?;

    let system_prompt = letta_instruction_text_for_drift(Some(summary), raw_agent_json)
        .is_some_and(|letta_sys| !drift_text_matches_den_prompt(&bear.system_prompt, &letta_sys));

    let model =
        norm_opt_trim(bear.default_model.as_deref()) != norm_opt_trim(summary.model.as_deref());

    let db_at = norm_opt_trim(bear.letta_agent_type.as_deref());
    let letta_at = norm_opt_trim(summary.agent_type.as_deref());
    let agent_type = db_at != letta_at;

    let desired_tool_source = expected_tool_ids.unwrap_or(&bear.letta_tool_ids.0);
    let desired_tools = sorted_tool_ids(desired_tool_source);
    let letta_tools = sorted_tool_ids(
        &diagnostics
            .tools
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>(),
    );
    // Letta may attach server-managed/base tools that Den did not explicitly select. Those extras
    // should not make the details page say Den is out of sync immediately after a successful sync.
    // Drift here means a Den-managed desired tool is missing from Letta.
    let tools = desired_tools.iter().any(|id| !letta_tools.contains(id));

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
            runtime_plan: None,
            context_profile: None,
            memfs_repo_path: None,
            provisioning_version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
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
        assert!(!flags.system_prompt);
    }

    #[test]
    fn drift_system_when_instructions_key_differs() {
        let b = sample_bear();
        let v = json!({
            "id": "agent-1",
            "instructions": "edited in letta only",
            "model": "gpt-4o",
            "agent_type": "memgpt_agent",
            "tools": [{"id": "t1"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(flags.system_prompt);
    }

    #[test]
    fn drift_does_not_flag_compiled_system_that_contains_den_prompt() {
        let b = sample_bear();
        let v = json!({
            "id": "agent-1",
            "system": "Generated Letta wrapper\n\nSystem instructions:\nhello\n\nTool instructions...",
            "model": "gpt-4o",
            "agent_type": "memgpt_agent",
            "tools": [{"id": "t1"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(!flags.drift_any);
        assert!(!flags.system_prompt);
    }

    #[test]
    fn drift_does_not_flag_extra_server_managed_tools() {
        let b = sample_bear();
        let v = json!({
            "id": "agent-1",
            "system": "hello",
            "model": "gpt-4o",
            "agent_type": "memgpt_agent",
            "tools": [{"id": "server-tool"}, {"id": "t1"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let flags = compute_letta_drift(&b, Some(&s), Some(&d), Some(&v)).expect("drift");
        assert!(!flags.drift_any);
        assert!(!flags.tools);
    }

    #[test]
    fn drift_uses_filtered_expected_tools_when_provided() {
        let mut b = sample_bear();
        b.letta_tool_ids = Json(vec!["legacy-filtered-out".into(), "t1".into()]);
        let v = json!({
            "id": "agent-1",
            "system": "hello",
            "model": "gpt-4o",
            "agent_type": "memgpt_agent",
            "tools": [{"id": "t1"}]
        });
        let s = AgentSummary::from_letta_agent_state(&v);
        let d = LettaAgentDiagnostics::from_agent_json(&v);
        let expected = vec!["t1".to_string()];
        let flags = compute_letta_drift_with_expected_tool_ids(
            &b,
            Some(&s),
            Some(&d),
            Some(&v),
            Some(&expected),
        )
        .expect("drift");
        assert!(!flags.drift_any);
        assert!(!flags.tools);
    }
}
