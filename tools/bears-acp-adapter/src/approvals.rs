use crate::{env_bool, paths::session_workspace_roots, RuntimeConfig, SessionContext};
use agent_client_protocol::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionResponse,
};
use anyhow::Result;
use reqwest::Url;
use serde_json::Value;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex as TokioMutex;

#[derive(Clone, Default)]
pub(crate) struct ApprovalCache {
    pub(crate) entries: Arc<TokioMutex<HashMap<String, ApprovalRecord>>>,
    pub(crate) persistence: Option<ApprovalPersistence>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovalPersistence {
    pub(crate) path: PathBuf,
    pub(crate) api_url: String,
    pub(crate) bear: String,
    pub(crate) client: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ApprovalRecord {
    pub(crate) api_url: String,
    pub(crate) bear: String,
    pub(crate) client: String,
    pub(crate) tool_name: String,
    pub(crate) permission_class: String,
    // Legacy workspace-root fingerprint. Kept so existing cache files remain readable.
    pub(crate) root_fingerprint: String,
    #[serde(default = "default_approval_scope_kind")]
    pub(crate) scope_kind: String,
    #[serde(default)]
    pub(crate) scope_fingerprint: String,
    pub(crate) risk: String,
    pub(crate) created_at_secs: u64,
    pub(crate) expires_at_secs: u64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ApprovalCacheFile {
    version: u32,
    entries: Vec<ApprovalRecord>,
}

fn default_approval_scope_kind() -> String {
    ApprovalScope::Workspace.as_str().to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApprovalScope {
    Directory,
    Workspace,
    Host,
    Global,
}

impl ApprovalScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Workspace => "workspace",
            Self::Host => "host",
            Self::Global => "global",
        }
    }
}

impl ApprovalCache {
    pub(crate) fn key(
        api_url: &str,
        bear: &str,
        client: &str,
        permission_class: &str,
        scope_kind: &str,
        scope_fingerprint: &str,
    ) -> String {
        format!(
            "{api_url}\n{bear}\n{client}\n{permission_class}\n{scope_kind}\n{scope_fingerprint}"
        )
    }

    pub(crate) async fn load_for_runtime(runtime: &RuntimeConfig) -> Self {
        if env_bool("BEARS_ACP_DISABLE_PERSISTENT_APPROVALS") {
            return Self::default();
        }
        let Some(config) = runtime.config.as_ref() else {
            return Self::default();
        };
        let path = approval_cache_path();
        let persistence = ApprovalPersistence {
            path: path.clone(),
            api_url: config.api_url.clone(),
            bear: config.bear.clone(),
            client: config.client.clone(),
        };
        let cache = Self {
            entries: Arc::new(TokioMutex::new(HashMap::new())),
            persistence: Some(persistence),
        };
        if env_bool("BEARS_ACP_CLEAR_APPROVALS") {
            let _ = fs::remove_file(&path);
            return cache;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(file) = serde_json::from_str::<ApprovalCacheFile>(&raw) {
                let now = now_secs();
                let mut entries = cache.entries.lock().await;
                for mut record in file.entries.into_iter().filter(|r| r.expires_at_secs > now) {
                    if record.permission_class.trim().is_empty() {
                        record.permission_class =
                            permission_class_for_tool(&record.tool_name).to_string();
                    }
                    if record.scope_fingerprint.trim().is_empty() {
                        record.scope_fingerprint = record.root_fingerprint.clone();
                    }
                    let key = Self::key(
                        &record.api_url,
                        &record.bear,
                        &record.client,
                        &record.permission_class,
                        &record.scope_kind,
                        &record.scope_fingerprint,
                    );
                    entries.insert(key, record);
                }
            }
        }
        cache
    }

    pub(crate) async fn remember(
        &self,
        context: &SessionContext,
        tool_name: &str,
        risk: &str,
        scope: ApprovalScope,
        target_path: Option<&Path>,
    ) {
        self.remember_for_target(context, tool_name, risk, scope, target_path, None)
            .await;
    }

    pub(crate) async fn remember_for_url(
        &self,
        context: &SessionContext,
        tool_name: &str,
        risk: &str,
        scope: ApprovalScope,
        target_url: Option<&str>,
    ) {
        self.remember_for_target(context, tool_name, risk, scope, None, target_url)
            .await;
    }

    pub(crate) async fn remember_for_target(
        &self,
        context: &SessionContext,
        tool_name: &str,
        risk: &str,
        scope: ApprovalScope,
        target_path: Option<&Path>,
        target_url: Option<&str>,
    ) {
        let Some(persistence) = self.persistence.as_ref() else {
            return;
        };
        let root_fingerprint = approval_root_fingerprint(context);
        let Some(scope_fingerprint) =
            approval_scope_fingerprint(context, scope, target_path, target_url)
        else {
            return;
        };
        let now = now_secs();
        let record = ApprovalRecord {
            api_url: persistence.api_url.clone(),
            bear: persistence.bear.clone(),
            client: persistence.client.clone(),
            tool_name: tool_name.to_string(),
            permission_class: permission_class_for_tool(tool_name).to_string(),
            root_fingerprint,
            scope_kind: scope.as_str().to_string(),
            scope_fingerprint,
            risk: risk.to_string(),
            created_at_secs: now,
            expires_at_secs: now + approval_ttl_secs(risk),
        };
        self.entries.lock().await.insert(
            Self::key(
                &record.api_url,
                &record.bear,
                &record.client,
                &record.permission_class,
                &record.scope_kind,
                &record.scope_fingerprint,
            ),
            record,
        );
        self.save().await;
    }

