//! Web tools (`web_fetch`, `web_search`) — orchestration layer.
//!
//! The logic here is runtime-agnostic: it depends only on the [`WebFetcher`]
//! capability seam and the pure [`text`] helpers. The `den` crate provides the
//! concrete `WebFetcher` (Postgres policy/audit + reqwest + search provider) and
//! a thin wrapper that maps [`DenError`] to its web-boundary error.

pub mod fetcher;
pub mod text;

pub use fetcher::{WebApproval, WebFetchAudit, WebFetcher, WebHttpResponse, WebUrl};

use crate::{tools::context::DenToolInvocationContext, DenError};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use text::{html_to_text_excerpt, truncate_chars};

/// Full downloaded bytes are capped to this before decoding.
const MAX_FETCH_BYTES: usize = 1_000_000;

/// Fixed supported search destination; used by both advertisement and the
/// effect-time client so the hat host grant names the actual provider.
pub const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Debug, Deserialize)]
struct WebFetchArguments {
    url: String,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WebSearchArguments {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

async fn record_unexecuted_fetch_attempt(
    ctx: &impl WebFetcher,
    bear_id: Uuid,
    session_id: &str,
    web_url: &WebUrl,
    decision: &WebApproval,
) -> Result<(), DenError> {
    ctx.record_fetch_attempt(WebFetchAudit {
        bear_id,
        session_id: Some(session_id),
        tool_call_id: None,
        url: &web_url.url,
        final_url: None,
        host: &web_url.host,
        execution_location: "den",
        approval_kind: decision.as_str(),
        http_status: None,
        content_type: None,
        bytes: None,
    })
    .await
}

/// `web_fetch`: policy-gated, SSRF-validated, bounded page fetch.
pub async fn web_fetch(
    ctx: &impl WebFetcher,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, DenError> {
    let bear_id = context.bear_id;
    let session_id = context.session_id.as_str();
    let args: WebFetchArguments = serde_json::from_value(arguments)?;
    let max_chars = args.max_chars.unwrap_or(8_000).clamp(1, 20_000);

    let (web_url, decision) = ctx.decide_fetch_approval(context, &args.url).await?;

    if decision.is_blocked() {
        record_unexecuted_fetch_attempt(ctx, bear_id, session_id, &web_url, &decision).await?;
        return Err(DenError::Authorization(format!(
            "web_fetch host or URL is blocked by bear policy: {}",
            web_url.host
        )));
    }

    if !decision.is_approved() {
        record_unexecuted_fetch_attempt(ctx, bear_id, session_id, &web_url, &decision).await?;
        return Err(DenError::Authorization(format!(
            "web_fetch requires approval for host {}",
            web_url.host
        )));
    }

    let response = ctx.http_get(&web_url.url).await?;

    let raw = String::from_utf8_lossy(&response.body).into_owned();
    let text = if response.content_type.to_ascii_lowercase().contains("html") {
        html_to_text_excerpt(&raw)
    } else {
        raw
    };
    let (text_excerpt, char_truncated) = truncate_chars(&text, max_chars);

    ctx.record_fetch_attempt(WebFetchAudit {
        bear_id,
        session_id: Some(session_id),
        tool_call_id: None,
        url: &web_url.url,
        final_url: Some(&response.final_url),
        host: &response.final_host,
        execution_location: "den",
        approval_kind: decision.as_str(),
        http_status: Some(i32::from(response.status)),
        content_type: Some(&response.content_type),
        bytes: Some(response.total_bytes as i64),
    })
    .await?;

    Ok(json!({
        "url": response.final_url,
        "host": response.final_host,
        "approval": decision.as_str(),
        "status": response.status,
        "content_type": response.content_type,
        "text_excerpt": text_excerpt,
        "truncated": response.body_truncated || char_truncated,
    }))
}

/// `web_search`: policy-gated provider search with Bear-preferred-host re-ranking.
/// The full trusted invocation context is needed before disclosing a query to
/// an external provider; a model-supplied query is never an authorization.
pub async fn web_search(
    ctx: &impl WebFetcher,
    context: &DenToolInvocationContext,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: WebSearchArguments = serde_json::from_value(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        return Err(DenError::ValidationError(
            "query must not be empty".to_string(),
        ));
    }
    let max_results = args
        .max_results
        .unwrap_or_else(|| ctx.default_search_max_results())
        .clamp(1, 10);

    ctx.authorize_search(context).await?;
    let mut value = ctx.provider_search(query, max_results).await?;

    let preferred_hosts = ctx.preferred_hosts(context.bear_id).await?;

    if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
        for result in results.iter_mut() {
            if let Some(url) = result.get("url").and_then(Value::as_str) {
                if let Some(host) = ctx.normalize_host(url) {
                    let preferred = preferred_hosts.iter().any(|h| h == &host);
                    result["host"] = json!(host);
                    result["preferred_source"] = json!(preferred);
                }
            }
        }
        results.sort_by_key(|item| {
            !item
                .get("preferred_source")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    }
    value["preferred_hosts"] = json!(preferred_hosts);
    value["instruction"] = json!("Prefer results with preferred_source=true when they are relevant; otherwise use ordinary relevance judgment.");
    Ok(value)
}

/// Re-exported for the runtime impl, which caps downloaded bytes to this.
pub const fn max_fetch_bytes() -> usize {
    MAX_FETCH_BYTES
}
