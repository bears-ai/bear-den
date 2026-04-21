use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::StatusCode;
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
    ///
    /// Sends a **modern BEARS** profile aligned with `letta --new-agent` / memfs-oriented agents:
    /// - `include_base_tools: false` — do not attach legacy core_memory-style tools automatically.
    /// - `git_enabled: true` when accepted — enable git-backed / Context Repository memory on the Letta server.
    ///   If Letta returns **400/422** (validation / unknown field), Den **retries once without** `git_enabled`
    ///   so older servers still provision; success is logged at `warn` when the fallback path is used.
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

        let body_with_git = build_create_agent_body(
            name,
            system_prompt,
            model,
            agent_type,
            tool_ids,
            true,
        );
        let (status, text) = self.post_create_agent_raw(&body_with_git).await?;

        if status.is_success() {
            return parse_create_agent_id(&text);
        }

        if letta_status_suggests_retry_without_git(status) {
            tracing::warn!(
                %status,
                "Letta rejected POST /v1/agents with git_enabled; retrying without git_enabled"
            );
            let body_no_git = build_create_agent_body(
                name,
                system_prompt,
                model,
                agent_type,
                tool_ids,
                false,
            );
            let (status2, text2) = self.post_create_agent_raw(&body_no_git).await?;
            if status2.is_success() {
                tracing::warn!(
                    "Letta create agent succeeded without git_enabled (server may not support Context Repository on this endpoint)"
                );
                return parse_create_agent_id(&text2);
            }
            return Err(CustomError::System(format!(
                "Letta create agent HTTP {status2}: {text2}\n\
                 (Earlier attempt with git_enabled returned HTTP {status}: {text})"
            )));
        }

        Err(CustomError::System(format!(
            "Letta create agent HTTP {status}: {text}"
        )))
    }

    /// Resolve `tool_ids` using `GET /v1/tools/` and remove legacy memory mutation tools by name.
    pub async fn filtered_tool_ids(&self, selected: &[String]) -> Result<Vec<String>, CustomError> {
        let catalog = self.list_tools().await?;
        Ok(super::tool_policy::filter_legacy_memory_tool_ids(
            &catalog, selected,
        ))
    }

    /// `POST /v1/agents/{id}/messages/stream` (Letta API v1 / SDK-style streaming).
    ///
    /// Prefer [`Self::post_conversation_messages_streaming`] for new code paths; this remains for
    /// callers that target the legacy agent-default endpoint directly.
    #[allow(dead_code)]
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

    /// `GET /v1/conversations/` — conversations for one agent (`order_by=last_message_at`, newest first).
    pub async fn list_conversations_for_agent(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> Result<serde_json::Value, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let limit = limit.clamp(1, 200);
        let limit_str = limit.to_string();
        let url = format!("{}/v1/conversations/", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .query(&[
                ("agent_id", agent_id),
                ("limit", limit_str.as_str()),
                ("order_by", "last_message_at"),
                ("order", "desc"),
            ])
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list conversations request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list conversations body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list conversations HTTP {status}: {text}"
            )));
        }

        serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Letta list conversations JSON: {e}; body: {text}"))
        })
    }

    /// `GET /v1/conversations/{conversation_id}/messages` — paginated history for one conversation.
    ///
    /// For the agent default thread, pass `conversation_id == "default"` and `Some(agent_id)`.
    ///
    /// `oldest_first`: when `true`, uses `order=asc` (first page = oldest messages) per Letta API.
    pub async fn list_conversation_messages(
        &self,
        conversation_id: &str,
        agent_id: Option<&str>,
        limit: u32,
        before: Option<&str>,
        oldest_first: bool,
    ) -> Result<serde_json::Value, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let limit = limit.clamp(1, 100);
        let limit_str = limit.to_string();
        let order = if oldest_first { "asc" } else { "desc" };
        let url = format!(
            "{}/v1/conversations/{}/messages",
            self.base_url,
            conversation_id
        );

        let mut req = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .query(&[("order", order), ("limit", limit_str.as_str())]);

        if let Some(a) = agent_id.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("agent_id", a)]);
        }
        if let Some(b) = before.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("before", b)]);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta list conversation messages failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta list conversation messages body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta list conversation messages HTTP {status}: {text}"
            )));
        }

        serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!(
                "Letta list conversation messages JSON: {e}; body: {text}"
            ))
        })
    }

    /// `PATCH /v1/conversations/{conversation_id}` — set `summary` (human-facing thread title).
    pub async fn patch_conversation_summary(
        &self,
        conversation_id: &str,
        summary: &str,
    ) -> Result<(), CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!(
            "{}/v1/conversations/{}",
            self.base_url, conversation_id
        );
        let body = json!({ "summary": summary });
        let resp = self
            .http
            .patch(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch conversation failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch conversation body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta patch conversation HTTP {status}: {text}"
            )));
        }

        Ok(())
    }

    /// `POST /v1/conversations/{conversation_id}/messages` with `streaming: true` (SSE).
    ///
    /// For the agent default thread, pass `conversation_id == "default"` and `Some(agent_id)` in the body.
    ///
    /// **Den web chat** does not call this for end-user streaming: it uses [`crate::core::codepool::CodePoolClient`]
    /// (`CODEPOOL_BASE_URL`). This method remains for tests, tooling, or non-web callers.
    pub async fn post_conversation_messages_streaming(
        &self,
        conversation_id: &str,
        agent_id: Option<&str>,
        user_input: &str,
    ) -> Result<reqwest::Response, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let mut body = serde_json::Map::new();
        body.insert(
            "messages".to_string(),
            json!([{
                "role": "user",
                "content": user_input,
            }]),
        );
        body.insert("streaming".to_string(), json!(true));
        body.insert("stream_tokens".to_string(), json!(true));
        if let Some(a) = agent_id.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("agent_id".to_string(), json!(a));
        }

        let url = format!(
            "{}/v1/conversations/{}/messages",
            self.base_url, conversation_id
        );

        let resp = self
            .http
            .post(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta conversation messages request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".to_string());
            return Err(CustomError::System(format!(
                "Letta conversation messages HTTP {status}: {text}"
            )));
        }

        Ok(resp)
    }

    /// `GET /v1/agents/{id}/messages` — paginated message history (`order`, `limit`, optional `before` cursor).
    #[allow(dead_code)]
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
    ///
    /// Like [`Self::create_agent`], retries once **without** `git_enabled` on **400/422** responses.
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

        let body_with_git = build_patch_agent_body(
            name,
            description,
            system,
            model,
            agent_type,
            tool_ids,
            true,
        );
        let (status, text) = self
            .post_patch_agent_raw(agent_id, &body_with_git)
            .await?;

        if status.is_success() {
            return Ok(());
        }

        if letta_status_suggests_retry_without_git(status) {
            tracing::warn!(
                %status,
                agent_id,
                "Letta rejected PATCH /v1/agents with git_enabled; retrying without git_enabled"
            );
            let body_no_git = build_patch_agent_body(
                name,
                description,
                system,
                model,
                agent_type,
                tool_ids,
                false,
            );
            let (status2, text2) = self
                .post_patch_agent_raw(agent_id, &body_no_git)
                .await?;
            if status2.is_success() {
                tracing::warn!(
                    agent_id,
                    "Letta patch agent succeeded without git_enabled (server may not support Context Repository on this endpoint)"
                );
                return Ok(());
            }
            return Err(CustomError::System(format!(
                "Letta patch agent HTTP {status2}: {text2}\n\
                 (Earlier attempt with git_enabled returned HTTP {status}: {text})"
            )));
        }

        Err(CustomError::System(format!(
            "Letta patch agent HTTP {status}: {text}"
        )))
    }

    async fn post_create_agent_raw(
        &self,
        body: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(StatusCode, String), CustomError> {
        let url = format!("{}/v1/agents", self.base_url);
        let resp = self
            .http
            .post(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta create agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta create agent body: {e}")))?;
        Ok((status, text))
    }

    async fn post_patch_agent_raw(
        &self,
        agent_id: &str,
        body: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(StatusCode, String), CustomError> {
        let url = format!("{}/v1/agents/{agent_id}", self.base_url);
        let resp = self
            .http
            .patch(url)
            .headers(self.auth_headers())
            .header(CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta patch agent body: {e}")))?;
        Ok((status, text))
    }

    /// `POST /v1/agents/{agent_id}/recompile` — materialize the compiled system prompt after PATCH
    /// (Letta treats this as a separate step from updating stored `system` / blocks).
    pub async fn recompile_agent(&self, agent_id: &str) -> Result<(), CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Letta is not configured (set LETTA_BASE_URL)".to_string(),
            ));
        }

        let url = format!("{}/v1/agents/{agent_id}/recompile", self.base_url);
        let resp = self
            .http
            .post(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Letta recompile agent request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Letta recompile agent body: {e}")))?;

        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Letta recompile agent HTTP {status}: {text}"
            )));
        }

        Ok(())
    }
}

