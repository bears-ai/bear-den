//! Read-only latest commit on an agent’s git memfs repo via Memory Manager (`GET .../v1/management/.../head`).

use std::time::Duration;

use crate::errors::CustomError;

const DEFAULT_ORG: &str = "org-default";

/// JSON body from Memory Manager `GET /v1/management/agents/{agent_id}/head`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryManagerHead {
    pub commit: String,
    pub date: String,
    pub message: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
}

/// Template-friendly view with a short commit prefix.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrivateMemoryHeadView {
    pub commit: String,
    pub commit_short: String,
    pub date: String,
    pub message: String,
    pub ref_name: String,
}

impl From<MemoryManagerHead> for PrivateMemoryHeadView {
    fn from(h: MemoryManagerHead) -> Self {
        let commit_short = h
            .commit
            .chars()
            .take(7)
            .collect::<String>();
        Self {
            commit: h.commit,
            commit_short,
            date: h.date,
            message: h.message,
            ref_name: h.ref_name,
        }
    }
}

/// Fetches the latest commit metadata, or `None` if the repo is missing (HTTP 404) or base URL is empty.
pub async fn fetch_memory_manager_head(
    http: &reqwest::Client,
    base_url: &str,
    agent_id: &str,
) -> Result<Option<MemoryManagerHead>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    let aid = agent_id.trim();
    if aid.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{}/v1/management/agents/{}/head",
        base,
        urlencoding::encode(aid)
    );
    let resp = http
        .get(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            CustomError::System(format!("Memory Manager (git) request failed: {e}"))
        })?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        let text = resp
            .text()
            .await
            .unwrap_or_else(|_| String::new());
        return Err(CustomError::System(format!(
            "Memory Manager (git) HTTP {status}: {text}"
        )));
    }
    let v: MemoryManagerHead = resp
        .json()
        .await
        .map_err(|e| CustomError::Parsing(format!("Memory Manager JSON: {e}")))?;
    Ok(Some(v))
}