    pub(crate) async fn is_allowed(
        &self,
        context: &SessionContext,
        tool_name: &str,
        target_path: Option<&Path>,
    ) -> bool {
        self.is_allowed_for_target(context, tool_name, target_path, None)
            .await
    }

    pub(crate) async fn is_allowed_for_url(
        &self,
        context: &SessionContext,
        tool_name: &str,
        target_url: Option<&str>,
    ) -> bool {
        self.is_allowed_for_target(context, tool_name, None, target_url)
            .await
    }

    pub(crate) async fn is_allowed_for_target(
        &self,
        context: &SessionContext,
        tool_name: &str,
        target_path: Option<&Path>,
        target_url: Option<&str>,
    ) -> bool {
        let Some(persistence) = self.persistence.as_ref() else {
            return false;
        };
        let permission_class = permission_class_for_tool(tool_name);
        let candidate_scopes = candidate_approval_scopes(target_path, target_url);
        let candidate_keys = candidate_scopes
            .into_iter()
            .filter_map(|scope| {
                approval_scope_fingerprint(context, scope, target_path, target_url).map(
                    |fingerprint| {
                        Self::key(
                            &persistence.api_url,
                            &persistence.bear,
                            &persistence.client,
                            permission_class,
                            scope.as_str(),
                            &fingerprint,
                        )
                    },
                )
            })
            .collect::<Vec<_>>();
        let now = now_secs();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, record| record.expires_at_secs > now);
        candidate_keys.iter().any(|key| entries.contains_key(key))
    }

    pub(crate) async fn clear_session(&self, _session_id: &str) {
        // Persistent approvals intentionally survive ACP session boundaries.
        // Use BEARS_ACP_CLEAR_APPROVALS=1 or remove the cache file to revoke.
    }