/// 400/422 usually mean validation or unknown fields (e.g. Letta build without `git_enabled` on this route).
fn letta_status_suggests_retry_without_git(status: StatusCode) -> bool {
    matches!(status.as_u16(), 400 | 422)
}

fn build_create_agent_body(
    name: &str,
    system_prompt: &str,
    model: Option<&str>,
    agent_type: Option<&str>,
    tool_ids: &[String],
    git_enabled: bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("system".to_string(), json!(system_prompt));
    body.insert("include_base_tools".to_string(), json!(false));
    if git_enabled {
        body.insert("git_enabled".to_string(), json!(true));
        // With local memfs (`LETTA_MEMFS_SERVICE_URL=local`), Letta Code expects git-backed agents.
        body.insert(
            "tags".to_string(),
            json!(vec!["git-memory-enabled".to_string()]),
        );
    }
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        body.insert("model".to_string(), json!(m));
    }
    if let Some(t) = agent_type.map(str::trim).filter(|s| !s.is_empty()) {
        body.insert("agent_type".to_string(), json!(t));
    }
    if !tool_ids.is_empty() {
        body.insert("tool_ids".to_string(), json!(tool_ids));
    }
    body
}

fn parse_create_agent_id(text: &str) -> Result<String, CustomError> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| {
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

fn build_patch_agent_body(
    name: &str,
    description: &str,
    system: &str,
    model: Option<&str>,
    agent_type: Option<&str>,
    tool_ids: &[String],
    git_enabled: bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("description".to_string(), json!(description));
    body.insert("system".to_string(), json!(system));
    body.insert("include_base_tools".to_string(), json!(false));
    if git_enabled {
        body.insert("git_enabled".to_string(), json!(true));
    }
    if let Some(m) = model.map(str::trim).filter(|s| !s.is_empty()) {
        body.insert("model".to_string(), json!(m));
    }
    if let Some(t) = agent_type.map(str::trim).filter(|s| !s.is_empty()) {
        body.insert("agent_type".to_string(), json!(t));
    }
    body.insert("tool_ids".to_string(), json!(tool_ids));
    body
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
