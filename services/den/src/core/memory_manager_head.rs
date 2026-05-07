//! Read-only repository files for an agent’s git memfs repo via MemFS Manager
//! (`GET .../v1/management/agents/{agent_id}/files`).

use std::time::Duration;

use crate::errors::CustomError;

const DEFAULT_ORG: &str = "org-default";

/// Raw file/directory node returned by MemFS Manager.
///
/// This model is intentionally permissive so Den can tolerate small response-shape
/// differences across MemFS Manager versions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryManagerRepoNode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub last_commit_date: Option<String>,
    #[serde(default)]
    pub last_commit_message: Option<String>,
    #[serde(default)]
    pub children: Vec<MemoryManagerRepoNode>,
}

/// Template-friendly node for rendering a hierarchical repository file list with
/// last commit metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrivateMemoryRepoNodeView {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub last_commit_date: Option<String>,
    pub last_commit_message: Option<String>,
    pub children: Vec<PrivateMemoryRepoNodeView>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrivateMemoryCommitRowView {
    pub path: String,
    pub last_commit_date: Option<String>,
    pub last_commit_message: Option<String>,
}

/// Template-friendly memory health summary returned by MemFS Manager's status endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryManagerStatusView {
    pub state: String,
    pub label: String,
    pub is_ok: bool,
    pub warning: Option<String>,
    pub commit_count: Option<i64>,
    pub commit_count_display: String,
    pub file_count: Option<i64>,
    pub file_count_display: String,
    pub memory_file_count: Option<i64>,
    pub memory_file_count_display: String,
    pub head_commit: Option<String>,
    pub head_date: Option<String>,
    pub head_message: Option<String>,
    pub recent_activity_count: usize,
    pub recent_activity: Vec<MemoryManagerActivityView>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryManagerActivityView {
    pub event: String,
    pub state: Option<String>,
    pub status: Option<i64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MemoryManagerStatusResponse {
    #[serde(default)]
    state: String,
    #[serde(default)]
    warning: Option<String>,
    #[serde(default)]
    commit_count: Option<i64>,
    #[serde(default)]
    file_count: Option<i64>,
    #[serde(default)]
    memory_file_count: Option<i64>,
    #[serde(default)]
    head: Option<MemoryManagerHeadResponse>,
    #[serde(default)]
    recent_activity: Vec<MemoryManagerActivityResponse>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MemoryManagerHeadResponse {
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MemoryManagerActivityResponse {
    #[serde(default)]
    event: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    status: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemfsViewHealth {
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub view_tip: Option<String>,
    #[serde(default)]
    pub quarantined: bool,
    #[serde(default)]
    pub diagnostic: Option<String>,
    #[serde(default)]
    pub canonical_repo: Option<String>,
    #[serde(default)]
    pub view_repo: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MemfsViewRegisterResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    view: Option<MemfsViewHealth>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemfsWriteRoleMemoryEntryRequest {
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_selection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemfsWriteRoleMemoryEntryResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub entry_id: Option<String>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub view: Option<MemfsViewHealth>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemfsRoleMemoryStatusResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub entry_count_by_kind: serde_json::Value,
    #[serde(default)]
    pub registered_view_count: usize,
    #[serde(default)]
    pub recent_activity: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemfsRoleMemoryTreeResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub files: serde_json::Value,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub total_file_count: usize,
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemfsRoleMemoryFileResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub size_bytes: usize,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemfsRoleMemorySearchResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bear_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub canonical_tip: Option<String>,
    #[serde(default)]
    pub results: serde_json::Value,
    #[serde(default)]
    pub result_count: usize,
    #[serde(default)]
    pub scanned_file_count: usize,
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub error: Option<String>,
}

impl From<MemoryManagerStatusResponse> for MemoryManagerStatusView {
    fn from(status: MemoryManagerStatusResponse) -> Self {
        let state = if status.state.trim().is_empty() {
            "unknown".to_string()
        } else {
            status.state
        };
        let label = match state.as_str() {
            "ok" => "Healthy",
            "missing_repo" => "Missing repository",
            "seed_only" => "Seed only",
            "no_memory_files" => "No memory files",
            "git_error" => "Git error",
            _ => "Unknown",
        }
        .to_string();
        let head = status.head;
        let commit_count_display = status
            .commit_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let file_count_display = status
            .file_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let memory_file_count_display = status
            .memory_file_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let recent_activity = status
            .recent_activity
            .into_iter()
            .map(|event| MemoryManagerActivityView {
                event: event.event,
                state: event.state,
                status: event.status,
            })
            .collect::<Vec<_>>();

        Self {
            is_ok: state == "ok",
            state,
            label,
            warning: status.warning,
            commit_count: status.commit_count,
            commit_count_display,
            file_count: status.file_count,
            file_count_display,
            memory_file_count: status.memory_file_count,
            memory_file_count_display,
            head_commit: head.as_ref().and_then(|h| h.commit.clone()),
            head_date: head.as_ref().and_then(|h| h.date.clone()),
            head_message: head.and_then(|h| h.message),
            recent_activity_count: recent_activity.len(),
            recent_activity,
        }
    }
}

impl From<MemoryManagerRepoNode> for PrivateMemoryRepoNodeView {
    fn from(node: MemoryManagerRepoNode) -> Self {
        let is_dir = matches!(
            node.node_type.trim().to_ascii_lowercase().as_str(),
            "dir" | "directory" | "folder" | "tree"
        ) || !node.children.is_empty();

        let name = if node.name.trim().is_empty() {
            node.path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            node.name
        };

        Self {
            name,
            path: node.path,
            is_dir,
            last_commit_date: node.last_commit_date,
            last_commit_message: node.last_commit_message,
            children: node
                .children
                .into_iter()
                .map(PrivateMemoryRepoNodeView::from)
                .collect(),
        }
    }
}

pub fn private_memory_commit_rows(
    nodes: &[PrivateMemoryRepoNodeView],
) -> Vec<PrivateMemoryCommitRowView> {
    fn visit(node: &PrivateMemoryRepoNodeView, rows: &mut Vec<PrivateMemoryCommitRowView>) {
        if node.is_dir {
            for child in &node.children {
                visit(child, rows);
            }
            return;
        }
        rows.push(PrivateMemoryCommitRowView {
            path: node.path.clone(),
            last_commit_date: node.last_commit_date.clone(),
            last_commit_message: node.last_commit_message.clone(),
        });
    }

    let mut rows = Vec::new();
    for node in nodes {
        visit(node, &mut rows);
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

/// Accepted response shapes from MemFS Manager.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum MemoryManagerFilesResponse {
    Bare(Vec<MemoryManagerRepoNode>),
    WrappedFiles { files: Vec<MemoryManagerRepoNode> },
    WrappedTree { tree: Vec<MemoryManagerRepoNode> },
    WrappedRoot { root: Vec<MemoryManagerRepoNode> },
}

impl MemoryManagerFilesResponse {
    fn into_nodes(self) -> Vec<MemoryManagerRepoNode> {
        match self {
            Self::Bare(v) => v,
            Self::WrappedFiles { files } => files,
            Self::WrappedTree { tree } => tree,
            Self::WrappedRoot { root } => root,
        }
    }
}

pub async fn register_memfs_role_view(
    http: &reqwest::Client,
    base_url: &str,
    agent_id: &str,
    bear_id: uuid::Uuid,
    role: &str,
) -> Result<Option<MemfsViewHealth>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() || agent_id.trim().is_empty() {
        return Ok(None);
    }
    let url = format!("{}/v1/management/views/register", base);
    let body = serde_json::json!({
        "agent_id": agent_id.trim(),
        "bear_id": bear_id.to_string(),
        "role": role,
        "org_id": DEFAULT_ORG,
    });
    let resp = http
        .post(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS view registration request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "MemFS view registration HTTP {status}: {text}"
        )));
    }
    let payload: MemfsViewRegisterResponse = serde_json::from_str(&text).map_err(|e| {
        CustomError::Parsing(format!("MemFS view registration JSON: {e}; body: {text}"))
    })?;
    if !payload.ok {
        return Err(CustomError::System(format!(
            "MemFS view registration failed: {}",
            payload.error.unwrap_or_else(|| "unknown error".to_string())
        )));
    }
    Ok(payload.view)
}

pub async fn write_memfs_role_memory_entry(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
    request: &MemfsWriteRoleMemoryEntryRequest,
) -> Result<Option<MemfsWriteRoleMemoryEntryResponse>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{}/v1/management/bears/{}/roles/{}/memory-entries",
        base,
        bear_id,
        urlencoding::encode(role)
    );
    let resp = http
        .post(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .json(request)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS role memory entry write request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "MemFS role memory entry write HTTP {status}: {text}"
        )));
    }
    let payload: MemfsWriteRoleMemoryEntryResponse = serde_json::from_str(&text).map_err(|e| {
        CustomError::Parsing(format!("MemFS role memory entry write JSON: {e}; body: {text}"))
    })?;
    if !payload.ok {
        return Err(CustomError::System(format!(
            "MemFS role memory entry write failed: {}",
            payload
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        )));
    }
    Ok(Some(payload))
}

