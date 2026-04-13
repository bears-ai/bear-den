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

    /// `POST /v1/agents` — returns Letta agent id (e.g. `agent-…`).
    pub async fn create_agent(
        &self,
        name: &str,
        system_prompt: &str,
        model: Option<&str>,
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

    /// `POST /v1/agents/{id}/messages` with `streaming: true`. Caller consumes the body stream.
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
            "input": user_input,
            "streaming": true,
            "include_pings": true,
        });

        let url = format!("{}/v1/agents/{agent_id}/messages", self.base_url);

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
}
