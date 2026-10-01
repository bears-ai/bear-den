//! `den-runtime` implementation of the `den-tools` [`WebFetcher`] capability seam.
//!
//! Delegates to `web_policy` (Postgres-backed approval + audit + preferred
//! hosts), `reqwest` (SSRF-validated HTTP egress), and the configured search
//! provider. Errors from the existing `CustomError`-returning `core::*` functions
//! are mapped to `DenError` via [`CustomError::into_den`] at this boundary.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::{
    context::DenToolInvocationContext,
    web::{max_fetch_bytes, WebApproval, WebFetchAudit, WebFetcher, WebHttpResponse, WebUrl},
};
use den_core::DenError;

use crate::{
    config::Config,
    core::{tools::support::validate_public_http_url, web_policy},
    errors::CustomError,
};

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;

pub(crate) struct DenWebFetcher<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) config: &'a Config,
}

const fn map_decision(decision: web_policy::WebApprovalDecision) -> WebApproval {
    match decision {
        web_policy::WebApprovalDecision::Preferred => WebApproval::Preferred,
        web_policy::WebApprovalDecision::Allowed => WebApproval::Allowed,
        web_policy::WebApprovalDecision::ApprovedUrl => WebApproval::ApprovedUrl,
        web_policy::WebApprovalDecision::ApprovedHost => WebApproval::ApprovedHost,
        web_policy::WebApprovalDecision::Blocked => WebApproval::Blocked,
        web_policy::WebApprovalDecision::RequiresApproval => WebApproval::RequiresApproval,
    }
}

/// Atomically consume a Den-bound approval. A supplied request ID has no
/// authority without the matching approved row and exact original URL.
async fn consume_web_fetch_once(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    raw_url: &str,
) -> Result<bool, DenError> {
    let Some(request_id) = context
        .request_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return Ok(false);
    };
    let consumed = sqlx::query_scalar!(
        "UPDATE runtime_approvals SET consumed_at = now()
         WHERE execution_request_id = $1 AND bear_id = $2
           AND conversation_id = $3 AND client_session_id = $4
           AND arguments_json ->> 'url' = $5 AND status = 'approved'
           AND consumed_at IS NULL
         RETURNING approval_id",
        request_id,
        context.bear_id,
        context.conversation_id,
        context.session_id,
        raw_url,
    )
    .fetch_optional(pool)
    .await?;
    Ok(consumed.is_some())
}

