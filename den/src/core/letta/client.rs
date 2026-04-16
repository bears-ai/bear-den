use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::json;

use crate::{config::Config, errors::CustomError};

/// One row from `GET /v1/models/` suitable for `<select>` options (LLM only).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LettaModelOption {
    /// Value for Letta `model` / Den `default_model` (e.g. `openai/gpt-4o`).
    pub handle: String,
    /// Human-facing label for the operator UI.
    pub label: String,
}

/// One row from `GET /v1/tools/` for multi-select (`tool_ids` on create/patch).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LettaToolOption {
    pub id: String,
    pub label: String,
}

/// Minimal row from `GET /v1/agents` for admin orphan-agent listing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LettaAgentListItem {
    pub id: String,
    pub name: Option<String>,
}

/// Thin Letta REST client (create agent, stream chat). Disabled when `letta_base_url` is empty.
#[derive(Clone)]
pub struct LettaClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl LettaClient {
    pub fn new(config: &Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: config.letta_base_url.trim_end_matches('/').to_string(),
            api_key: config.letta_api_key.trim().to_string(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.base_url.is_empty()
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if !self.api_key.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", self.api_key)) {
                h.insert(AUTHORIZATION, v);
            }
        }
        h
    }

    /// `GET /v1/models/` — LLM handles for agent creation (`model` must be set for provisioning).
    pub async fn list_llm_models(&self) -> Result<Vec<LettaModelOption>, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/models/", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list models request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list models body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list models HTTP {status}: {text}"
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta list models JSON: {e}; body: {text}"))
        })?;

        Ok(parse_letta_llm_model_list(&v))
    }

    /// `GET /v1/tools/` — tool ids for agent `tool_ids`.
    pub async fn list_tools(&self) -> Result<Vec<LettaToolOption>, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/tools/", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list tools request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list tools body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list tools HTTP {status}: {text}"
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta list tools JSON: {e}; body: {text}"))
        })?;

        Ok(parse_letta_tool_list(&v))
    }

    /// `POST /v1/agents` — returns Letta agent id (e.g. `agent-…`).
    pub async fn create_agent(
        &self,
        name: &str,
        system_prompt: &str,
        model: Option<&str>,
        agent_type: Option<&str>,
        tool_ids: &[String],
    ) -> Result<String, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), json!(name));
        body.insert("system".to_string(), json!(system_prompt));
        if let Some(m) = model.filter(|s| !s.is_empty()) {
            body.insert("model".to_string(), json!(m));
        }
        if let Some(t) = agent_type.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("agent_type".to_string(), json!(t));
        }
        if !tool_ids.is_empty() {
            body.insert("tool_ids".to_string(), json!(tool_ids));
        }

        let url = format!("{}/v1/agents", self.base_url);
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta create agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta create agent body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta create agent HTTP {status}: {text}"
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta create agent JSON: {e}; body: {text}"))
        })?;

        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| {
                CustomError::Parsing(format!(
                    "Letta create agent response missing id: {text}"
                ))
            })?
            .to_string();

        Ok(id)
    }

    /// `POST /v1/agents/{id}/messages/stream` (Letta API v1 / SDK-style streaming).
    ///
    /// Older servers accepted `POST …/messages` with `{ input, streaming: true }`; current Letta
    /// returns 422 for that shape and expects this dedicated SSE endpoint with a `messages` array.
    pub async fn post_messages_streaming(
        &self,
        agent_id: &str,
        user_input: &str,
    ) -> Result<reqwest::Response, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let body = json!({
            "messages": [{
                "role": "user",
                "content": user_input,
            }],
            "stream_tokens": true,
        });

        let url = format!("{}/v1/agents/{agent_id}/messages/stream", self.base_url);

        let resp = self
            .http
            .post(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta messages request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".to_string());
            return Err(CustomError::System(format!(
                "Letta messages HTTP {status}: {text}"
            )));
        }

        Ok(resp)
    }

    /// `GET /v1/agents/{id}/messages` — paginated message history (`order`, `limit`, optional `before` cursor).
    pub async fn list_agent_messages(
        &self,
        agent_id: &str,
        limit: u32,
        before: Option<&str>,
    ) -> Result<serde_json::Value, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let limit = limit.clamp(1, 100);
        let limit_str = limit.to_string();
        let url = format!("{}/v1/agents/{agent_id}/messages", self.base_url);

        let mut req = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .query(&[("order", "desc"), ("limit", limit_str.as_str())]);

        if let Some(b) = before.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("before", b)]);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list messages request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list messages body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list messages HTTP {status}: {text}"
            )));
        }

        serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta list messages JSON: {e}; body: {text}"))
        })
    }

    /// `GET /v1/health` — used by operator console and deploy health checks.
    pub async fn check_health(&self) -> Result<String, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/health", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta health request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta health body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta health HTTP {status}: {text}"
            )));
        }

        Ok(text)
    }

    /// `GET /v1/agents/{agent_id}` — full JSON for operator diagnostics.
    ///
    /// Requests `include=agent.blocks` and `include=agent.tools` because current Letta
    /// APIs omit those relationships by default unless asked (see Letta retrieve-agent docs).
    pub async fn fetch_agent(&self, agent_id: &str) -> Result<serde_json::Value, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/agents/{agent_id}", self.base_url);
        let resp = self
            .http
            .get(url)
            .query(&[
                ("include", "agent.blocks"),
                ("include", "agent.tools"),
            ])
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta get agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta get agent body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta get agent HTTP {status}: {text}"
            )));
        }

        serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta get agent JSON: {e}; body: {text}"))
        })
    }

    /// `GET /v1/agents` — list agents (shape varies by server; ids required).
    pub async fn list_agents(&self) -> Result<Vec<LettaAgentListItem>, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/agents", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list agents request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list agents body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list agents HTTP {status}: {text}"
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta list agents JSON: {e}; body: {text}"))
        })?;

        Ok(parse_letta_agent_list(&v))
    }

    /// `PATCH /v1/agents/{agent_id}` — align Letta agent with Den bear fields.
    pub async fn patch_agent(
        &self,
        agent_id: &str,
        name: &str,
        description: &str,
        system: &str,
        model: Option<&str>,
        agent_type: Option<&str>,
        tool_ids: &[String],
    ) -> Result<(), CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), json!(name));
        body.insert("description".to_string(), json!(description));
        body.insert("system".to_string(), json!(system));
        if let Some(m) = model.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("model".to_string(), json!(m));
        }
        if let Some(t) = agent_type.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("agent_type".to_string(), json!(t));
        }
        body.insert("tool_ids".to_string(), json!(tool_ids));

        let url = format!("{}/v1/agents/{agent_id}", self.base_url);
        let resp = self
            .http
            .patch(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch agent body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta patch agent HTTP {status}: {text}"
            )));
        }

        Ok(())
    }
}