pub async fn fetch_memfs_role_memory_status(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
) -> Result<Option<MemfsRoleMemoryStatusResponse>, CustomError> {
    fetch_memfs_role_memory_json(
        http,
        base_url,
        bear_id,
        role,
        "memory-status",
        &[],
    )
    .await
}

pub async fn fetch_memfs_role_memory_tree(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
) -> Result<Option<MemfsRoleMemoryTreeResponse>, CustomError> {
    fetch_memfs_role_memory_json(http, base_url, bear_id, role, "memory-tree", &[]).await
}

pub async fn fetch_memfs_role_memory_file(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
    path: &str,
) -> Result<Option<MemfsRoleMemoryFileResponse>, CustomError> {
    fetch_memfs_role_memory_json(
        http,
        base_url,
        bear_id,
        role,
        "memory-files",
        &[("path", path)],
    )
    .await
}

pub async fn search_memfs_role_memory(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
    query: &str,
    limit: Option<usize>,
) -> Result<Option<MemfsRoleMemorySearchResponse>, CustomError> {
    let limit_string = limit.map(|n| n.to_string());
    let mut params = vec![("query", query)];
    if let Some(limit_ref) = limit_string.as_deref() {
        params.push(("limit", limit_ref));
    }
    fetch_memfs_role_memory_json(
        http,
        base_url,
        bear_id,
        role,
        "memory-search",
        &params,
    )
    .await
}

