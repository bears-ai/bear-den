//! OpenAI-compatible embeddings client (Bifrost / `LLM_API_URL`) for the derived recall
//! index (ADR-0038). Embeds passages and recall queries with the active platform standard
//! (`EMBEDDING_STANDARD` → `EMBEDDING_MODEL` / `EMBEDDING_DIMENSIONS`).
//!
//! Like the chat client, this routes through Bifrost using the `provider/model` handle
//! convention. Recall is optional: when `LLM_API_URL` is empty the client is disabled and
//! callers degrade to keyword search.

use std::time::Duration;

use serde_json::{json, Value};

mod credential;
mod failure;
pub use credential::BearEmbeddingCredential;
use failure::EmbeddingFailure;

use den_core::{config::Config, DenError};

use crate::client::normalize_llm_model_handle;

/// Embeds text via the OpenAI-compatible `/v1/embeddings` endpoint behind Bifrost.
#[derive(Clone)]
pub struct EmbeddingClient {
    http: reqwest::Client,
    base_url: String,
    credential: BearEmbeddingCredential,
    model: String,
    dimensions: u32,
}

impl EmbeddingClient {
    /// Production callers resolve the current Bear's credential for each operation.
    /// `llm_api_key` is not a substitute for Bear-scoped Bifrost authorization.
    pub fn new(config: &Config, credential: BearEmbeddingCredential) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_mins(1))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client for embeddings");
        Self {
            http,
            base_url: config.llm_api_url.trim_end_matches('/').to_string(),
            credential,
            model: normalize_llm_model_handle(&config.embedding_model),
            dimensions: config.embedding_dimensions,
        }
    }

    /// `false` when no inference substrate is configured (recall disabled → keyword fallback).
    pub fn is_enabled(&self) -> bool {
        !self.base_url.is_empty()
    }

    /// The normalized embedding model handle (e.g. `openai/text-embedding-3-small`).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The embedding vector dimensionality requested for the active standard.
    pub fn dimensions(&self) -> u32 {
        self.dimensions
    }

    /// Embed a single string, returning its vector.
    pub async fn embed_one(&self, input: &str) -> Result<Vec<f32>, DenError> {
        let mut vectors = self.embed(std::slice::from_ref(&input.to_string())).await?;
        vectors
            .pop()
            .ok_or_else(|| DenError::System("embeddings response contained no vectors".to_string()))
    }

    /// Embed a batch of strings, returning vectors in input order.
    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, DenError> {
        if !self.is_enabled() {
            return Err(EmbeddingFailure::NotConfigured.into());
        }
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/embeddings", self.base_url);
        let body = embedding_request_body(&self.model, inputs, self.dimensions);
        let resp = self
            .http
            .post(&url)
            .header("x-bf-vk", self.credential.header())
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                DenError::from(if error.is_timeout() {
                    EmbeddingFailure::Timeout
                } else {
                    EmbeddingFailure::Transport
                })
            })?;
        let status = resp.status();
        // Error bodies may echo credentials and input. Never read or project them.
        if !status.is_success() {
            return Err(EmbeddingFailure::Http(status).into());
        }
        let text = resp
            .text()
            .await
            .map_err(|_| DenError::from(EmbeddingFailure::ResponseRead))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|_| DenError::from(EmbeddingFailure::ResponseParse))?;
        parse_embedding_response(&value, inputs.len(), self.dimensions)
    }
}

/// Build the `/v1/embeddings` request body. `dimensions` is included only when non-zero so
/// the returned vector width matches the recall collection (supported by `text-embedding-3-*`).
fn embedding_request_body(model: &str, inputs: &[String], dimensions: u32) -> Value {
    let mut body = json!({
        "model": model,
        "input": inputs,
    });
    if dimensions > 0 {
        body["dimensions"] = json!(dimensions);
    }
    body
}

/// Parse an OpenAI-compatible embeddings response into vectors ordered by `index`,
/// validating count and (when configured) width.
fn parse_embedding_response(
    value: &Value,
    expected_count: usize,
    expected_dims: u32,
) -> Result<Vec<Vec<f32>>, DenError> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| DenError::System("embeddings response missing `data` array".to_string()))?;
    if data.len() != expected_count {
        return Err(DenError::System(format!(
            "embeddings response returned {} vectors, expected {expected_count}",
            data.len()
        )));
    }

    let mut indexed: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
    for (fallback_idx, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .map(|i| i as usize)
            .unwrap_or(fallback_idx);
        let embedding = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                DenError::System("embeddings response item missing `embedding`".to_string())
            })?;
        let vector: Vec<f32> = embedding
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| {
                DenError::System("embeddings response contained a non-numeric value".to_string())
            })?;
        if expected_dims > 0 && vector.len() != expected_dims as usize {
            return Err(DenError::System(format!(
                "embeddings vector width {} does not match configured dimensions {expected_dims}",
                vector.len()
            )));
        }
        indexed.push((index, vector));
    }

    indexed.sort_by_key(|(idx, _)| *idx);
    Ok(indexed.into_iter().map(|(_, v)| v).collect())
}

#[cfg(test)]
mod tests;