fn parse_letta_llm_model_list(v: &serde_json::Value) -> Vec<LettaModelOption> {
    let items: &[serde_json::Value] = if let Some(a) = v.as_array() {
        a.as_slice()
    } else if let Some(a) = v.get("models").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        a.as_slice()
    } else {
        &[]
    };

    let mut out: Vec<LettaModelOption> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();

    for item in items {
        if item.get("model_type").and_then(|x| x.as_str()) == Some("embedding") {
            continue;
        }

        let handle = item
            .get("handle")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                let provider = item.get("provider_type").and_then(|x| x.as_str())?;
                let name = item
                    .get("name")
                    .and_then(|x| x.as_str())
                    .or(item.get("model").and_then(|x| x.as_str()))?;
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                Some(format!("{provider}/{name}"))
            });

        let Some(handle) = handle else {
            continue;
        };

        if !seen.insert(handle.clone()) {
            continue;
        }

        let label = item
            .get("display_name")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| handle.clone());

        out.push(LettaModelOption { handle, label });
    }

    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn parse_letta_agent_list(v: &serde_json::Value) -> Vec<LettaAgentListItem> {
    let items: &[serde_json::Value] = if let Some(a) = v.as_array() {
        a.as_slice()
    } else if let Some(a) = v.get("agents").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        a.as_slice()
    } else {
        &[]
    };

    let mut out: Vec<LettaAgentListItem> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();

    for item in items {
        let id = item
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let Some(id) = id else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }

        let name = item
            .get("name")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        out.push(LettaAgentListItem { id, name });
    }

    out.sort_by(|a, b| {
        let an = a.name.as_deref().unwrap_or(&a.id);
        let bn = b.name.as_deref().unwrap_or(&b.id);
        an.cmp(bn).then_with(|| a.id.cmp(&b.id))
    });
    out
}

