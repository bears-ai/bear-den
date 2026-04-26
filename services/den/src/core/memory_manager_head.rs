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
