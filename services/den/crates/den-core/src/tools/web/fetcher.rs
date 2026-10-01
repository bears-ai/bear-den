//! The `WebFetcher` capability seam and its data shapes.
//!
//! `den-tools` owns the web tool *orchestration* (argument parsing, approval
//! branching, audit construction, text extraction, result shaping). The actual
//! capabilities — Postgres-backed policy/audit, HTTP egress, and the search
//! provider — are inverted behind [`WebFetcher`], which `den-runtime` (the `den`
//! crate today) implements over `web_policy` + `reqwest` + `Config`.
//!
//! See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md` (Phase B).

use crate::{tools::context::DenToolInvocationContext, DenError};
use uuid::Uuid;

/// Per-bear policy outcome for a candidate URL. Mirrors `web_policy`'s decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebApproval {
    Preferred,
    Allowed,
    ApprovedUrl,
    ApprovedHost,
    ApprovedOnce,
    Blocked,
    RequiresApproval,
}

impl WebApproval {
    /// Stable wire string recorded in the audit trail (`approval_kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preferred => "preferred",
            Self::Allowed => "allowed",
            Self::ApprovedUrl => "user_url",
            Self::ApprovedHost => "user_host",
            Self::ApprovedOnce => "approved_once",
            Self::Blocked => "denied",
            Self::RequiresApproval => "requires_approval",
        }
    }

    pub fn is_approved(self) -> bool {
        matches!(
            self,
            Self::Preferred
                | Self::Allowed
                | Self::ApprovedUrl
                | Self::ApprovedHost
                | Self::ApprovedOnce
        )
    }

    pub fn is_blocked(self) -> bool {
        matches!(self, Self::Blocked)
    }
}

/// A normalized URL plus its canonical host.
#[derive(Debug, Clone)]
pub struct WebUrl {
    pub url: String,
    pub host: String,
}

/// One row for the `bear_web_fetches` audit trail. Borrows so the orchestrator
/// can build it from locals just before an awaited write.
#[derive(Debug, Clone, Copy)]
pub struct WebFetchAudit<'a> {
    pub bear_id: Uuid,
    pub session_id: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
    pub url: &'a str,
    pub final_url: Option<&'a str>,
    pub host: &'a str,
    pub execution_location: &'a str,
    pub approval_kind: &'a str,
    pub http_status: Option<i32>,
    pub content_type: Option<&'a str>,
    pub bytes: Option<i64>,
}

/// Result of an HTTP GET performed by the runtime.
///
/// `body` is already capped to the runtime's max-bytes budget; `total_bytes` is
/// the full downloaded length (used for the audit), and `body_truncated` reports
/// whether the cap was hit.
#[derive(Debug, Clone)]
pub struct WebHttpResponse {
    pub final_url: String,
    pub final_host: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub total_bytes: usize,
    pub body_truncated: bool,
}

/// Capabilities the web tools need from the runtime. Methods return
/// [`DenError`]; the `den` web boundary maps that to `CustomError`.
///
/// `Send + Sync` so the generic orchestration futures are `Send` (the dispatcher
/// runs on the tokio multi-thread runtime).
// Native async fn in trait: workspace-internal, consumed via generic bounds /
// concrete impls only (never `dyn`), so Send flows through monomorphization.
#[allow(async_fn_in_trait)]
pub trait WebFetcher: Send + Sync {
    /// Normalize `raw_url`, recheck Bear policy, and consume only an exact
    /// Den-bound one-shot approval for this invocation when needed. Context
    /// fields are data, not grants; the runtime verifies the persisted approval.
    async fn decide_fetch_approval(
        &self,
        context: &DenToolInvocationContext,
        raw_url: &str,
    ) -> Result<(WebUrl, WebApproval), DenError>;

    /// Append a row to the fetch audit trail.
    async fn record_fetch_attempt(&self, audit: WebFetchAudit<'_>) -> Result<(), DenError>;

    /// Perform a bounded, SSRF-validated HTTP GET (validates the initial and the
    /// post-redirect final URL) and return the capped body + metadata.
    async fn http_get(&self, url: &str) -> Result<WebHttpResponse, DenError>;

    /// Hosts the bear has marked `preferred`, highest priority first.
    async fn preferred_hosts(&self, bear_id: Uuid) -> Result<Vec<String>, DenError>;

    /// Canonical host for a search-result URL, or `None` if it cannot be parsed.
    fn normalize_host(&self, url: &str) -> Option<String>;

    /// Configured default for `max_results` when the caller omits it.
    fn default_search_max_results(&self) -> usize;

    /// Run the configured external search provider and return its raw result
    /// JSON (shape: `{ "results": [ { "url", "title", "snippet", ... } ], ... }`).
    async fn provider_search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<serde_json::Value, DenError>;
}
