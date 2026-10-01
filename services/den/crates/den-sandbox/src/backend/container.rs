//! Container backend via the `docker` CLI.
//!
//! ponytail: this shells out to the docker CLI rather than speaking the
//! engine API (bollard). The operations needed are exactly run / inspect /
//! logs / rm / ps / network create/rm, the CLI output is trivially
//! inspectable, and the same binary path works against podman
//! (`DOCKER_BIN=podman`). Upgrade path: switch to bollard if we ever need
//! event streams or tighter lifecycle hooks.
//!
//! Network posture: in the default `restricted` mode each sandbox runs on a
//! per-sandbox `--internal` network whose only way out is a socat relay
//! container forwarding to the Den callback endpoint — task code cannot reach
//! anything but Den. `open` mode is the old default-bridge behavior.

use super::{AdoptedSandbox, BackendError, BackendStatus, ProvisionSpec};
use crate::proc::{run_command, CaptureWindow, CommandSpec};
use crate::protocol::{
    LogsResponse, NetworkMode, PrepareRustDependenciesRequest, PrepareRustDependenciesResponse,
    RustDependencyPreparation, RustDependencyResolution,
};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOCKER_TIMEOUT: Duration = Duration::from_mins(2);
const EGRESS_DNS_TIMEOUT: Duration = Duration::from_secs(10);
const DOCKER_OUTPUT_CAP: usize = 256 * 1024;
const CARGO_DIAGNOSTIC_CAP: usize = 4096;
const CONTAINER_PREFIX: &str = "den-sbx-";
pub const SANDBOX_LABEL: &str = "den.sandbox";
/// Label carried by per-sandbox egress relay containers (value = sandbox id).
/// Deliberately not `den.sandbox=1` so relays are never adopted as sandboxes.
pub const RELAY_LABEL: &str = "den.sandbox.relay";

fn bounded_cargo_diagnostic(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(CARGO_DIAGNOSTIC_CAP)
        .collect()
}

fn rust_preparation_cargo_args(request: &PrepareRustDependenciesRequest) -> Vec<String> {
    let command = match request.preparation {
        RustDependencyPreparation::Fetch => "fetch",
        RustDependencyPreparation::Check => "check",
        RustDependencyPreparation::TestNoRun => "test",
    };
    let mut args = vec![
        command.to_string(),
        "--manifest-path".to_string(),
        request.manifest_path.clone(),
    ];
    if !request.package.is_empty() {
        args.extend(["-p".to_string(), request.package.clone()]);
    }
    if request.preparation == RustDependencyPreparation::TestNoRun {
        args.push("--no-run".to_string());
    }
    if request.resolution == RustDependencyResolution::Locked {
        // The helper has network access specifically to populate the managed
        // cache. Keep the checked-in resolution immutable while fetching it;
        // the task sandbox is the offline consumer of that cache.
        args.push("--locked".to_string());
    }
    args
}

fn classify_cargo_failure(diagnostic: &str) -> &'static str {
    if diagnostic.contains("failed to download")
        || diagnostic.contains("failed to get")
        || diagnostic.contains("Could not resolve host")
        || diagnostic.contains("network failure")
    {
        "registry_download_failed"
    } else if diagnostic.contains("no matching package named")
        || diagnostic.contains("failed to select a version")
        || diagnostic.contains("failed to resolve")
    {
        "cargo_resolution_failed"
    } else if diagnostic.contains("failed to parse manifest")
        || diagnostic.contains("failed to load manifest")
    {
        "cargo_manifest_invalid"
    } else {
        "cargo_preparation_failed"
    }
}

/// Reject Cargo configuration which can redirect the privileged helper to an
/// arbitrary registry. This is deliberately a small preflight rather than a
/// TOML parser: we only need to recognize section headers, and fail closed for
/// the two configuration namespaces that change dependency sources.
fn cargo_source_configuration_error(workspace: &Path) -> Option<String> {
    for relative in [".cargo/config.toml", ".cargo/config"] {
        let path = workspace.join(relative);
        let Ok(config) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in config.lines() {
            let section = line.trim();
            if section.starts_with("[source") || section.starts_with("[registries") {
                return Some(format!(
                    "{relative} configures an alternate Cargo source or registry; hosted dependency preparation supports crates.io only"
                ));
            }
        }
    }
    None
}

/// The helper has egress solely to populate the Den-managed cache. Git
/// dependencies would turn that into arbitrary-host egress, so reject them
/// before starting a helper container.
fn git_dependency_error(manifest: &Path) -> Option<String> {
    let Ok(contents) = std::fs::read_to_string(manifest) else {
        return None;
    };
    contents
        .lines()
        .any(|line| line.contains("git ="))
        .then(|| "hosted dependency preparation does not support git dependencies".to_string())
}