fn parse_letta_tool_list(v: &serde_json::Value) -> Vec<LettaToolOption> {
    let items: &[serde_json::Value] = if let Some(a) = v.as_array() {
        a.as_slice()
    } else if let Some(a) = v.get("tools").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.as_slice()
    } else if let Some(a) = v.get("items").and_then(|x| x.as_array()) {
        a.as_slice()
    } else {
        &[]
    };

    let mut out: Vec<LettaToolOption> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();

    for item in items {
        let id = item
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let Some(id) = id else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }

        let label = item
            .get("name")
            .or_else(|| item.get("tool_name"))
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| id.clone());

        out.push(LettaToolOption { id, label });
    }

    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_models_accepts_top_level_array() {
        let v = serde_json::json!([
            {"handle": "openai/gpt-4o", "display_name": "GPT-4o", "model_type": "llm"},
            {"handle": "anthropic/claude-3-5-sonnet-20241022", "model_type": "llm"}
        ]);
        let m = parse_letta_llm_model_list(&v);
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|x| x.handle == "openai/gpt-4o"));
    }

    #[test]
    fn parse_models_skips_embedding_and_de_duplicates() {
        let v = serde_json::json!({
            "models": [
                {"handle": "x/y", "model_type": "embedding"},
                {"handle": "openai/gpt-4o", "model_type": "llm"},
                {"handle": "openai/gpt-4o", "model_type": "llm"}
            ]
        });
        let m = parse_letta_llm_model_list(&v);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn parse_models_synthesizes_handle_from_provider_and_name() {
        let v = serde_json::json!([
            {"provider_type": "openai", "name": "gpt-4o-mini", "model_type": "llm"}
        ]);
        let m = parse_letta_llm_model_list(&v);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].handle, "openai/gpt-4o-mini");
    }

    #[test]
    fn disabled_when_base_empty() {
        let mut c = Config::test_stub();
        c.letta_base_url = String::new();
        let client = LettaClient::new(&c);
        assert!(!client.is_enabled());
    }

    #[test]
    fn parse_tools_top_level_array() {
        let v = serde_json::json!([
            {"id": "tool-a", "name": "Alpha"},
            {"id": "tool-b", "name": "Beta"}
        ]);
        let t = parse_letta_tool_list(&v);
        assert_eq!(t.len(), 2);
        assert!(t.iter().any(|x| x.id == "tool-a" && x.label == "Alpha"));
    }

    #[test]
    fn parse_tools_de_duplicates_by_id() {
        let v = serde_json::json!({
            "tools": [
                {"id": "x", "name": "one"},
                {"id": "x", "name": "two"}
            ]
        });
        let t = parse_letta_tool_list(&v);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn parse_agents_top_level_array_sorted_by_name() {
        let v = serde_json::json!([
            {"id": "agent-b", "name": "Beta"},
            {"id": "agent-a", "name": "Alpha"}
        ]);
        let a = parse_letta_agent_list(&v);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].id, "agent-a");
        assert_eq!(a[1].id, "agent-b");
    }

    #[test]
    fn parse_agents_nested_skips_missing_id() {
        let v = serde_json::json!({
            "agents": [
                {"name": "no id"},
                {"id": "agent-1", "name": "One"}
            ]
        });
        let a = parse_letta_agent_list(&v);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].id, "agent-1");
    }
}