async fn fetch_memfs_role_memory_json<T>(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
    endpoint: &str,
    query: &[(&str, &str)],
) -> Result<Option<T>, CustomError>
where
    T: serde::de::DeserializeOwned,
{
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{}/v1/management/bears/{}/roles/{}/{}",
        base,
        bear_id,
        urlencoding::encode(role),
        endpoint
    );
    let resp = http
        .get(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .query(query)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS role memory {endpoint} request failed: {e}")))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "MemFS role memory {endpoint} HTTP {status}: {text}"
        )));
    }
    let payload: T = serde_json::from_str(&text).map_err(|e| {
        CustomError::Parsing(format!("MemFS role memory {endpoint} JSON: {e}; body: {text}"))
    })?;
    Ok(Some(payload))
}

pub async fn fetch_memfs_role_view_health(
    http: &reqwest::Client,
    base_url: &str,
    bear_id: uuid::Uuid,
    role: &str,
) -> Result<Option<MemfsViewHealth>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{}/v1/management/bears/{}/roles/{}",
        base,
        bear_id,
        urlencoding::encode(role)
    );
    let resp = http
        .get(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS view health request failed: {e}")))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CustomError::System(format!(
            "MemFS view health HTTP {status}: {text}"
        )));
    }
    let payload: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| CustomError::Parsing(format!("MemFS view health JSON: {e}; body: {text}")))?;
    let first = payload
        .get("views")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .cloned();
    match first {
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| CustomError::Parsing(format!("MemFS view health row JSON: {e}"))),
        None => Ok(None),
    }
}

/// Fetches repository files metadata for an agent.
///
/// Returns:
/// - `Ok(None)` if MemFS Manager URL is empty, agent id is empty, or repo does not exist (HTTP 404)
/// - `Ok(Some(...))` with hierarchical nodes otherwise
pub async fn fetch_memory_manager_repository_files(
    http: &reqwest::Client,
    base_url: &str,
    agent_id: &str,
) -> Result<Option<Vec<PrivateMemoryRepoNodeView>>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }

    let aid = agent_id.trim();
    if aid.is_empty() {
        return Ok(None);
    }

    let url = format!(
        "{}/v1/management/agents/{}/files",
        base,
        urlencoding::encode(aid)
    );

    let resp = http
        .get(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS Manager (git) request failed: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_else(|_| String::new());
        return Err(CustomError::System(format!(
            "MemFS Manager (git) HTTP {status}: {text}"
        )));
    }

    let payload: MemoryManagerFilesResponse = resp
        .json()
        .await
        .map_err(|e| CustomError::Parsing(format!("MemFS Manager JSON: {e}")))?;

    let nodes = payload
        .into_nodes()
        .into_iter()
        .map(PrivateMemoryRepoNodeView::from)
        .collect::<Vec<_>>();

    Ok(Some(nodes))
}

/// Fetches MemFS Manager's self-reported sync health for an agent.
pub async fn fetch_memory_manager_repository_status(
    http: &reqwest::Client,
    base_url: &str,
    agent_id: &str,
) -> Result<Option<MemoryManagerStatusView>, CustomError> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }

    let aid = agent_id.trim();
    if aid.is_empty() {
        return Ok(None);
    }

    let url = format!(
        "{}/v1/management/agents/{}/status",
        base,
        urlencoding::encode(aid)
    );

    let resp = http
        .get(&url)
        .header("X-Organization-Id", DEFAULT_ORG)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CustomError::System(format!("MemFS Manager status request failed: {e}")))?;

    let status = resp.status();
    if !(status.is_success() || status == reqwest::StatusCode::NOT_FOUND) {
        let text = resp.text().await.unwrap_or_else(|_| String::new());
        return Err(CustomError::System(format!(
            "MemFS Manager status HTTP {status}: {text}"
        )));
    }

    let payload: MemoryManagerStatusResponse = resp
        .json()
        .await
        .map_err(|e| CustomError::Parsing(format!("MemFS Manager status JSON: {e}")))?;

    Ok(Some(MemoryManagerStatusView::from(payload)))
}
