//! Wire types shared by the provider API (`server`) and the Den-side client.
//!
//! Everything here is plain serde data — no behavior — mirroring the role
//! `bearwire-protocol` plays for the armature edge.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact DNS names a work-surface owner has approved for outbound HTTPS.
///
/// This is deliberately neither a URL nor a pattern: ports, IP literals,
/// wildcards, and trailing-dot aliases are rejected at the protocol boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AllowedOutboundHosts(Vec<String>);

impl AllowedOutboundHosts {
    pub fn new(hosts: Vec<String>) -> Result<Self, String> {
        let mut normalized = Vec::with_capacity(hosts.len());
        for host in hosts {
            let host = host.trim().to_ascii_lowercase();
            if !is_exact_hostname(&host) {
                return Err(format!(
                    "allowed outbound host must be an exact DNS hostname: {host:?}"
                ));
            }
            if !normalized.contains(&host) {
                normalized.push(host);
            }
        }
        Ok(Self(normalized))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for AllowedOutboundHosts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::<String>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

fn is_exact_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.ends_with('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Execution boundary type. Only [`SandboxType::Container`] is implemented;
/// the rest exist so requests name their intent explicitly and get an explicit
/// "unimplemented" error instead of a silently different boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxType {
    Container,
    LocalWorkspaceReadonly,
    LocalWorkspaceWritable,
    EphemeralCopy,
    RemoteEphemeral,
}

impl SandboxType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::LocalWorkspaceReadonly => "local_workspace_readonly",
            Self::LocalWorkspaceWritable => "local_workspace_writable",
            Self::EphemeralCopy => "ephemeral_copy",
            Self::RemoteEphemeral => "remote_ephemeral",
        }
    }
}

/// Network posture of a sandbox. `Restricted` (the default) attaches the
/// container to an internal network whose only way out is a TCP relay to the
/// Den callback endpoint — task code cannot reach anything else. `Open` is
/// the pre-v1 behavior: default bridge, unrestricted egress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Restricted,
    Open,
}

