use std::time::Duration;

use serde::Deserialize;

use crate::{config::Config, core::letta::LettaModelOption, errors::CustomError};

#[derive(Debug, Clone, Deserialize)]
pub struct BifrostModelMetadata {
    pub handle: String,
    #[allow(dead_code)]
    pub provider: String,
    #[allow(dead_code)]
    pub model: String,
    pub display_name: Option<String>,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[allow(dead_code)]
    pub supports_tools: Option<bool>,
    #[allow(dead_code)]
    pub supports_responses_api: Option<bool>,
    #[allow(dead_code)]
    pub supports_vision: Option<bool>,
}

impl BifrostModelMetadata {
    pub fn to_letta_model_option(&self) -> LettaModelOption {
        let label = match (self.display_name.as_deref(), self.max_output_tokens) {
            (Some(name), Some(out)) => format!(
                "{} ({} ctx / {} out)",
                name,
                format_tokens(self.context_window),
                format_tokens(out)
            ),
            (Some(name), None) => format!("{} ({} ctx)", name, format_tokens(self.context_window)),
            (None, Some(out)) => format!(
                "{} ({} ctx / {} out)",
                self.handle,
                format_tokens(self.context_window),
                format_tokens(out)
            ),
            (None, None) => format!(
                "{} ({} ctx)",
                self.handle,
                format_tokens(self.context_window)
            ),
        };
        LettaModelOption {
            handle: self.handle.clone(),
            label,
            context_window: Some(self.context_window),
            max_output_tokens: self.max_output_tokens,
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn format_tokens(n: u32) -> String {
    if n >= 1_000_000 {
        let whole = n / 1_000_000;
        let frac = (n % 1_000_000) / 100_000;
        if frac == 0 {
            format!("{whole}M")
        } else {
            format!("{whole}.{frac}M")
        }
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct BifrostModelMetadataResponse {
    models: Vec<BifrostModelMetadata>,
}

#[derive(Clone)]
pub struct BifrostClient {
    http: reqwest::Client,
    metadata_url: String,
}

impl BifrostClient {
    pub fn new(config: &Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        Self {
            http,
            metadata_url: config.bifrost_metadata_url.trim().to_string(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.metadata_url.is_empty()
    }

    pub async fn list_models(&self) -> Result<Vec<BifrostModelMetadata>, CustomError> {
        if !self.is_enabled() {
            return Err(CustomError::System(
                "Bifrost metadata is not configured (set BIFROST_METADATA_URL)".to_string(),
            ));
        }

        let resp = self
            .http
            .get(&self.metadata_url)
            .send()
            .await
            .map_err(|e| {
                CustomError::System(format!("Bifrost model metadata request failed: {e}"))
            })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| {
            CustomError::System(format!("Bifrost model metadata response body: {e}"))
        })?;
        if !status.is_success() {
            return Err(CustomError::System(format!(
                "Bifrost model metadata HTTP {status}: {text}"
            )));
        }

        let payload: BifrostModelMetadataResponse = serde_json::from_str(&text).map_err(|e| {
            CustomError::Parsing(format!("Bifrost model metadata JSON: {e}; body: {text}"))
        })?;

        let mut models: Vec<BifrostModelMetadata> = payload
            .models
            .into_iter()
            .filter(|m| m.enabled && !m.handle.trim().is_empty())
            .collect();
        models.sort_by(|a, b| {
            a.display_name
                .as_deref()
                .unwrap_or(&a.handle)
                .cmp(b.display_name.as_deref().unwrap_or(&b.handle))
        });
        Ok(models)
    }

    pub async fn get_model(
        &self,
        handle: &str,
    ) -> Result<Option<BifrostModelMetadata>, CustomError> {
        let handle = handle.trim();
        if handle.is_empty() {
            return Ok(None);
        }
        Ok(self
            .list_models()
            .await?
            .into_iter()
            .find(|m| m.handle == handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_large_context_labels() {
        let m = BifrostModelMetadata {
            handle: "gpt-4.1".into(),
            provider: "openai".into(),
            model: "gpt-4.1".into(),
            display_name: Some("GPT-4.1".into()),
            context_window: 1_047_576,
            max_output_tokens: Some(32_768),
            enabled: true,
            supports_tools: Some(true),
            supports_responses_api: Some(true),
            supports_vision: Some(true),
        };
        let opt = m.to_letta_model_option();
        assert_eq!(opt.handle, "gpt-4.1");
        assert!(opt.label.contains("1M ctx"));
        assert_eq!(opt.context_window, Some(1_047_576));
    }
}
