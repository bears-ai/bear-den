use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::core::bears::model::{Bear, BearAgentRole};

use crate::{config::Config, errors::CustomError};

pub struct BearRuntimeMessageRequest<'a> {
    pub session_id: &'a str,
    pub conversation_id: &'a str,
    pub bear: &'a Bear,
    pub role_agent_id: &'a str,
    pub user_id: i32,
    pub username: Option<&'a str>,
    pub membership_role: Option<&'a str>,
    pub user_input: &'a str,
    pub runtime_plan: &'a serde_json::Value,
    pub request_id: Uuid,
}

pub struct BearChannelMessageRequest<'a> {
    pub runtime: BearRuntimeMessageRequest<'a>,
    pub channel_family: &'a str,
    pub channel_client: &'a str,
    pub channel_protocol: &'a str,
    pub supports_cancellation: bool,
    pub supports_rich_events: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodepoolCancelResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodepoolMemfsCheck {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub remote_url_source: String,
    #[serde(default)]
    pub ls_remote: CodepoolMemfsLsRemote,
    #[serde(default)]
    pub clone: Option<CodepoolMemfsClone>,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodepoolMemfsLsRemote {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub refs: Vec<CodepoolMemfsRef>,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodepoolMemfsRef {
    #[serde(default)]
    pub sha: String,
    #[serde(default, rename = "ref")]
    pub ref_: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodepoolMemfsClone {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub file_count: Option<i64>,
    #[serde(default)]
    pub files: Vec<String>,
}

/// Runtime abstraction for Den -> bear harness message streaming.
///
/// `CodePoolClient` is the first implementation. Keeping this trait near the concrete client
/// lets Den introduce ACP/native test clients later without changing web chat handlers.
#[async_trait]
pub trait BearRuntimeClient {
    async fn post_bear_channel_message_streaming(
        &self,
        request: BearRuntimeMessageRequest<'_>,
    ) -> Result<reqwest::Response, CustomError>;
}

/// HTTP client for **Codepool** (Letta Code SDK harness). Disabled when `codepool_base_url` is empty.
#[derive(Clone)]
pub struct CodePoolClient {
    http: reqwest::Client,
    base_url: String,
    internal_token: String,
}

impl CodePoolClient {
    pub fn new(config: &Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: config.codepool_base_url.trim_end_matches('/').to_string(),
            internal_token: config.codepool_internal_token.trim().to_string(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.base_url.is_empty()
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if !self.internal_token.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", self.internal_token)) {
                h.insert(AUTHORIZATION, v);
            }
        }
        h
    }

    /// `GET /internal/pool` — conversation + channel listener stats (JSON).
    pub async fn fetch_pool_stats(&self) -> Result<String, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "CODEPOOL_BASE_URL is not set".to_string(),
            ));
        }
        let url = format!("{}/internal/pool", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Codepool pool stats request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Codepool pool stats body: {e}")))?;
        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Codepool pool stats HTTP {status}: {text}"
            )));
        }
        Ok(text)
    }

    /// `GET /internal/memfs/{agent_id}/check` — non-mutating git remote validation.
    pub async fn fetch_memfs_check(
        &self,
        agent_id: &str,
        clone: bool,
    ) -> Result<CodepoolMemfsCheck, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "CODEPOOL_BASE_URL is not set".to_string(),
            ));
        }
        let mode = if clone { "clone" } else { "ls-remote" };
        let url = format!(
            "{}/internal/memfs/{}/check?mode={}",
            self.base_url,
            urlencoding::encode(agent_id.trim()),
            mode
        );
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| {
                CustomError::System(format!("Codepool memfs check request failed: {e}"))
            })?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Codepool memfs check body: {e}")))?;
        if !(status.is_success() || status == reqwest::StatusCode::BAD_GATEWAY) {
            return Err(CustomError::System(format!(
                "Codepool memfs check HTTP {status}: {text}"
            )));
        }
        serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Codepool memfs check JSON: {e}; body: {text}"))
        })
    }

    /// `GET /version` — npm `version`, `git_sha` from image (same auth pattern as health).
    pub async fn fetch_version_json(&self) -> Result<String, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "CODEPOOL_BASE_URL is not set".to_string(),
            ));
        }
        let url = format!("{}/version", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Codepool version request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Codepool version body: {e}")))?;
        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Codepool version HTTP {status}: {text}"
            )));
        }
        Ok(text)
    }

    pub async fn post_bear_channel_cancel(
        &self,
        session_id: &str,
        request_id: Uuid,
    ) -> Result<CodepoolCancelResponse, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Codepool is not configured (set CODEPOOL_BASE_URL)".to_string(),
            ));
        }
        let url = format!(
            "{}/internal/bear_channel/sessions/{}/cancel",
            self.base_url,
            urlencoding::encode(session_id),
        );
        let mut headers = self.auth_headers();
        if let Ok(v) = HeaderValue::from_str(&request_id.to_string()) {
            headers.insert(HeaderName::from_static("x-request-id"), v);
        }
        let resp = self
            .http
            .post(url)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({ "request_id": request_id.to_string() }))
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Codepool cancel request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Codepool cancel body: {e}")))?;
        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Codepool cancel HTTP {status}: {text}"
            )));
        }
        serde_json::from_str(&text)
            .map_err(|e| CustomError::Parsing(format!("Codepool cancel JSON: {e}; body: {text}")))
    }

    /// `GET /health`
    pub async fn check_health(&self) -> Result<String, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "CODEPOOL_BASE_URL is not set".to_string(),
            ));
        }
        let url = format!("{}/health", self.base_url);
        let resp = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| CustomError::System(format!("Codepool health request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CustomError::System(format!("Codepool health body: {e}")))?;
        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Codepool health HTTP {status}: {text}"
            )));
        }
        Ok(text)
    }

    /// `bear_channel` is the preferred Den → Codepool runtime boundary. Den keeps external
    /// authentication and browser/API compatibility; Codepool receives trusted bear/user/channel
    /// context and manages the warm Letta Code runtime.
    pub async fn post_bear_channel_message_streaming(
        &self,
        request: BearRuntimeMessageRequest<'_>,
    ) -> Result<reqwest::Response, CustomError> {
        self.post_bear_channel_message_for_channel_streaming(BearChannelMessageRequest {
            runtime: request,
            channel_family: "browser_chat",
            channel_client: "den_web",
            channel_protocol: "den_chat",
            supports_cancellation: false,
            supports_rich_events: true,
        })
        .await
    }

    /// Lower-level `bear_channel` sender for non-browser Letta Code-backed clients.
    ///
    /// ACP no longer uses this path; ACP is API-direct to the Bear's `pair` role.
    pub async fn post_bear_channel_message_for_channel_streaming(
        &self,
        request: BearChannelMessageRequest<'_>,
    ) -> Result<reqwest::Response, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Codepool is not configured (set CODEPOOL_BASE_URL)".to_string(),
            ));
        }

        let BearChannelMessageRequest {
            runtime,
            channel_family,
            channel_client,
            channel_protocol,
            supports_cancellation,
            supports_rich_events,
        } = request;
        let BearRuntimeMessageRequest {
            session_id,
            conversation_id,
            bear,
            role_agent_id,
            user_id,
            username,
            membership_role,
            user_input,
            runtime_plan,
            request_id,
        } = runtime;

        let agent_role = BearAgentRole::Talk;
        let agent_id = role_agent_id.trim();
        if agent_id.is_empty() {
            return Err(CustomError::System(
                "This bear is not provisioned in Letta yet (missing talk role agent).".to_string(),
            ));
        }

        let body = json!({
            "session_id": session_id,
            "conversation_id": conversation_id,
            "bear": {
                "id": bear.id.to_string(),
                "slug": bear.slug,
                "name": bear.name,
                "agent_role": agent_role.as_str(),
                "role_agent_id": agent_id,
                "runtime_family": agent_role.runtime_family(),
            },
            "user": {
                "id": user_id,
                "username": username,
                "membership_role": membership_role,
            },
            "channel": {
                "family": channel_family,
                "client": channel_client,
                "protocol": channel_protocol,
            },
            "message": {
                "type": "text",
                "content": user_input,
            },
            "capabilities": {
                "server_tools": crate::core::den_tools::builtin_den_tool_descriptors_for_role(agent_role),
                "supports_cancellation": supports_cancellation,
                "supports_rich_events": supports_rich_events,
            },
            "runtime_plan": runtime_plan,
            "request_id": request_id.to_string(),
        });

        let url = format!(
            "{}/internal/bear_channel/sessions/{}/messages",
            self.base_url,
            urlencoding::encode(session_id),
        );

        let mut headers = self.auth_headers();
        if let Ok(v) = HeaderValue::from_str(&request_id.to_string()) {
            headers.insert(HeaderName::from_static("x-request-id"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&bear.id.to_string()) {
            headers.insert(HeaderName::from_static("x-bear-id"), v);
        }

        let resp = self
            .http
            .post(url)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                CustomError::System(format!("Codepool bear_channel request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".to_string());
            return Err(CustomError::System(format!(
                "Codepool bear_channel HTTP {status}: {text}"
            )));
        }

        Ok(resp)
    }

    /// Same contract as [`crate::core::letta::LettaClient::post_conversation_messages_streaming`],
    /// plus `bear_id`, `role_agent_id`, and `runtime_plan` for codepool memfs provisioning.
    /// Kept for compatibility; Den web chat should use [`Self::post_bear_channel_message_streaming`].
    pub async fn post_conversation_messages_streaming(
        &self,
        conversation_id: &str,
        agent_id: Option<&str>,
        user_input: &str,
        bear_id: Uuid,
        runtime_plan: &serde_json::Value,
        request_id: Uuid,
    ) -> Result<reqwest::Response, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Codepool is not configured (set CODEPOOL_BASE_URL)".to_string(),
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
        body.insert("bear_id".to_string(), json!(bear_id.to_string()));
        body.insert("runtime_plan".to_string(), runtime_plan.clone());
        let role_agent_id = agent_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CustomError::System("role_agent_id is required for Codepool".to_string())
            })?;
        body.insert("role_agent_id".to_string(), json!(role_agent_id));

        let url = format!(
            "{}/v1/conversations/{}/messages",
            self.base_url, conversation_id
        );

        let mut headers = self.auth_headers();
        if let Ok(v) = HeaderValue::from_str(&request_id.to_string()) {
            headers.insert(HeaderName::from_static("x-request-id"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&bear_id.to_string()) {
            headers.insert(HeaderName::from_static("x-bear-id"), v);
        }

        let resp = self
            .http
            .post(url)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                CustomError::System(format!(
                    "Codepool conversation messages request failed: {e}"
                ))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".to_string());
            return Err(CustomError::System(format!(
                "Codepool conversation messages HTTP {status}: {text}"
            )));
        }

        Ok(resp)
    }
}

#[async_trait]
impl BearRuntimeClient for CodePoolClient {
    async fn post_bear_channel_message_streaming(
        &self,
        request: BearRuntimeMessageRequest<'_>,
    ) -> Result<reqwest::Response, CustomError> {
        CodePoolClient::post_bear_channel_message_streaming(self, request).await
    }
}
