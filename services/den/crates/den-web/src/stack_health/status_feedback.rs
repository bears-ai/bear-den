//! Public status is a projection, never a credential or upstream-body dump.

use super::StatusPayload;
use crate::config::Config;

pub(super) fn sanitize(payload: &mut StatusPayload, config: &Config) {
    let mut secrets = vec![
        config.mailgun_api_key.clone(),
        config.den_secret_encryption_key.clone(),
        config.bifrost_admin_password.clone(),
        config.llm_api_key.clone(),
        config.s3_access_key_id.clone(),
        config.s3_secret_access_key.clone(),
        config.github_packages_token.clone(),
        config.brave_search_api_key.clone(),
        config.sandbox_service_token.clone(),
        config.sandbox_server_token.clone(),
    ];
    for name in ["JWT_SECRET", "OPENAI_API_KEY", "LLM_API_KEY"] {
        if let Ok(value) = std::env::var(name) {
            secrets.push(value);
        }
    }
    for source in [
        Some(config.database_url.as_str()),
        Some(config.bifrost_base_url.as_str()),
        Some(config.bifrost_management_url.as_str()),
        Some(config.llm_api_url.as_str()),
        config.qdrant_url.as_deref(),
        config.sandbox_server_url.as_deref(),
    ] {
        if let Some(url) = source.and_then(|source| url::Url::parse(source).ok()) {
            if let Some(password) = url.password() {
                secrets.push(password.to_string());
            }
            secrets.extend(url.query_pairs().map(|(_, value)| value.into_owned()));
        }
    }
    secrets.retain(|secret| !secret.is_empty());
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    let urls = regex::Regex::new(r#"(?i)[a-z][a-z0-9+.-]*://[^\s<>\"']+"#)
        .expect("status URL redaction pattern");
    let clean = |text: &mut String| {
        *text =
            urls.replace_all(text, |capture: &regex::Captures<'_>| match url::Url::parse(
                &capture[0],
            ) {
                Ok(mut url) => {
                    let _ = url.set_username("");
                    let _ = url.set_password(None);
                    url.set_query(None);
                    url.set_fragment(None);
                    url.to_string()
                }
                Err(_) => "[redacted URL]".to_string(),
            })
            .into_owned();
        for secret in &secrets {
            *text = text.replace(secret, "[redacted]");
        }
    };
    for check in &mut payload.health.checks {
        clean(&mut check.detail);
    }
    for error in [
        &mut payload.ghcr_error,
        &mut payload.model_registry.gateway_error,
    ]
    .into_iter()
    .flatten()
    {
        clean(error);
    }
    if let Some(package) = &mut payload.ghcr_den {
        for tag in &mut package.tags {
            clean(tag);
        }
    }
}