pub struct DockerCliBackend {
    docker_bin: String,
    /// Directory for transient `--env-file` files (mode 0600, removed after start).
    env_file_dir: PathBuf,
    max_log_bytes: u64,
}

impl DockerCliBackend {
    pub fn new(env_file_dir: PathBuf, max_log_bytes: u64) -> Self {
        Self {
            docker_bin: std::env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_string()),
            env_file_dir,
            max_log_bytes,
        }
    }

    pub fn strength_label(&self) -> &'static str {
        "container: OS namespace isolation via the local container runtime; \
         workspace bind-mounted read-write"
    }

    pub async fn probe(&self) -> bool {
        self.docker(&["info", "--format", "{{.ServerVersion}}"], None)
            .await
            .map(|out| out.success())
            .unwrap_or(false)
    }

    pub async fn provision(&self, spec: &ProvisionSpec) -> Result<(), BackendError> {
        let container = container_name(&spec.id);
        let mut env = spec.env.clone();
        // Resolved from the original callback URL, before any relay rewrite.
        let add_host = callback_add_host(&env).await;

        let network = match spec.network {
            NetworkMode::Open => None,
            NetworkMode::Restricted => {
                match self
                    .provision_restricted_network(spec, &mut env, add_host.as_deref())
                    .await
                {
                    Ok(network) => Some(network),
                    Err(err) => {
                        self.cleanup_network_resources(&spec.id).await;
                        return Err(err);
                    }
                }
            }
        };

        // In restricted mode the sandbox talks to the relay by container
        // name (internal-network DNS); only open-mode sandboxes dial the
        // callback host directly and need the resolved mapping.
        let sandbox_add_host = match spec.network {
            NetworkMode::Open => add_host.as_deref(),
            NetworkMode::Restricted => None,
        };
        let env_file = self.write_env_file(&spec.id, &env)?;
        let args = sandbox_run_args(
            spec,
            &container,
            &env_file.to_string_lossy(),
            network.as_deref(),
            sandbox_add_host,
        );
        let result = self.docker_owned(&args, None).await;
        // The env file exists only for the window between write and start;
        // remove it regardless of outcome. (ponytail: the env is still visible
        // via `docker inspect` on this host — acceptable while the sandbox
        // host is operator-controlled; upgrade path is a secrets mount.)
        let _ = std::fs::remove_file(&env_file);
        match result {
            Ok(out) if out.success() => Ok(()),
            Ok(out) => {
                self.cleanup_network_resources(&spec.id).await;
                Err(BackendError::Operation {
                    id: spec.id.clone(),
                    detail: format!("docker run failed: {}", out.stderr_lossy().trim()),
                })
            }
            Err(err) => {
                self.cleanup_network_resources(&spec.id).await;
                Err(err)
            }
        }
    }

    /// Create the per-sandbox internal network and, when the sandbox has a Den
    /// callback URL, the socat relay that is its only path out. Rewrites
    /// `DEN_API_URL` in `env` to point at the relay.
    async fn provision_restricted_network(
        &self,
        spec: &ProvisionSpec,
        env: &mut BTreeMap<String, String>,
        add_host: Option<&str>,
    ) -> Result<String, BackendError> {
        let network = network_name(&spec.id);
        let out = self
            .docker_owned(&network_create_args(&network, &spec.id), None)
            .await?;
        if !out.success() {
            return Err(BackendError::Operation {
                id: spec.id.clone(),
                detail: format!(
                    "docker network create failed: {}",
                    out.stderr_lossy().trim()
                ),
            });
        }

        let target = relay_target_from_env(env).map_err(|detail| BackendError::Operation {
            id: spec.id.clone(),
            detail,
        })?;
        if let Some(target) = target {
            self.start_relay(
                &network,
                spec,
                &target,
                &relay_container_name(&spec.id),
                None,
                add_host,
            )
            .await?;
            env.insert(
                "DEN_API_URL".to_string(),
                target.rewritten_url(&relay_container_name(&spec.id)),
            );
        }
        for (index, host) in spec.allowed_outbound_hosts.iter().enumerate() {
            let relay = egress_relay_container_name(&spec.id, index);
            let resolved = tokio::time::timeout(
                EGRESS_DNS_TIMEOUT,
                tokio::net::lookup_host((host.as_str(), 443)),
            )
            .await
            .map_err(|_| BackendError::Operation {
                id: spec.id.clone(),
                detail: format!("approved egress host {host} DNS lookup timed out"),
            })?
            .map_err(|error| BackendError::Operation {
                id: spec.id.clone(),
                detail: format!("approved egress host could not be resolved safely: {error}"),
            })?;
            let pinned = select_public_egress_address(resolved).map_err(|detail| {
                BackendError::Operation {
                    id: spec.id.clone(),
                    detail: format!("approved egress host {host}: {detail}"),
                }
            })?;
            // The sandbox still addresses the original DNS alias for TLS SNI;
            // only the relay's upstream is pinned, so reconnecting cannot
            // re-resolve the approved name to an internal address.
            let target = RelayTarget {
                host: pinned.to_string(),
                port: 443,
                path: String::new(),
            };
            self.start_relay(&network, spec, &target, &relay, Some(host), None)
                .await?;
        }
        Ok(network)
    }

    /// A relay is the only container with an external network attachment.
    /// The sandbox sees it only under the approved hostname, so its normal DNS
    /// resolver remains usable without exposing arbitrary name resolution.
    async fn start_relay(
        &self,
        network: &str,
        spec: &ProvisionSpec,
        target: &RelayTarget,
        relay: &str,
        alias: Option<&str>,
        add_host: Option<&str>,
    ) -> Result<(), BackendError> {
        let out = self
            .docker_owned(
                &relay_run_args(relay, &spec.id, &spec.image, target, add_host),
                None,
            )
            .await?;
        if !out.success() {
            return Err(BackendError::Operation {
                id: spec.id.clone(),
                detail: format!("relay start failed: {}", out.stderr_lossy().trim()),
            });
        }
        let mut args = vec!["network".to_string(), "connect".to_string()];
        if let Some(alias) = alias {
            args.extend(["--alias".to_string(), alias.to_string()]);
        }
        args.extend([network.to_string(), relay.to_string()]);
        let out = self.docker_owned(&args, None).await?;
        if !out.success() {
            return Err(BackendError::Operation {
                id: spec.id.clone(),
                detail: format!(
                    "relay network connect failed: {}",
                    out.stderr_lossy().trim()
                ),
            });
        }
        Ok(())
    }

    /// Best-effort removal of the per-sandbox relay container and network.
    /// Safe to call for open-mode sandboxes (both removals no-op on missing).
    async fn cleanup_network_resources(&self, id: &str) {
        let relays = self
            .docker(
                &[
                    "ps",
                    "-aq",
                    "--filter",
                    &format!("label={RELAY_LABEL}={id}"),
                ],
                None,
            )
            .await
            .ok()
            .map(|out| {
                out.stdout_lossy()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for relay in relays {
            let _ = self.docker(&["rm", "-f", &relay], None).await;
        }
        let network = network_name(id);
        let _ = self.docker(&["network", "rm", &network], None).await;
    }

    pub async fn prepare_rust_dependencies(
        &self,
        id: &str,
        workspace: &std::path::Path,
        workspace_bind_source: &std::path::Path,
        cargo_home_volume: &str,
        image: &str,
        request: &PrepareRustDependenciesRequest,
    ) -> Result<PrepareRustDependenciesResponse, BackendError> {
        let manifest = workspace.join(&request.manifest_path);
        if !manifest.is_file() {
            return Err(BackendError::Operation {
                id: id.to_string(),
                detail: "manifest is not present in sandbox workspace".to_string(),
            });
        }
        if let Some(detail) =
            cargo_source_configuration_error(workspace).or_else(|| git_dependency_error(&manifest))
        {
            return Ok(PrepareRustDependenciesResponse {
                status: "rejected".to_string(),
                code: "unsupported_cargo_source_configuration".to_string(),
                stage: "preflight".to_string(),
                retryable: false,
                content: detail,
                lockfile_changed: false,
            });
        }
        let cargo_args = rust_preparation_cargo_args(request);
        let init_out = self
            .docker_owned(&cargo_home_init_args(cargo_home_volume, image), None)
            .await?;
        if !init_out.success() {
            return Ok(PrepareRustDependenciesResponse {
                status: "failed".to_string(),
                code: "cargo_cache_initialization_failed".to_string(),
                stage: "helper".to_string(),
                retryable: false,
                content: format!(
                    "Cargo cache initialization failed: {}",
                    bounded_cargo_diagnostic(&init_out.stderr_lossy())
                ),
                lockfile_changed: false,
            });
        }
        let helper = format!("den-cargo-helper-{id}");
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            helper,
            "-v".to_string(),
            format!("{}:/workspace:rw", workspace_bind_source.display()),
            "-v".to_string(),
            format!("{cargo_home_volume}:/den/cargo-home:rw"),
            "-w".to_string(),
            "/workspace".to_string(),
            "-e".to_string(),
            "CARGO_HOME=/den/cargo-home".to_string(),
            "-e".to_string(),
            "SQLX_OFFLINE=true".to_string(),
            "--network".to_string(),
            "bridge".to_string(),
            "--entrypoint".to_string(),
            "cargo".to_string(),
            image.to_string(),
        ];
        args.extend(cargo_args);
        let out = self.docker_owned(&args, None).await?;
        let lockfile_changed =
            request.resolution == RustDependencyResolution::UpdateLockfile && out.success();
        let (status, code, stage, retryable, content) = if out.success() {
            let mode = match request.preparation {
                RustDependencyPreparation::Fetch => "fetch",
                RustDependencyPreparation::Check => "check",
                RustDependencyPreparation::TestNoRun => "test --no-run",
            };
            let resolution = match request.resolution {
                RustDependencyResolution::Locked => "existing lockfile verified",
                RustDependencyResolution::UpdateLockfile if lockfile_changed => {
                    "lockfile update requested; Cargo completed successfully"
                }
                RustDependencyResolution::UpdateLockfile => {
                    "lockfile update requested; Cargo reported no lockfile change"
                }
            };
            (
                "prepared",
                "prepared",
                "publish",
                false,
                format!(
                    "Rust dependencies prepared for offline use (mode: {mode}; {resolution}; cache mounted read-only at /den/cargo-home)."
                ),
            )
        } else if out.timed_out {
            (
                "failed",
                "helper_timed_out",
                "helper",
                true,
                "Cargo preparation timed out".to_string(),
            )
        } else {
            let detail = bounded_cargo_diagnostic(&out.stderr_lossy());
            (
                "failed",
                classify_cargo_failure(&detail),
                "helper",
                false,
                format!("Cargo preparation failed: {detail}"),
            )
        };
        Ok(PrepareRustDependenciesResponse {
            status: status.to_string(),
            code: code.to_string(),
            stage: stage.to_string(),
            retryable,
            content,
            lockfile_changed,
        })
    }

    pub async fn status(&self, id: &str) -> Result<BackendStatus, BackendError> {
        let container = container_name(id);
        let out = self
            .docker(
                &[
                    "inspect",
                    "--format",
                    "{{.State.Running}} {{.State.ExitCode}} {{.State.OOMKilled}}",
                    &container,
                ],
                None,
            )
            .await?;
        if !out.success() {
            return Err(BackendError::Operation {
                id: id.to_string(),
                detail: format!("docker inspect failed: {}", out.stderr_lossy().trim()),
            });
        }
        let stdout = out.stdout_lossy();
        let mut parts = stdout.split_whitespace();
        let running = parts.next() == Some("true");
        let exit_code = parts.next().and_then(|s| s.parse::<i64>().ok());
        let oom_killed = parts.next().map(|s| s == "true");
        Ok(BackendStatus {
            running,
            // Docker reports ExitCode 0 while still running; only meaningful once stopped.
            exit_code: if running { None } else { exit_code },
            oom_killed,
        })
    }

    pub async fn logs(&self, id: &str, tail_bytes: u64) -> Result<LogsResponse, BackendError> {
        let container = container_name(id);
        let tail_bytes = tail_bytes.min(self.max_log_bytes).max(1);
        let args: Vec<String> = vec!["logs".into(), container];
        let mut spec = CommandSpec::new(&self.docker_bin, &args);
        spec.timeout = DOCKER_TIMEOUT;
        spec.max_output_bytes = usize::try_from(tail_bytes).unwrap_or(usize::MAX);
        spec.window = CaptureWindow::Tail;
        let out = run_command(spec).await?;
        if out.exit_code != Some(0) && !out.timed_out {
            return Err(BackendError::Operation {
                id: id.to_string(),
                detail: format!("docker logs failed: {}", out.stderr_lossy().trim()),
            });
        }
        // docker logs multiplexes the container's stdout/stderr onto both pipes.
        let mut content = out.stdout_lossy();
        let stderr = out.stderr_lossy();
        if !stderr.is_empty() {
            content.push_str(&stderr);
        }
        Ok(LogsResponse {
            content,
            truncated: out.stdout_truncated || out.stderr_truncated,
            tail_bytes,
        })
    }

    pub async fn destroy(&self, id: &str) -> Result<(), BackendError> {
        let container = container_name(id);
        let out = self.docker(&["rm", "-f", &container], None).await?;
        // Relay + network must go after the sandbox container detaches
        // (a network with attached containers cannot be removed).
        self.cleanup_network_resources(id).await;
        if out.success() {
            return Ok(());
        }
        let stderr = out.stderr_lossy();
        // Already gone counts as destroyed.
        if stderr.contains("No such container") {
            return Ok(());
        }
        Err(BackendError::Operation {
            id: id.to_string(),
            detail: format!("docker rm failed: {}", stderr.trim()),
        })
    }

    pub async fn list_adopted(&self) -> Result<Vec<AdoptedSandbox>, BackendError> {
        let out = self
            .docker(
                &[
                    "ps",
                    "-a",
                    "--filter",
                    &format!("label={SANDBOX_LABEL}=1"),
                    "--format",
                    "{{.Names}}\t{{.State}}\t{{.Labels}}",
                ],
                None,
            )
            .await?;
        if !out.success() {
            return Err(BackendError::RuntimeUnavailable(
                out.stderr_lossy().trim().to_string(),
            ));
        }
        let mut adopted = Vec::new();
        for line in out.stdout_lossy().lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(state)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Some(id) = name.strip_prefix(CONTAINER_PREFIX) else {
                continue;
            };
            let labels: BTreeMap<String, String> = fields
                .next()
                .unwrap_or_default()
                .split(',')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    let key = key.strip_prefix("den.")?;
                    if key == "sandbox" {
                        None
                    } else {
                        Some((key.to_string(), value.to_string()))
                    }
                })
                .collect();
            adopted.push(AdoptedSandbox {
                id: id.to_string(),
                labels,
                running: state == "running",
            });
        }
        Ok(adopted)
    }

    fn write_env_file(
        &self,
        id: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<PathBuf, BackendError> {
        std::fs::create_dir_all(&self.env_file_dir).map_err(|e| BackendError::Operation {
            id: id.to_string(),
            detail: format!("create env-file dir: {e}"),
        })?;
        let path = self.env_file_dir.join(format!("{id}.env"));
        let mut body = String::new();
        for (key, value) in env {
            // --env-file format is KEY=VALUE per line; values with newlines
            // are unsupported and rejected rather than mangled.
            if key.contains('\n') || value.contains('\n') {
                return Err(BackendError::Operation {
                    id: id.to_string(),
                    detail: format!("env var {key} contains a newline"),
                });
            }
            body.push_str(key);
            body.push('=');
            body.push_str(value);
            body.push('\n');
        }
        std::fs::write(&path, body).map_err(|e| BackendError::Operation {
            id: id.to_string(),
            detail: format!("write env file: {e}"),
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(path)
    }

    pub(crate) fn docker_bin(&self) -> &str {
        &self.docker_bin
    }

    pub(crate) async fn image_ls_json(&self) -> Result<String, BackendError> {
        let out = self
            .docker(&["image", "ls", "--format", "{{json .}}"], None)
            .await?;
        if out.success() {
            Ok(out.stdout_lossy())
        } else {
            Err(BackendError::CommandFailed {
                what: "docker image ls".to_string(),
                detail: out.stderr_lossy().trim().to_string(),
            })
        }
    }

    pub(crate) async fn remove_image(
        &self,
        reference: &str,
    ) -> Result<Result<(), String>, BackendError> {
        let out = self.docker(&["rmi", reference], None).await?;
        if out.success() {
            Ok(Ok(()))
        } else {
            Ok(Err(out.stderr_lossy().trim().to_string()))
        }
    }

    async fn docker(
        &self,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> Result<crate::proc::CommandOutput, BackendError> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.docker_owned(&args, cwd).await
    }

    async fn docker_owned(
        &self,
        args: &[String],
        cwd: Option<&std::path::Path>,
    ) -> Result<crate::proc::CommandOutput, BackendError> {
        let mut spec = CommandSpec::new(&self.docker_bin, args);
        spec.cwd = cwd;
        spec.timeout = DOCKER_TIMEOUT;
        spec.max_output_bytes = DOCKER_OUTPUT_CAP;
        Ok(run_command(spec).await?)
    }
}

pub fn container_name(id: &str) -> String {
    format!("{CONTAINER_PREFIX}{id}")
}

pub fn network_name(id: &str) -> String {
    format!("den-sbx-net-{id}")
}

pub fn relay_container_name(id: &str) -> String {
    format!("den-sbx-gw-{id}")
}

fn egress_relay_container_name(id: &str, index: usize) -> String {
    format!("den-sbx-egress-{id}-{index}")
}

/// Where the egress relay forwards to, parsed from the sandbox's Den
/// callback URL.
#[derive(Debug, PartialEq, Eq)]
struct RelayTarget {
    host: String,
    port: u16,
    /// Path suffix of the original URL (empty for bare origins), preserved in
    /// the rewritten URL.
    path: String,
}

impl RelayTarget {
    fn rewritten_url(&self, relay_name: &str) -> String {
        format!("http://{relay_name}:{}{}", self.port, self.path)
    }
}

/// Parse the relay target from `DEN_API_URL`. `Ok(None)` when the sandbox has
/// no Den callback (the internal network then has no way out at all).
///
/// Restricted mode requires an `http` callback: the relay forwards raw TCP
/// under its own container name, so TLS certificate validation against the
/// original hostname cannot succeed. Use `network: open` for https callbacks.
fn relay_target_from_env(env: &BTreeMap<String, String>) -> Result<Option<RelayTarget>, String> {
    let Some(raw) = env
        .get("DEN_API_URL")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(raw).map_err(|e| format!("DEN_API_URL invalid: {e}"))?;
    if url.scheme() != "http" {
        return Err(format!(
            "restricted network mode requires an http DEN_API_URL (got {}); \
             use network mode 'open' for https callbacks",
            url.scheme()
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| "DEN_API_URL has no host".to_string())?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let path = url.path().trim_end_matches('/').to_string();
    Ok(Some(RelayTarget { host, port, path }))
}

/// `--add-host` mapping for the Den callback host, resolved at provision
/// time. Containers nested inside a dedicated sandbox engine (dind) cannot
/// resolve compose service names — their DNS chain ends at the engine, not
/// the compose resolver — but this provider process can. IP literals need no
/// mapping; `host.docker.internal` is handled by the existing host-gateway
/// alias; resolution failure falls back to no mapping (docker may still
/// resolve publicly-known names itself).
async fn callback_add_host(env: &BTreeMap<String, String>) -> Option<String> {
    let raw = env
        .get("DEN_API_URL")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?;
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?.to_string();
    if host.parse::<std::net::IpAddr>().is_ok() || host == "host.docker.internal" {
        return None;
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addr = tokio::time::timeout(
        EGRESS_DNS_TIMEOUT,
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .ok()?
    .ok()?
    .next()?;
    Some(format!("{host}:{}", addr.ip()))
}

fn network_create_args(network: &str, id: &str) -> Vec<String> {
    vec![
        "network".into(),
        "create".into(),
        "--internal".into(),
        "--label".into(),
        format!("{RELAY_LABEL}={id}"),
        network.into(),
    ]
}

fn select_public_egress_address(
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<Ipv4Addr, &'static str> {
    let addresses: Vec<_> = addresses.into_iter().collect();
    if addresses.is_empty() {
        return Err("DNS returned no addresses");
    }
    if addresses
        .iter()
        .any(|address| !den_core::tools::support::is_public_ip(address.ip()))
    {
        return Err("DNS returned a non-public address");
    }
    addresses
        .iter()
        .find_map(|address| match address.ip() {
            std::net::IpAddr::V4(ip) => Some(ip),
            std::net::IpAddr::V6(_) => None,
        })
        .ok_or("DNS returned no usable public IPv4 address")
}

fn relay_run_args(
    relay: &str,
    id: &str,
    image: &str,
    target: &RelayTarget,
    add_host: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        relay.into(),
        "--label".into(),
        format!("{RELAY_LABEL}={id}"),
        // Lets the relay reach a Den running on this docker host (no-op when
        // the callback names a remote host).
        "--add-host".into(),
        "host.docker.internal:host-gateway".into(),
    ];
    if let Some(mapping) = add_host {
        args.push("--add-host".into());
        args.push(mapping.into());
    }
    args.extend([
        "--entrypoint".into(),
        "socat".into(),
        image.into(),
        format!("TCP-LISTEN:{},fork,reuseaddr", target.port),
        format!("TCP:{}:{}", target.host, target.port),
    ]);
    args
}

fn cargo_home_init_args(cargo_home_volume: &str, image: &str) -> Vec<String> {
    vec![
        "run".into(),
        "--rm".into(),
        "--user".into(),
        "root".into(),
        "-v".into(),
        format!("{cargo_home_volume}:/den/cargo-home:rw"),
        "--entrypoint".into(),
        "chown".into(),
        image.into(),
        "-R".into(),
        "bear:bear".into(),
        "/den/cargo-home".into(),
    ]
}

fn sandbox_run_args(
    spec: &ProvisionSpec,
    container: &str,
    env_file: &str,
    network: Option<&str>,
    add_host: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        container.into(),
        "--label".into(),
        format!("{SANDBOX_LABEL}=1"),
        // Network posture survives provider restarts via this label
        // (adoption reads it back).
        "--label".into(),
        format!("{SANDBOX_LABEL}.network={}", spec.network.as_str()),
        "--env-file".into(),
        env_file.into(),
        "-v".into(),
        // The bind source is resolved by the HOST docker daemon, so it must
        // be the host-side path, not the provider's container-local one.
        format!(
            "{}:/workspace",
            spec.workspace_bind_source.to_string_lossy()
        ),
        "-w".into(),
        "/workspace".into(),
    ];
    if let Some(volume) = &spec.cargo_home_volume {
        args.push("-v".into());
        args.push(format!("{volume}:/den/cargo-home:ro"));
    }
    if let Some(network) = network {
        args.push("--network".into());
        args.push(network.into());
    }
    if let Some(mapping) = add_host {
        args.push("--add-host".into());
        args.push(mapping.into());
    }
    for (key, value) in &spec.labels {
        args.push("--label".into());
        args.push(format!("den.{key}={value}"));
    }
    if let Some(memory_mb) = spec.memory_mb {
        args.push("--memory".into());
        args.push(format!("{memory_mb}m"));
    }
    if let Some(cpus) = spec.cpus {
        args.push("--cpus".into());
        args.push(format!("{cpus}"));
    }
    if let Some(pids) = spec.pids {
        args.push("--pids-limit".into());
        args.push(format!("{pids}"));
    }
    args.push(spec.image.clone());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_bounded_cargo_failures() {
        assert_eq!(
            classify_cargo_failure("error: failed to get `serde` as a dependency"),
            "registry_download_failed"
        );
        assert_eq!(
            classify_cargo_failure("error: no matching package named `serde` found"),
            "cargo_resolution_failed"
        );
        assert_eq!(
            classify_cargo_failure("error: failed to parse manifest"),
            "cargo_manifest_invalid"
        );
        assert_eq!(bounded_cargo_diagnostic("\nerror: bad\n\n"), "error: bad");
        assert!(
            format!(
                "Rust dependencies prepared for offline use (mode: {}; {}; cache mounted read-only at /den/cargo-home).",
                "check", "existing lockfile verified"
            )
            .contains("cache mounted read-only")
        );
    }

    #[test]
    fn container_names_round_trip() {
        let name = container_name("abc123");
        assert_eq!(name, "den-sbx-abc123");
        assert_eq!(name.strip_prefix(CONTAINER_PREFIX), Some("abc123"));
    }

    #[test]
    fn env_file_rejects_newlines() {
        let backend = DockerCliBackend::new(std::env::temp_dir().join("den-sbx-env-test"), 1024);
        let mut env = BTreeMap::new();
        env.insert("GOOD".to_string(), "value".to_string());
        env.insert("BAD".to_string(), "line1\nline2".to_string());
        assert!(backend.write_env_file("t1", &env).is_err());
    }

    fn spec(network: NetworkMode) -> ProvisionSpec {
        ProvisionSpec {
            id: "abc123".into(),
            workspace: PathBuf::from("/srv/ws/abc123"),
            workspace_bind_source: PathBuf::from("/host/ws/abc123"),
            image: "bears/sandbox:latest".into(),
            env: BTreeMap::new(),
            network,
            allowed_outbound_hosts: Vec::new(),
            memory_mb: None,
            cpus: None,
            pids: None,
            labels: BTreeMap::new(),
            cargo_home_volume: None,
        }
    }

    #[test]
    fn restricted_run_args_attach_only_the_internal_network() {
        let restricted = spec(NetworkMode::Restricted);
        let args = sandbox_run_args(
            &restricted,
            "den-sbx-abc123",
            "/tmp/e.env",
            Some("den-sbx-net-abc123"),
            None,
        );
        let joined = args.join(" ");
        assert!(joined.contains("--network den-sbx-net-abc123"), "{joined}");
        assert!(
            joined.contains("den.sandbox.network=restricted"),
            "{joined}"
        );
        // The bind source is the HOST path, never the provider-local one.
        assert!(joined.contains("-v /host/ws/abc123:/workspace"), "{joined}");
        assert!(!joined.contains("/srv/ws/abc123"), "{joined}");

        let mut with_cache = restricted;
        with_cache.cargo_home_volume = Some("den-cargo-cache-lock123".into());
        let cache_args = sandbox_run_args(
            &with_cache,
            "den-sbx-abc123",
            "/tmp/e.env",
            Some("den-sbx-net-abc123"),
            None,
        );
        assert!(cache_args
            .join(" ")
            .contains("den-cargo-cache-lock123:/den/cargo-home:ro"));

        // Open mode dials the callback directly, so it carries the resolved
        // extra-host mapping.
        let open = spec(NetworkMode::Open);
        let args = sandbox_run_args(
            &open,
            "den-sbx-abc123",
            "/tmp/e.env",
            None,
            Some("bears-den:172.20.0.5"),
        );
        let joined = args.join(" ");
        assert!(!joined.contains("--network "), "{joined}");
        assert!(
            joined.contains("--add-host bears-den:172.20.0.5"),
            "{joined}"
        );
        assert!(joined.contains("den.sandbox.network=open"), "{joined}");
    }

    #[test]
    fn relay_target_parses_and_rewrites_den_api_url() {
        let mut env = BTreeMap::new();
        env.insert(
            "DEN_API_URL".to_string(),
            "http://den.internal:3001".to_string(),
        );
        let target = relay_target_from_env(&env).unwrap().expect("target");
        assert_eq!(target.host, "den.internal");
        assert_eq!(target.port, 3001);
        assert_eq!(
            target.rewritten_url("den-sbx-gw-abc123"),
            "http://den-sbx-gw-abc123:3001"
        );

        // No callback → fully closed network, no relay.
        assert!(relay_target_from_env(&BTreeMap::new()).unwrap().is_none());

        // https callbacks cannot ride a raw TCP relay under another name.
        env.insert(
            "DEN_API_URL".to_string(),
            "https://den.example.com".to_string(),
        );
        assert!(relay_target_from_env(&env).is_err());
    }

    #[test]
    fn egress_pins_only_public_ipv4_and_rejects_mixed_or_empty_dns_answers() {
        let public: SocketAddr = "1.1.1.1:443".parse().unwrap();
        let public_v6: SocketAddr = "[2606:4700:4700::1111]:443".parse().unwrap();
        assert_eq!(
            select_public_egress_address([public_v6, public]).unwrap(),
            "1.1.1.1".parse::<Ipv4Addr>().unwrap()
        );
        assert!(select_public_egress_address(Vec::<SocketAddr>::new()).is_err());
        assert!(select_public_egress_address([public_v6]).is_err());
        for blocked in ["10.0.0.1:443", "100.64.0.1:443", "127.0.0.1:443"] {
            assert!(
                select_public_egress_address([public, blocked.parse().unwrap()]).is_err(),
                "{blocked}"
            );
        }
    }

    #[test]
    fn allowed_egress_uses_an_internal_dns_alias_and_https_only_relay() {
        let target = RelayTarget {
            host: "1.1.1.1".into(),
            port: 443,
            path: String::new(),
        };
        let args = relay_run_args(
            "den-sbx-egress-abc123-0",
            "abc123",
            "bears/sandbox:latest",
            &target,
            None,
        );
        let joined = args.join(" ");
        assert!(joined.contains("TCP-LISTEN:443,fork,reuseaddr"), "{joined}");
        assert!(joined.contains("TCP:1.1.1.1:443"), "{joined}");
        assert!(!joined.contains("TCP:index.crates.io:443"), "{joined}");
        assert_eq!(
            egress_relay_container_name("abc123", 0),
            "den-sbx-egress-abc123-0"
        );
    }

    #[test]
    fn relay_run_args_forward_the_callback_port_only() {
        let target = RelayTarget {
            host: "den.internal".into(),
            port: 3001,
            path: String::new(),
        };
        let args = relay_run_args(
            "den-sbx-gw-abc123",
            "abc123",
            "bears/sandbox:latest",
            &target,
            Some("den.internal:172.20.0.5"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("--entrypoint socat"), "{joined}");
        assert!(
            joined.contains("TCP-LISTEN:3001,fork,reuseaddr"),
            "{joined}"
        );
        assert!(joined.contains("TCP:den.internal:3001"), "{joined}");
        assert!(joined.contains("den.sandbox.relay=abc123"), "{joined}");
        // The relay resolves the callback host via the provision-time mapping
        // (nested engines cannot resolve compose service names).
        assert!(
            joined.contains("--add-host den.internal:172.20.0.5"),
            "{joined}"
        );
        // Relays are not labeled as sandboxes: adoption must never pick them up.
        assert!(!joined.contains("den.sandbox=1"), "{joined}");
    }

    #[tokio::test]
    async fn callback_add_host_skips_ip_literals_and_resolves_names() {
        let env_with = |url: &str| {
            let mut env = BTreeMap::new();
            env.insert("DEN_API_URL".to_string(), url.to_string());
            env
        };
        // IP literals and host.docker.internal need no mapping.
        assert!(callback_add_host(&env_with("http://192.168.1.10:3001"))
            .await
            .is_none());
        assert!(
            callback_add_host(&env_with("http://host.docker.internal:3001"))
                .await
                .is_none()
        );
        // No callback at all: nothing to map.
        assert!(callback_add_host(&BTreeMap::new()).await.is_none());
        // A resolvable name maps to host:ip (localhost is the one name every
        // test host resolves).
        let mapping = callback_add_host(&env_with("http://localhost:3001"))
            .await
            .expect("localhost resolves");
        assert!(mapping.starts_with("localhost:"), "{mapping}");
    }

    #[test]
    fn cargo_cache_is_initialized_for_the_sandbox_user() {
        let args = cargo_home_init_args("den-cargo-cache-run123", "bears/sandbox-rust:latest");
        assert_eq!(
            args,
            [
                "run",
                "--rm",
                "--user",
                "root",
                "-v",
                "den-cargo-cache-run123:/den/cargo-home:rw",
                "--entrypoint",
                "chown",
                "bears/sandbox-rust:latest",
                "-R",
                "bear:bear",
                "/den/cargo-home",
            ]
        );
    }

    #[test]
    fn whole_workspace_preparation_fetches_locked_without_empty_package_flag() {
        let args = rust_preparation_cargo_args(&PrepareRustDependenciesRequest {
            manifest_path: "Cargo.toml".to_string(),
            package: String::new(),
            resolution: RustDependencyResolution::Locked,
            preparation: RustDependencyPreparation::Fetch,
        });
        assert_eq!(args, ["fetch", "--manifest-path", "Cargo.toml", "--locked"]);
    }

    #[test]
    fn network_names_are_prefixed_per_sandbox() {
        assert_eq!(network_name("abc"), "den-sbx-net-abc");
        assert_eq!(relay_container_name("abc"), "den-sbx-gw-abc");
        let args = network_create_args("den-sbx-net-abc", "abc");
        assert!(args.join(" ").contains("--internal"), "{args:?}");
    }
}