impl NetworkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restricted => "restricted",
            Self::Open => "open",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleState {
    Provisioning,
    Running,
    Exited,
    Failed,
    Destroyed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CleanupState {
    Pending,
    Done,
    Failed { reason: String },
}

/// Resource bounds for one sandbox. `timeout_secs` is always enforced (the
/// provider's reaper destroys overdue sandboxes); the rest apply only where the
/// backend supports them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxLimits {
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_log_bytes: Option<u64>,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            timeout_secs: 900,
            memory_mb: None,
            cpus: None,
            pids: None,
            max_log_bytes: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSandboxRequest {
    /// Name of a configured root on the sandbox host.
    pub root: String,
    /// Git ref to provision at (git-backed roots only). Defaults to the
    /// root's upstream default ref, or the pristine clone's HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub sandbox_type: SandboxType,
    /// Whether the work needs a writable workspace.
    #[serde(default = "default_true")]
    pub requires_write: bool,
    /// Catalog image **name** (not a raw reference) — resolved against the
    /// provider's image catalog. Falls back to the root's default image, the
    /// catalog default, then the provider's `SANDBOX_IMAGE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Network posture; defaults to [`NetworkMode::Restricted`].
    #[serde(default)]
    pub network: NetworkMode,
    /// An optional Den-verified run ceiling. `Some(empty)` prohibits outbound
    /// host relays; `None` preserves the legacy surface-only contract. The
    /// provider intersects this with its own saved root ceiling and never
    /// treats the request as an independent grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_outbound_hosts: Option<AllowedOutboundHosts>,
    /// Environment injected into the sandbox (armature credentials, work
    /// order id, ...). Values are never logged by the provider.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub limits: SandboxLimits,
    /// Opaque caller labels (e.g. `work_run_id`), stamped onto the container
    /// so orphans can be reconciled after a provider restart.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Provider-owned volume with prepared Cargo registry artifacts. The
    /// sandbox receives it read-only at `/den/cargo-home`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_home_volume: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareRustDependenciesRequest {
    /// Checkout-relative path to the selected Cargo manifest.
    pub manifest_path: String,
    /// Cargo package name, validated by Den before it reaches the provider.
    pub package: String,
    pub resolution: RustDependencyResolution,
    pub preparation: RustDependencyPreparation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RustDependencyResolution {
    Locked,
    UpdateLockfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RustDependencyPreparation {
    Fetch,
    Check,
    TestNoRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareRustDependenciesResponse {
    pub status: String,
    pub code: String,
    pub stage: String,
    pub retryable: bool,
    pub content: String,
    pub lockfile_changed: bool,
}

/// Facts recognized about the workspace at provision time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSurface {
    pub is_git: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub dirty: bool,
    pub untracked_present: bool,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,
    /// Workspace-relative Cargo manifests recognized as Rust roots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cargo_manifest_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_manager_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lockfiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_command_hints: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxUsage {
    pub duration_ms: u64,
    /// Approximate log bytes observed (from the most recent full log read).
    pub log_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_killed: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SandboxDescriptor {
    pub id: String,
    pub state: SandboxLifecycleState,
    pub sandbox_type: SandboxType,
    /// Honest human-readable statement of the isolation actually provided.
    pub strength_label: String,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub work_surface: WorkSurface,
    /// Trusted provisioning performed before the sandbox process started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_dependency_preparation: Option<PrepareRustDependenciesResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub usage: SandboxUsage,
    pub cleanup: CleanupState,
    /// Network posture the sandbox actually runs with.
    #[serde(default)]
    pub network: NetworkMode,
    pub labels: BTreeMap<String, String>,
    /// RFC 3339 timestamps.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogsResponse {
    /// UTF-8 (lossy) tail of the sandbox's combined stdout/stderr.
    pub content: String,
    pub truncated: bool,
    pub tail_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// Porcelain status code, e.g. `M`, `A`, `D`, `??`.
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResponse {
    pub changed_files: Vec<ChangedFile>,
    /// Unified diff of tracked changes vs the provisioned ref. Contents of
    /// untracked files are not included (they are listed in `changed_files`).
    pub patch: String,
    pub patch_truncated: bool,
}

/// Publish (push) a sandbox workspace's commits to the root's upstream.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishRequest {
    /// Target branch on the upstream (`refs/heads/<branch>`).
    pub branch: String,
    /// Commit any uncommitted workspace changes before pushing so nothing is
    /// silently dropped.
    #[serde(default = "default_true")]
    pub auto_commit_leftovers: bool,
    /// The root's default ref is refused as a target unless this is set —
    /// pushing to `main` must always be an explicit choice.
    #[serde(default)]
    pub allow_default_ref: bool,
    /// Git author/committer name for provider-created leftover commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    /// Caller label used in the auto-commit message (e.g. the work run id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishResponse {
    pub branch: String,
    /// Workspace HEAD after any auto-commit.
    pub commit: String,
    /// Commits ahead of the provisioned base that the push carried.
    pub commits_pushed: u64,
    /// Whether leftover changes were auto-committed before the push.
    pub auto_committed: bool,
    /// False when there was nothing beyond the provisioned base to push (the
    /// push is skipped rather than creating an empty branch move).
    pub pushed: bool,
}

/// One selectable image in the provider's catalog (from the roots file).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogImage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogRoot {
    pub name: String,
    pub has_upstream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_ref: Option<String>,
    /// Catalog image name this root defaults to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_image: Option<String>,
}

/// Selectable roots and images, for dispatch UIs and model tools. Image
/// references themselves stay on the sandbox host; only names travel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogResponse {
    pub images: Vec<CatalogImage>,
    pub roots: Vec<CatalogRoot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootStatus {
    pub name: String,
    pub has_upstream: bool,
    /// Whether the pristine clone / source path currently looks usable.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootCommitFileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootInspectionResponse {
    pub name: String,
    pub default_ref: String,
    pub head: String,
    pub short_head: String,
    pub subject: String,
    pub files: Vec<RootCommitFileChange>,
    pub additions: u64,
    pub deletions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_head: Option<String>,
    /// `in_sync`, `remote_differs`, or `remote_unavailable`.
    pub origin_status: String,
}

/// Read-only comparison of two refs in the provider's pristine bare clone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootComparisonResponse {
    pub base_ref: String,
    pub head_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    pub patch: String,
    pub patch_truncated: bool,
    /// Pristine roots are bare clones, therefore never have a dirty worktree.
    pub worktree_clean: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncRootResponse {
    pub synced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::RootComparisonResponse;

    #[test]
    fn root_comparison_keeps_refs_commits_and_cleanliness() {
        let comparison = RootComparisonResponse {
            base_ref: "main".into(),
            head_ref: "den/job-1".into(),
            base_commit: Some("a".repeat(40)),
            head_commit: Some("b".repeat(40)),
            patch: "diff --git a/a b/a\n".into(),
            patch_truncated: false,
            worktree_clean: true,
        };
        let value = serde_json::to_value(comparison).unwrap();
        assert_eq!(value["base_ref"], "main");
        assert_eq!(value["head_ref"], "den/job-1");
        assert_eq!(value["worktree_clean"], true);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub backend_available: bool,
    pub active_sandboxes: usize,
    pub roots: Vec<RootStatus>,
    /// Missing on old providers: Den must not send a hat-bound run ceiling
    /// to a provider that would silently ignore the request field.
    #[serde(default)]
    pub run_outbound_ceiling_supported: bool,
    /// Per-connection Den authorization for hat-bound external HTTPS relays.
    /// Old providers deserialize as false and must not receive hat-bound Work.
    #[serde(default)]
    pub dynamic_egress_supported: bool,
}

/// Uniform error body for non-2xx provider responses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Stable machine-readable kind, e.g. `unimplemented_type`, `queue_full`.
    pub kind: String,
    pub error: String,
}

/// Den-managed provider configuration, pushed declaratively (full set) to
/// `PUT /sandbox/v1/managed-config`. Den's database is the source of truth;
/// the provider persists a copy on its volume so provisioning works between
/// pushes and across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedConfig {
    pub surfaces: Vec<ManagedSurface>,
    pub images: Vec<ManagedImage>,
    /// Opaque Den-computed version (content hash) echoed back by
    /// [`ManagedConfigStatus`] so reconciles can short-circuit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One managed work surface: a git upstream the provider serves as a root.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSurface {
    /// Root name (path-safe slug; the provider re-validates).
    pub name: String,
    pub upstream_url: String,
    pub default_ref: String,
    /// Catalog image name this surface's sandboxes default to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_image: Option<String>,
    /// Owner-approved destinations. Empty deliberately means no egress.
    #[serde(default, skip_serializing_if = "AllowedOutboundHosts::is_empty")]
    pub allowed_outbound_hosts: AllowedOutboundHosts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<ManagedCredential>,
    /// Optional GitHub App installation. The app identity and private key are
    /// provider deployment secrets, never part of this managed payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_app: Option<ManagedGitHubApp>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedGitHubApp {
    pub installation_id: i64,
    /// Authorizes publishing branches. False retains anonymous/read-only
    /// behavior even when the App is installed for the repository.
    #[serde(default)]
    pub write_enabled: bool,
}

/// Credential material for a managed surface's upstream. Values are written
/// to 0600 files on the provider and never logged — `Debug` is redacted.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedCredential {
    SshKey { private_key: String },
    HttpsToken { token: String },
}

impl std::fmt::Debug for ManagedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SshKey { .. } => f.write_str("ManagedCredential::SshKey(<redacted>)"),
            Self::HttpsToken { .. } => f.write_str("ManagedCredential::HttpsToken(<redacted>)"),
        }
    }
}

