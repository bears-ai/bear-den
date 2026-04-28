//! Read-only repository files for an agent’s git memfs repo via Memory Manager
//! (`GET .../v1/management/agents/{agent_id}/files`).

use std::time::Duration;

use crate::errors::CustomError;

const DEFAULT_ORG: &str = "org-default";

/// Raw file/directory node returned by Memory Manager.
///
/// This model is intentionally permissive so Den can tolerate small response-shape
/// differences across Memory Manager versions.
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

/// Template-friendly memory health summary returned by Memory Manager's status endpoint.
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

/// Accepted response shapes from Memory Manager.
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

/// Fetches repository files metadata for an agent.
///
/// Returns:
/// - `Ok(None)` if Memory Manager URL is empty, agent id is empty, or repo does not exist (HTTP 404)
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
        .map_err(|e| CustomError::System(format!("Memory Manager (git) request failed: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_else(|_| String::new());
        return Err(CustomError::System(format!(
            "Memory Manager (git) HTTP {status}: {text}"
        )));
    }

    let payload: MemoryManagerFilesResponse = resp
        .json()
        .await
        .map_err(|e| CustomError::Parsing(format!("Memory Manager JSON: {e}")))?;

    let nodes = payload
        .into_nodes()
        .into_iter()
        .map(PrivateMemoryRepoNodeView::from)
        .collect::<Vec<_>>();

    Ok(Some(nodes))
}

/// Fetches Memory Manager's self-reported sync health for an agent.
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
        .map_err(|e| CustomError::System(format!("Memory Manager status request failed: {e}")))?;

    let status = resp.status();
    if !(status.is_success() || status == reqwest::StatusCode::NOT_FOUND) {
        let text = resp.text().await.unwrap_or_else(|_| String::new());
        return Err(CustomError::System(format!(
            "Memory Manager status HTTP {status}: {text}"
        )));
    }

    let payload: MemoryManagerStatusResponse = resp
        .json()
        .await
        .map_err(|e| CustomError::Parsing(format!("Memory Manager status JSON: {e}")))?;

    Ok(Some(MemoryManagerStatusView::from(payload)))
}
