use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::json;
use uuid::Uuid;

use crate::{config::Config, errors::CustomError};

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

    /// Same contract as [`crate::core::letta::LettaClient::post_conversation_messages_streaming`],
    /// plus `bear_id` and `runtime_plan` for codepool memfs provisioning.
    pub async fn post_conversation_messages_streaming(
        &self,
        conversation_id: &str,
        agent_id: Option<&str>,
        user_input: &str,
        bear_id: Uuid,
        runtime_plan: &serde_json::Value,
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
            .map_err(|e| {
                CustomError::System(format!("Codepool conversation messages request failed: {e}"))
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