/// One managed catalog image: name -> container image reference.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedImage {
    pub name: String,
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
}

#[cfg(test)]
mod managed_config_tests {
    use super::*;

    #[test]
    fn managed_config_rejects_unknown_json_fields() {
        let config_with_typo = serde_json::json!({
            "surfaces": [{
                "name": "site",
                "upstream_url": "https://example.invalid/repo.git",
                "default_ref": "main",
                "default_image": "base",
                "credential": {
                    "kind": "https_token",
                    "token": "sekrit"
                },
                "upstream_urrl": "typo should not be ignored"
            }],
            "images": [{
                "name": "base",
                "image": "bears/sandbox:latest",
                "default": true
            }],
            "version": "v-test"
        });

        let err = serde_json::from_value::<ManagedConfig>(config_with_typo).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected serde error: {err}"
        );
    }

    #[test]
    fn allowed_outbound_hosts_normalizes_and_rejects_non_hostnames() {
        let hosts = AllowedOutboundHosts::new(vec![
            " INDEX.CRATES.IO ".to_string(),
            "index.crates.io".to_string(),
            "static.crates.io".to_string(),
        ])
        .expect("valid hosts");
        assert_eq!(hosts.as_slice(), ["index.crates.io", "static.crates.io"]);

        for invalid in [
            "https://example.com",
            "*.example.com",
            "127.0.0.1",
            "example.com:443",
        ] {
            assert!(
                AllowedOutboundHosts::new(vec![invalid.to_string()]).is_err(),
                "{invalid}"
            );
        }
    }
}

/// Non-secret summary of the provider's applied managed config.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagedConfigStatus {
    pub surfaces: usize,
    pub images: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One image present in the engine's local store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineImage {
    pub repository: String,
    pub tag: String,
    pub id: String,
    pub size: String,
    pub created: String,
    /// Whether any catalog entry references this image.
    pub in_catalog: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageStoreResponse {
    pub images: Vec<EngineImage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullImageRequest {
    /// Registry image reference, e.g. `ghcr.io/owner/bears-sandbox:latest`.
    pub reference: String,
}

/// The only things the provider will build: the Dockerfile variants shipped
/// in this repository. There is deliberately no free-form build input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildVariant {
    Base,
    Rust,
    Node,
    Godot,
}

impl BuildVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Godot => "godot",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildImageRequest {
    pub variant: BuildVariant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveImageRequest {
    pub reference: String,
}

/// Returned by long-running image operations (pull, build); poll
/// `GET /sandbox/v1/operations/{id}` for progress.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationAccepted {
    pub operation_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Running,
    Succeeded,
    Failed,
}

/// A background provider operation. Operations live in provider memory only
/// and do not survive restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationDescriptor {
    pub id: String,
    /// `pull` or `build`.
    pub kind: String,
    /// Image reference or build tag the operation targets.
    pub target: String,
    pub state: OperationState,
    /// UTF-8 (lossy) tail of the operation's combined output.
    pub log_tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// RFC 3339 timestamps.
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}