impl WebFetcher for DenWebFetcher<'_> {
    async fn decide_fetch_approval(
        &self,
        context: &DenToolInvocationContext,
        raw_url: &str,
    ) -> Result<(WebUrl, WebApproval), DenError> {
        let (normalized, decision) =
            web_policy::decide_web_fetch_approval(self.pool, context.bear_id, raw_url)
                .await
                .map_err(CustomError::into_den)?;
        let decision = if decision == web_policy::WebApprovalDecision::RequiresApproval
            && consume_web_fetch_once(self.pool, context, raw_url).await?
        {
            WebApproval::ApprovedOnce
        } else {
            map_decision(decision)
        };
        Ok((
            WebUrl {
                url: normalized.url,
                host: normalized.host,
            },
            decision,
        ))
    }

    async fn record_fetch_attempt(&self, audit: WebFetchAudit<'_>) -> Result<(), DenError> {
        web_policy::record_web_fetch_attempt(
            self.pool,
            web_policy::WebFetchAuditParams {
                bear_id: audit.bear_id,
                session_id: audit.session_id,
                tool_call_id: audit.tool_call_id,
                url: audit.url,
                final_url: audit.final_url,
                host: audit.host,
                execution_location: audit.execution_location,
                approval_kind: audit.approval_kind,
                http_status: audit.http_status,
                content_type: audit.content_type,
                bytes: audit.bytes,
            },
        )
        .await
        .map_err(CustomError::into_den)
    }

    async fn http_get(&self, url: &str) -> Result<WebHttpResponse, DenError> {
        let parsed = validate_public_http_url(url)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(5))
            // An approval for one URL must not follow a redirect to a second
            // destination without another explicit authorization decision.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| DenError::System(format!("web fetch client build failed: {e}")))?;
        let resp = client
            .get(parsed.as_str())
            .header(reqwest::header::USER_AGENT, "BEARS Den web_fetch/0.1")
            .send()
            .await
            .map_err(|e| DenError::System(format!("web fetch request failed: {e}")))?;
        let final_url = resp.url().clone();
        validate_public_http_url(final_url.as_str())?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| DenError::System(format!("web fetch response read failed: {e}")))?;
        let max_bytes = max_fetch_bytes();
        let total_bytes = bytes.len();
        let body_truncated = total_bytes > max_bytes;
        let body = if body_truncated {
            bytes[..max_bytes].to_vec()
        } else {
            bytes.to_vec()
        };
        let final_normalized =
            web_policy::normalize_web_url(final_url.as_str()).map_err(CustomError::into_den)?;
        Ok(WebHttpResponse {
            final_url: final_url.to_string(),
            final_host: final_normalized.host,
            status: status.as_u16(),
            content_type,
            body,
            total_bytes,
            body_truncated,
        })
    }

    async fn preferred_hosts(&self, bear_id: Uuid) -> Result<Vec<String>, DenError> {
        web_policy::preferred_hosts_for_bear(self.pool, bear_id)
            .await
            .map_err(CustomError::into_den)
    }

    fn normalize_host(&self, url: &str) -> Option<String> {
        web_policy::normalize_web_url(url).ok().map(|n| n.host)
    }

    fn default_search_max_results(&self) -> usize {
        self.config.den_search_max_results
    }

    async fn provider_search(&self, query: &str, max_results: usize) -> Result<Value, DenError> {
        match self.config.den_search_provider.as_str() {
            "brave" => brave_web_search(self.config, query, max_results)
                .await
                .map_err(CustomError::into_den),
            "" => Err(DenError::System(format!(
                "den.web.search is registered but DEN_SEARCH_PROVIDER is not configured (query={}, max_results={max_results}). Set DEN_SEARCH_PROVIDER=brave and BRAVE_SEARCH_API_KEY.",
                Value::String(query.to_string())
            ))),
            other => Err(DenError::System(format!(
                "unsupported DEN_SEARCH_PROVIDER={other:?}; supported providers: brave"
            ))),
        }
    }
}

fn truncate_search_detail(s: String) -> String {
    const MAX: usize = 500;
    if s.len() <= MAX {
        s
    } else {
        format!("{}…", &s[..MAX.saturating_sub(1)])
    }
}

async fn brave_web_search(
    config: &Config,
    query: &str,
    max_results: usize,
) -> Result<Value, CustomError> {
    let key = config.brave_search_api_key.trim();
    if key.is_empty() {
        return Err(CustomError::System(
            "DEN_SEARCH_PROVIDER=brave requires BRAVE_SEARCH_API_KEY".to_string(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| CustomError::System(format!("Brave search client build failed: {e}")))?;
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&[("q", query), ("count", &max_results.to_string())])
        .send()
        .await
        .map_err(|e| CustomError::System(format!("Brave search request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "Brave search HTTP {status}: {}",
            truncate_search_detail(text)
        )));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|e| CustomError::Parsing(format!("Brave search JSON: {e}")))?;
    let results = payload
        .get("web")
        .and_then(|v| v.get("results"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .take(max_results)
        .map(|item| {
            serde_json::json!({
                "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "snippet": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                "source_domain": item.get("profile").and_then(|p| p.get("long_name")).and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "provider": "brave",
        "query": query,
        "max_results": max_results,
        "results": results,
        "note": "Search snippets are untrusted external content. Use web_fetch on selected URLs for bounded page content."
    }))
}