    async fn save(&self) {
        let Some(persistence) = self.persistence.as_ref() else {
            return;
        };
        let now = now_secs();
        let entries = self
            .entries
            .lock()
            .await
            .values()
            .filter(|record| record.expires_at_secs > now)
            .cloned()
            .collect::<Vec<_>>();
        let file = ApprovalCacheFile {
            version: 1,
            entries,
        };
        if let Some(parent) = persistence.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = persistence.path.with_extension("tmp");
        if let Ok(raw) = serde_json::to_string_pretty(&file) {
            if fs::write(&tmp, raw).is_ok() {
                let _ = fs::rename(tmp, &persistence.path);
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn approval_ttl_secs(risk: &str) -> u64 {
    if matches!(risk, "writes_workspace" | "deletes_workspace") {
        7 * 24 * 60 * 60
    } else {
        28 * 24 * 60 * 60
    }
}

fn approval_cache_path() -> PathBuf {
    if let Ok(path) = env::var("BEARS_ACP_APPROVALS_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("bears")
            .join("acp-approvals.json");
    }
    PathBuf::from(".bears-acp-approvals.json")
}

pub(crate) fn permission_class_for_tool(tool_name: &str) -> &'static str {
    match tool_name {
        "fs_read_text_file" | "fs_list_directory" | "fs_search_files" | "fs_find_paths"
        | "fs_stat" | "fs.read_text_file" | "read_text_file" => "read_files",
        "fs_replace_text"
        | "fs_create_text_file"
        | "fs_create_directory"
        | "fs_move_path"
        | "fs_copy_path"
        | "fs_apply_patch" => "edit_files",
        "fs_delete_path" => "delete_files",
        "git_status" | "git_diff" | "git_log" | "git_show" | "git_blame" => "git_read",
        "git_add" | "git_restore" | "git_commit" | "git_stash" => "git_write",
        "process_run" => "command_run",
        "web_search" | "web_fetch" | "http_request" => "network",
        _ => "local_files",
    }
}

pub(crate) fn approval_root_fingerprint(context: &SessionContext) -> String {
    let roots = if context.roots.is_empty() {
        vec![context.cwd.clone()]
    } else {
        context.roots.clone()
    };
    roots.join("|")
}

fn approval_scope_fingerprint(
    context: &SessionContext,
    scope: ApprovalScope,
    target_path: Option<&Path>,
    target_url: Option<&str>,
) -> Option<String> {
    match scope {
        ApprovalScope::Directory => {
            approval_directory_scope(context, target_path).map(|path| path.display().to_string())
        }
        ApprovalScope::Workspace => Some(approval_root_fingerprint(context)),
        ApprovalScope::Host => target_url.and_then(approval_url_host_scope),
        ApprovalScope::Global => Some("global".to_string()),
    }
}

pub(crate) fn candidate_approval_scopes(
    target_path: Option<&Path>,
    target_url: Option<&str>,
) -> Vec<ApprovalScope> {
    if target_url.and_then(approval_url_host_scope).is_some() {
        return vec![ApprovalScope::Host, ApprovalScope::Global];
    }
    if target_path.is_some() {
        return vec![
            ApprovalScope::Directory,
            ApprovalScope::Workspace,
            ApprovalScope::Global,
        ];
    }
    vec![ApprovalScope::Workspace, ApprovalScope::Global]
}

pub(crate) fn approval_url_host_scope(raw_url: &str) -> Option<String> {
    let url = Url::parse(raw_url.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

pub(crate) fn approval_directory_scope(
    context: &SessionContext,
    target_path: Option<&Path>,
) -> Option<PathBuf> {
    let path = target_path?;
    let roots = session_workspace_roots(context);
    if roots.iter().any(|root| path == root) {
        return None;
    }
    let directory = path.parent()?.to_path_buf();
    if roots.iter().any(|root| directory == *root) {
        return None;
    }
    Some(directory)
}

pub(crate) fn approval_workspace_scope_label(context: &SessionContext) -> String {
    let roots = session_workspace_roots(context);
    if roots.len() == 1 {
        roots
            .first()
            .map(|root| root.display().to_string())
            .unwrap_or_else(|| "this workspace".to_string())
    } else {
        "workspace roots".to_string()
    }
}

pub(crate) fn permission_options_for_context(
    context: Option<&SessionContext>,
    target_path: Option<&Path>,
    target_url: Option<&str>,
) -> Vec<PermissionOption> {
    let mut options = vec![PermissionOption::new(
        "allow_once",
        "Only this time",
        PermissionOptionKind::AllowOnce,
    )];
    if let Some(host) = target_url.and_then(approval_url_host_scope) {
        options.push(PermissionOption::new(
            "allow_host",
            format!("Always for {host}"),
            PermissionOptionKind::AllowAlways,
        ));
    } else if let Some(context) = context {
        if let Some(directory) = approval_directory_scope(context, target_path) {
            options.push(PermissionOption::new(
                "allow_directory",
                format!("Always for {}", directory.display()),
                PermissionOptionKind::AllowAlways,
            ));
        }
        options.push(PermissionOption::new(
            "allow_workspace",
            format!("Always for {}", approval_workspace_scope_label(context)),
            PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(PermissionOption::new(
        "allow_global",
        "Always",
        PermissionOptionKind::AllowAlways,
    ));
    options.push(PermissionOption::new(
        "reject_once",
        "Deny",
        PermissionOptionKind::RejectOnce,
    ));
    options.push(PermissionOption::new(
        "reject_always",
        "Always deny",
        PermissionOptionKind::RejectAlways,
    ));
    options
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PermissionDecision {
    pub(crate) approved: bool,
    pub(crate) remember: bool,
    pub(crate) scope: ApprovalScope,
}

pub(crate) fn parse_permission_decision(result: &Value) -> Result<PermissionDecision> {
    if let Ok(response) = serde_json::from_value::<RequestPermissionResponse>(result.clone()) {
        return Ok(match response.outcome {
            RequestPermissionOutcome::Selected(selected) => {
                permission_decision_from_option_id(&selected.option_id.to_string())
            }
            RequestPermissionOutcome::Cancelled => PermissionDecision {
                approved: false,
                remember: false,
                scope: ApprovalScope::Workspace,
            },
            _ => PermissionDecision {
                approved: false,
                remember: false,
                scope: ApprovalScope::Workspace,
            },
        });
    }
    let approved = result
        .get("approved")
        .or_else(|| result.get("approve"))
        .or_else(|| result.get("granted"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            // Some clients answer `{}` after applying their own auto-approval policy.
            result.is_object()
        });
    Ok(PermissionDecision {
        approved,
        remember: false,
        scope: ApprovalScope::Workspace,
    })
}

pub(crate) fn permission_decision_from_option_id(id: &str) -> PermissionDecision {
    match id {
        "allow_once" | "allow" | "approve" | "approved" | "yes" => PermissionDecision {
            approved: true,
            remember: false,
            scope: ApprovalScope::Workspace,
        },
        "allow_directory" => PermissionDecision {
            approved: true,
            remember: true,
            scope: ApprovalScope::Directory,
        },
        "allow_workspace" | "allow_always" => PermissionDecision {
            approved: true,
            remember: true,
            scope: ApprovalScope::Workspace,
        },
        "allow_host" => PermissionDecision {
            approved: true,
            remember: true,
            scope: ApprovalScope::Host,
        },
        "allow_global" => PermissionDecision {
            approved: true,
            remember: true,
            scope: ApprovalScope::Global,
        },
        _ => PermissionDecision {
            approved: false,
            remember: false,
            scope: ApprovalScope::Workspace,
        },
    }
}

#[allow(dead_code)]
pub(crate) fn parse_permission_approved(result: &Value) -> Result<bool> {
    Ok(parse_permission_decision(result)?.approved)
}
