use serde_json::json;

pub mod acp_diag_phase {
    pub const DESCRIPTOR_ADVERTISED: &str = "descriptor_advertised";
    pub const LETTA_TOOL_CALL_MAPPED: &str = "letta_tool_call_mapped";
    pub const TOOL_REQUEST_REGISTERED: &str = "tool_request_registered";
    pub const ADAPTER_PERMISSION_REQUESTED: &str = "adapter_permission_requested";
    pub const ADAPTER_PERMISSION_DENIED: &str = "adapter_permission_denied";
    pub const ADAPTER_EXECUTION_STARTED: &str = "adapter_execution_started";
    pub const ADAPTER_EXECUTION_FAILED: &str = "adapter_execution_failed";
    pub const ADAPTER_RESULT_POSTED: &str = "adapter_result_posted";
    pub const DEN_RESULT_DELIVERED: &str = "den_result_delivered";
    pub const LETTA_CONTINUATION_STARTED: &str = "letta_continuation_started";
    pub const LETTA_CONTINUATION_FAILED: &str = "letta_continuation_failed";
    pub const TOOL_RESULT_TIMEOUT: &str = "tool_result_timeout";
    pub const RECENTLY_SETTLED_RESULT: &str = "recently_settled_result";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolName {
    ReadTextFile,
    ListDirectory,
    FindPaths,
    SearchFiles,
    Stat,
    ReplaceText,
    CreateTextFile,
    CreateDirectory,
    MovePath,
    CopyPath,
    ApplyPatch,
    DeletePath,
    GitStatus,
    GitDiff,
    GitLog,
    GitShow,
    GitAdd,
    GitRestore,
    GitCommit,
    GitStash,
    ProcessRun,
    ChromeOpen,
    ChromeSnapshot,
    ChromeConsoleMessages,
    ChromeNetworkRequests,
    ChromeScreenshot,
}

impl AcpToolName {
    pub fn descriptor(self) -> &'static AcpToolDescriptor {
        match self {
            Self::ReadTextFile => &ACP_READ_TEXT_FILE_TOOL,
            Self::ListDirectory => &ACP_LIST_DIRECTORY_TOOL,
            Self::FindPaths => &ACP_FIND_PATHS_TOOL,
            Self::SearchFiles => &ACP_SEARCH_FILES_TOOL,
            Self::Stat => &ACP_STAT_TOOL,
            Self::ReplaceText => &ACP_REPLACE_TEXT_TOOL,
            Self::CreateTextFile => &ACP_CREATE_TEXT_FILE_TOOL,
            Self::CreateDirectory => &ACP_CREATE_DIRECTORY_TOOL,
            Self::MovePath => &ACP_MOVE_PATH_TOOL,
            Self::CopyPath => &ACP_COPY_PATH_TOOL,
            Self::ApplyPatch => &ACP_APPLY_PATCH_TOOL,
            Self::DeletePath => &ACP_DELETE_PATH_TOOL,
            Self::GitStatus => &ACP_GIT_STATUS_TOOL,
            Self::GitDiff => &ACP_GIT_DIFF_TOOL,
            Self::GitLog => &ACP_GIT_LOG_TOOL,
            Self::GitShow => &ACP_GIT_SHOW_TOOL,
            Self::GitAdd => &ACP_GIT_ADD_TOOL,
            Self::GitRestore => &ACP_GIT_RESTORE_TOOL,
            Self::GitCommit => &ACP_GIT_COMMIT_TOOL,
            Self::GitStash => &ACP_GIT_STASH_TOOL,
            Self::ProcessRun => &ACP_PROCESS_RUN_TOOL,
            Self::ChromeOpen => &ACP_CHROME_OPEN_TOOL,
            Self::ChromeSnapshot => &ACP_CHROME_SNAPSHOT_TOOL,
            Self::ChromeConsoleMessages => &ACP_CHROME_CONSOLE_MESSAGES_TOOL,
            Self::ChromeNetworkRequests => &ACP_CHROME_NETWORK_REQUESTS_TOOL,
            Self::ChromeScreenshot => &ACP_CHROME_SCREENSHOT_TOOL,
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ReadTextFile,
            Self::ListDirectory,
            Self::FindPaths,
            Self::SearchFiles,
            Self::Stat,
            Self::ReplaceText,
            Self::CreateTextFile,
            Self::CreateDirectory,
            Self::MovePath,
            Self::CopyPath,
            Self::ApplyPatch,
            Self::DeletePath,
            Self::GitStatus,
            Self::GitDiff,
            Self::GitLog,
            Self::GitShow,
            Self::GitAdd,
            Self::GitRestore,
            Self::GitCommit,
            Self::GitStash,
            Self::ProcessRun,
            Self::ChromeOpen,
            Self::ChromeSnapshot,
            Self::ChromeConsoleMessages,
            Self::ChromeNetworkRequests,
            Self::ChromeScreenshot,
        ]
    }

    pub fn missing_required_string_arg(self, args: &serde_json::Value) -> Option<&'static str> {
        for arg in self.required_string_args() {
            if self == Self::SearchFiles
                && *arg == "query"
                && args
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
            {
                continue;
            }
            if args
                .get(arg)
                .and_then(|v| v.as_str())
                .filter(|s| self.allow_empty_required_string(*arg) || !s.trim().is_empty())
                .is_none()
            {
                return Some(arg);
            }
        }
        None
    }

    pub fn required_string_args(self) -> &'static [&'static str] {
        match self {
            Self::ReadTextFile | Self::ListDirectory | Self::Stat => &["path"],
            Self::FindPaths => &["glob"],
            Self::SearchFiles => &["path", "query"],
            Self::ReplaceText => &["path", "old_text", "new_text"],
            Self::CreateTextFile => &["path", "content"],
            Self::CreateDirectory | Self::DeletePath => &["path"],
            Self::MovePath | Self::CopyPath => &["source_path", "destination_path"],
            Self::ApplyPatch => &["patch"],
            Self::GitStatus | Self::GitDiff | Self::GitLog => &[],
            Self::GitShow => &["revision"],
            Self::GitAdd | Self::GitRestore => &["paths"],
            Self::GitCommit => &["message"],
            Self::GitStash => &[],
            Self::ProcessRun => &["command", "cwd"],
            Self::ChromeOpen => &["url"],
            Self::ChromeSnapshot
            | Self::ChromeConsoleMessages
            | Self::ChromeNetworkRequests
            | Self::ChromeScreenshot => &[],
        }
    }

    fn allow_empty_required_string(self, arg: &str) -> bool {
        matches!(self, Self::ReplaceText | Self::CreateTextFile)
            && matches!(arg, "old_text" | "new_text" | "content")
    }

    pub fn from_provider_alias(raw: &str) -> Option<Self> {
        match raw {
            "bears/read_text_file"
            | "fs.read_text_file"
            | "fs_read_text_file"
            | "read_text_file" => Some(Self::ReadTextFile),
            "bears/list_directory"
            | "fs/list_directory"
            | "fs.list_directory"
            | "fs_list_directory"
            | "list_directory" => Some(Self::ListDirectory),
            "bears/find_paths" | "fs/find_paths" | "fs.find_paths" | "fs_find_paths"
            | "find_paths" => Some(Self::FindPaths),
            "bears/search_files" | "fs/search_files" | "fs.search_files" | "fs_search_files"
            | "search_files" => Some(Self::SearchFiles),
            "bears/stat" | "fs/stat" | "fs.stat" | "fs_stat" | "stat" => Some(Self::Stat),
            "bears/replace_text" | "fs/replace_text" | "fs.replace_text" | "fs_replace_text"
            | "replace_text" => Some(Self::ReplaceText),
            "bears/create_text_file"
            | "fs/create_text_file"
            | "fs.create_text_file"
            | "fs_create_text_file"
            | "create_text_file" => Some(Self::CreateTextFile),
            "bears/create_directory"
            | "fs/create_directory"
            | "fs.create_directory"
            | "fs_create_directory"
            | "create_directory" => Some(Self::CreateDirectory),
            "bears/move_path" | "fs/move_path" | "fs.move_path" | "fs_move_path" | "move_path" => {
                Some(Self::MovePath)
            }
            "bears/copy_path" | "fs/copy_path" | "fs.copy_path" | "fs_copy_path" | "copy_path" => {
                Some(Self::CopyPath)
            }
            "bears/apply_patch" | "fs/apply_patch" | "fs.apply_patch" | "fs_apply_patch"
            | "apply_patch" => Some(Self::ApplyPatch),
            "bears/delete_path" | "fs/delete_path" | "fs.delete_path" | "fs_delete_path"
            | "delete_path" => Some(Self::DeletePath),
            "bears/git_status" | "git/status" | "git.status" | "git_status" => {
                Some(Self::GitStatus)
            }
            "bears/git_diff" | "git/diff" | "git.diff" | "git_diff" => Some(Self::GitDiff),
            "bears/git_log" | "git/log" | "git.log" | "git_log" => Some(Self::GitLog),
            "bears/git_show" | "git/show" | "git.show" | "git_show" => Some(Self::GitShow),
            "bears/git_add" | "git/add" | "git.add" | "git_add" => Some(Self::GitAdd),
            "bears/git_restore" | "git/restore" | "git.restore" | "git_restore" => {
                Some(Self::GitRestore)
            }
            "bears/git_commit" | "git/commit" | "git.commit" | "git_commit" => {
                Some(Self::GitCommit)
            }
            "bears/git_stash" | "git/stash" | "git.stash" | "git_stash" => Some(Self::GitStash),
            "bears/process_run" | "process/run" | "process.run" | "process_run" => {
                Some(Self::ProcessRun)
            }
            "bears/chrome_open" | "chrome/open" | "chrome.open" | "chrome_open" => {
                Some(Self::ChromeOpen)
            }
            "bears/chrome_snapshot" | "chrome/snapshot" | "chrome.snapshot" | "chrome_snapshot" => {
                Some(Self::ChromeSnapshot)
            }
            "bears/chrome_console_messages"
            | "chrome/console_messages"
            | "chrome.console_messages"
            | "chrome_console_messages" => Some(Self::ChromeConsoleMessages),
            "bears/chrome_network_requests"
            | "chrome/network_requests"
            | "chrome.network_requests"
            | "chrome_network_requests" => Some(Self::ChromeNetworkRequests),
            "bears/chrome_screenshot"
            | "chrome/screenshot"
            | "chrome.screenshot"
            | "chrome_screenshot" => Some(Self::ChromeScreenshot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolStatus {
    Ok,
    Error,
    Cancelled,
    Timeout,
    PermissionDenied,
    Unsupported,
}

impl AcpToolStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ok" => Some(Self::Ok),
            "error" => Some(Self::Error),
            "cancelled" => Some(Self::Cancelled),
            "timeout" => Some(Self::Timeout),
            "permission_denied" => Some(Self::PermissionDenied),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::PermissionDenied => "permission_denied",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AcpToolDescriptor {
    pub provider_name: &'static str,
    pub canonical_name: &'static str,
    pub adapter_method: &'static str,
    pub client_method: &'static str,
    pub title: &'static str,
    pub kind: &'static str,
    pub risk: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpToolPolicy {
    pub scope_basis: &'static str,
    pub role_basis: &'static str,
    pub allowed_roots_basis: &'static str,
    pub path_containment: &'static str,
    pub approval_required: bool,
    pub sensitive_path_policy: &'static str,
    pub max_lines: Option<u32>,
    pub max_entries: Option<u32>,
    pub max_results: Option<u32>,
    pub max_bytes: Option<u64>,
    pub recursive_default: Option<bool>,
    pub include_hidden_default: Option<bool>,
    pub max_replacements: Option<u32>,
    pub create_files: Option<bool>,
    pub allow_multiple: Option<bool>,
    pub deny_hidden_paths: Option<bool>,
    pub total_timeout_ms: u64,
    pub permission_timeout_ms: u64,
}

impl AcpToolPolicy {
    pub fn to_json(self, descriptor: &AcpToolDescriptor) -> serde_json::Value {
        let mut policy = json!({
            "scope_basis": self.scope_basis,
            "role_basis": self.role_basis,
            "allowed_roots_basis": self.allowed_roots_basis,
            "path_containment": self.path_containment,
            "risk": descriptor.risk,
            "approval_required": self.approval_required,
            "sensitive_path_policy": self.sensitive_path_policy,
            "canonical_tool": descriptor.canonical_name,
            "provider_tool": descriptor.provider_name,
            "adapter_method": descriptor.adapter_method,
            "client_method": descriptor.client_method,
            "tool_timeout_ms": self.total_timeout_ms,
            "total_timeout_ms": self.total_timeout_ms,
            "permission_timeout_ms": self.permission_timeout_ms,
        });
        if let Some(max_lines) = self.max_lines {
            policy["max_lines"] = json!(max_lines);
        }
        if let Some(max_entries) = self.max_entries {
            policy["max_entries"] = json!(max_entries);
        }
        if let Some(max_results) = self.max_results {
            policy["max_results"] = json!(max_results);
        }
        if let Some(max_bytes) = self.max_bytes {
            policy["max_bytes"] = json!(max_bytes);
        }
        if let Some(recursive_default) = self.recursive_default {
            policy["recursive_default"] = json!(recursive_default);
        }
        if let Some(include_hidden_default) = self.include_hidden_default {
            policy["include_hidden_default"] = json!(include_hidden_default);
        }
        if let Some(max_replacements) = self.max_replacements {
            policy["max_replacements"] = json!(max_replacements);
        }
        if let Some(create_files) = self.create_files {
            policy["create_files"] = json!(create_files);
        }
        if let Some(allow_multiple) = self.allow_multiple {
            policy["allow_multiple"] = json!(allow_multiple);
        }
        if let Some(deny_hidden_paths) = self.deny_hidden_paths {
            policy["deny_hidden_paths"] = json!(deny_hidden_paths);
        }
        policy
    }
}

pub const ACP_READ_TEXT_FILE_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_read_text_file",
    canonical_name: "acp.fs.read_text_file",
    adapter_method: "bears/read_text_file",
    client_method: "fs/read_text_file",
    title: "Read file",
    kind: "read",
    risk: "read_only",
};

pub const ACP_LIST_DIRECTORY_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_list_directory",
    canonical_name: "acp.fs.list_directory",
    adapter_method: "bears/list_directory",
    client_method: "fs/list_directory",
    title: "List directory",
    kind: "read",
    risk: "read_only",
};

pub const ACP_FIND_PATHS_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_find_paths",
    canonical_name: "acp.fs.find_paths",
    adapter_method: "bears/find_paths",
    client_method: "fs/find_paths",
    title: "Find paths",
    kind: "search",
    risk: "read_only",
};

pub const ACP_SEARCH_FILES_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_search_files",
    canonical_name: "acp.fs.search_files",
    adapter_method: "bears/search_files",
    client_method: "fs/search_files",
    title: "Search files",
    kind: "search",
    risk: "read_only",
};

pub const ACP_STAT_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_stat",
    canonical_name: "acp.fs.stat",
    adapter_method: "bears/stat",
    client_method: "fs/stat",
    title: "Stat path",
    kind: "read",
    risk: "read_only",
};

pub const ACP_REPLACE_TEXT_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_replace_text",
    canonical_name: "acp.fs.replace_text",
    adapter_method: "bears/replace_text",
    client_method: "fs/replace_text",
    title: "Replace text",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_CREATE_TEXT_FILE_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_create_text_file",
    canonical_name: "acp.fs.create_text_file",
    adapter_method: "bears/create_text_file",
    client_method: "fs/create_text_file",
    title: "Create text file",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_CREATE_DIRECTORY_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_create_directory",
    canonical_name: "acp.fs.create_directory",
    adapter_method: "bears/create_directory",
    client_method: "fs/create_directory",
    title: "Create directory",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_MOVE_PATH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_move_path",
    canonical_name: "acp.fs.move_path",
    adapter_method: "bears/move_path",
    client_method: "fs/move_path",
    title: "Move path",
    kind: "move",
    risk: "writes_workspace",
};

pub const ACP_COPY_PATH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_copy_path",
    canonical_name: "acp.fs.copy_path",
    adapter_method: "bears/copy_path",
    client_method: "fs/copy_path",
    title: "Copy path",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_APPLY_PATCH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_apply_patch",
    canonical_name: "acp.fs.apply_patch",
    adapter_method: "bears/apply_patch",
    client_method: "fs/apply_patch",
    title: "Apply patch",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_DELETE_PATH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_delete_path",
    canonical_name: "acp.fs.delete_path",
    adapter_method: "bears/delete_path",
    client_method: "fs/delete_path",
    title: "Delete path",
    kind: "delete",
    risk: "deletes_workspace",
};

pub const ACP_GIT_STATUS_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_status",
    canonical_name: "acp.git.status",
    adapter_method: "bears/git_status",
    client_method: "git/status",
    title: "Git status",
    kind: "read",
    risk: "read_only",
};

pub const ACP_GIT_DIFF_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_diff",
    canonical_name: "acp.git.diff",
    adapter_method: "bears/git_diff",
    client_method: "git/diff",
    title: "Git diff",
    kind: "read",
    risk: "read_only",
};

pub const ACP_GIT_LOG_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_log",
    canonical_name: "acp.git.log",
    adapter_method: "bears/git_log",
    client_method: "git/log",
    title: "Git log",
    kind: "read",
    risk: "read_only",
};

pub const ACP_GIT_SHOW_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_show",
    canonical_name: "acp.git.show",
    adapter_method: "bears/git_show",
    client_method: "git/show",
    title: "Git show",
    kind: "read",
    risk: "read_only",
};

pub const ACP_GIT_ADD_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_add",
    canonical_name: "acp.git.add",
    adapter_method: "bears/git_add",
    client_method: "git/add",
    title: "Git add",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_GIT_RESTORE_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_restore",
    canonical_name: "acp.git.restore",
    adapter_method: "bears/git_restore",
    client_method: "git/restore",
    title: "Git restore",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_GIT_COMMIT_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_commit",
    canonical_name: "acp.git.commit",
    adapter_method: "bears/git_commit",
    client_method: "git/commit",
    title: "Git commit",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_GIT_STASH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "git_stash",
    canonical_name: "acp.git.stash",
    adapter_method: "bears/git_stash",
    client_method: "git/stash",
    title: "Git stash",
    kind: "edit",
    risk: "writes_workspace",
};

pub const ACP_PROCESS_RUN_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "process_run",
    canonical_name: "acp.process.run",
    adapter_method: "bears/process_run",
    client_method: "process/run",
    title: "Run process",
    kind: "execute",
    risk: "executes_process",
};

pub const ACP_CHROME_OPEN_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "chrome_open",
    canonical_name: "acp.chrome.open",
    adapter_method: "bears/chrome_open",
    client_method: "chrome/open",
    title: "Chrome open",
    kind: "fetch",
    risk: "browser_access",
};
pub const ACP_CHROME_SNAPSHOT_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "chrome_snapshot",
    canonical_name: "acp.chrome.snapshot",
    adapter_method: "bears/chrome_snapshot",
    client_method: "chrome/snapshot",
    title: "Chrome snapshot",
    kind: "read",
    risk: "browser_access",
};
pub const ACP_CHROME_CONSOLE_MESSAGES_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "chrome_console_messages",
    canonical_name: "acp.chrome.console_messages",
    adapter_method: "bears/chrome_console_messages",
    client_method: "chrome/console_messages",
    title: "Chrome console messages",
    kind: "read",
    risk: "browser_access",
};
pub const ACP_CHROME_NETWORK_REQUESTS_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "chrome_network_requests",
    canonical_name: "acp.chrome.network_requests",
    adapter_method: "bears/chrome_network_requests",
    client_method: "chrome/network_requests",
    title: "Chrome network requests",
    kind: "read",
    risk: "browser_access",
};
pub const ACP_CHROME_SCREENSHOT_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "chrome_screenshot",
    canonical_name: "acp.chrome.screenshot",
    adapter_method: "bears/chrome_screenshot",
    client_method: "chrome/screenshot",
    title: "Chrome screenshot",
    kind: "read",
    risk: "browser_access",
};

pub fn provider_tool_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

const ACP_READ_TEXT_FILE_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: Some(2_000),
    max_entries: None,
    max_results: None,
    max_bytes: None,
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_LIST_DIRECTORY_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: Some(1_000),
    max_results: None,
    max_bytes: None,
    recursive_default: Some(false),
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_FIND_PATHS_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: Some(500),
    max_bytes: None,
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_SEARCH_FILES_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: Some(200),
    max_bytes: Some(1_048_576),
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 180_000,
    permission_timeout_ms: 120_000,
};

const ACP_STAT_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: None,
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_REPLACE_TEXT_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: Some(1_048_576),
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: Some(1),
    create_files: Some(false),
    allow_multiple: Some(false),
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_CREATE_TEXT_FILE_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: Some(1_048_576),
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: Some(true),
    allow_multiple: None,
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_CREATE_DIRECTORY_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: None,
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: Some(true),
    allow_multiple: None,
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_MOVE_PATH_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: None,
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_COPY_PATH_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: Some(1_000),
    max_results: None,
    max_bytes: Some(5_242_880),
    recursive_default: Some(false),
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_APPLY_PATCH_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: Some(100),
    max_results: None,
    max_bytes: Some(1_048_576),
    recursive_default: None,
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: Some(true),
    allow_multiple: Some(true),
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_DELETE_PATH_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "deny_sensitive_paths",
    max_lines: None,
    max_entries: Some(100),
    max_results: None,
    max_bytes: None,
    recursive_default: Some(false),
    include_hidden_default: Some(false),
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: Some(true),
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_GIT_STATUS_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: Some(500),
    max_bytes: Some(262_144),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_GIT_DIFF_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: Some(262_144),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_GIT_LOG_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: Some(100),
    max_bytes: Some(262_144),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_GIT_SHOW_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: Some(262_144),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_GIT_WRITE_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: Some(100),
    max_results: None,
    max_bytes: Some(262_144),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 150_000,
    permission_timeout_ms: 120_000,
};

const ACP_PROCESS_RUN_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "acp_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_cwd_under_allowed_roots",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: None,
    max_results: None,
    max_bytes: Some(65_536),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 120_000,
    permission_timeout_ms: 120_000,
};

const ACP_CHROME_POLICY: AcpToolPolicy = AcpToolPolicy {
    scope_basis: "acp:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "chrome_cdp_endpoint",
    path_containment: "adapter_enforced_chrome_cdp_endpoint",
    approval_required: true,
    sensitive_path_policy: "client_permission_required",
    max_lines: None,
    max_entries: Some(500),
    max_results: None,
    max_bytes: Some(1_048_576),
    recursive_default: None,
    include_hidden_default: None,
    max_replacements: None,
    create_files: None,
    allow_multiple: None,
    deny_hidden_paths: None,
    total_timeout_ms: 120_000,
    permission_timeout_ms: 120_000,
};

pub fn acp_tool_policy(tool: AcpToolName) -> AcpToolPolicy {
    match tool {
        AcpToolName::ReadTextFile => ACP_READ_TEXT_FILE_POLICY,
        AcpToolName::ListDirectory => ACP_LIST_DIRECTORY_POLICY,
        AcpToolName::FindPaths => ACP_FIND_PATHS_POLICY,
        AcpToolName::SearchFiles => ACP_SEARCH_FILES_POLICY,
        AcpToolName::Stat => ACP_STAT_POLICY,
        AcpToolName::ReplaceText => ACP_REPLACE_TEXT_POLICY,
        AcpToolName::CreateTextFile => ACP_CREATE_TEXT_FILE_POLICY,
        AcpToolName::CreateDirectory => ACP_CREATE_DIRECTORY_POLICY,
        AcpToolName::MovePath => ACP_MOVE_PATH_POLICY,
        AcpToolName::CopyPath => ACP_COPY_PATH_POLICY,
        AcpToolName::ApplyPatch => ACP_APPLY_PATCH_POLICY,
        AcpToolName::DeletePath => ACP_DELETE_PATH_POLICY,
        AcpToolName::GitStatus => ACP_GIT_STATUS_POLICY,
        AcpToolName::GitDiff => ACP_GIT_DIFF_POLICY,
        AcpToolName::GitLog => ACP_GIT_LOG_POLICY,
        AcpToolName::GitShow => ACP_GIT_SHOW_POLICY,
        AcpToolName::GitAdd => ACP_GIT_WRITE_POLICY,
        AcpToolName::GitRestore => ACP_GIT_WRITE_POLICY,
        AcpToolName::GitCommit => ACP_GIT_WRITE_POLICY,
        AcpToolName::GitStash => ACP_GIT_WRITE_POLICY,
        AcpToolName::ProcessRun => ACP_PROCESS_RUN_POLICY,
        AcpToolName::ChromeOpen
        | AcpToolName::ChromeSnapshot
        | AcpToolName::ChromeConsoleMessages
        | AcpToolName::ChromeNetworkRequests
        | AcpToolName::ChromeScreenshot => ACP_CHROME_POLICY,
    }
}

pub fn acp_tool_policy_json_for_provider(tool_name: &str) -> serde_json::Value {
    let Some(tool) = AcpToolName::from_provider_alias(tool_name) else {
        return json!({
            "scope_basis": "acp:tools",
            "risk": "read_only",
            "approval_required": true,
            "sensitive_path_policy": "client_permission_required",
        });
    };
    acp_tool_policy(tool).to_json(tool.descriptor())
}

pub fn supported_provider_tool_names() -> Vec<&'static str> {
    AcpToolName::all()
        .iter()
        .map(|tool| tool.descriptor().provider_name)
        .collect()
}

pub fn acp_client_tool_descriptors() -> serde_json::Value {
    json!(AcpToolName::all()
        .iter()
        .map(|tool| acp_client_tool_descriptor(tool.descriptor()))
        .collect::<Vec<_>>())
}

pub fn acp_client_tool_descriptors_for_client_context(
    client_context: &serde_json::Value,
) -> serde_json::Value {
    let names = acp_provider_tool_names_for_client_context(client_context);
    let descriptors = names
        .iter()
        .filter_map(|name| AcpToolName::from_provider_alias(name))
        .map(|tool| acp_client_tool_descriptor(tool.descriptor()))
        .collect::<Vec<_>>();
    if names == vec![ACP_READ_TEXT_FILE_TOOL.provider_name]
        && !adapter_supports_tool(client_context, ACP_READ_TEXT_FILE_TOOL.provider_name)
    {
        tracing::info!(
            phase = acp_diag_phase::DESCRIPTOR_ADVERTISED,
            tools = ?names,
            "ACP adapter did not advertise direct tools; falling back to read-text descriptor only"
        );
    } else {
        tracing::info!(
            phase = acp_diag_phase::DESCRIPTOR_ADVERTISED,
            tools = ?names,
            "ACP client tool descriptors advertised"
        );
    }
    json!(descriptors)
}

pub fn acp_provider_tool_names_for_client_context(
    client_context: &serde_json::Value,
) -> Vec<&'static str> {
    let names = AcpToolName::all()
        .iter()
        .filter(|tool| adapter_supports_tool(client_context, tool.descriptor().provider_name))
        .map(|tool| tool.descriptor().provider_name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        vec![ACP_READ_TEXT_FILE_TOOL.provider_name]
    } else {
        names
    }
}

fn adapter_supports_tool(client_context: &serde_json::Value, provider_name: &str) -> bool {
    client_context
        .pointer(&format!("/adapter/direct_tools/{provider_name}/supported"))
        .and_then(|v| v.as_bool())
        .or_else(|| {
            client_context
                .pointer(&format!("/direct_tools/{provider_name}"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
}

pub fn acp_read_text_file_client_tool_descriptor() -> serde_json::Value {
    acp_client_tool_descriptor(&ACP_READ_TEXT_FILE_TOOL)
}

fn chrome_descriptor(
    tool: &AcpToolDescriptor,
    properties: serde_json::Value,
    required: Vec<&str>,
) -> serde_json::Value {
    json!({
        "name": tool.provider_name,
        "description": format!(
            "ACP Chrome DevTools tool ({}, adapter={}, kind={}, risk={}). Requires BEARS_CHROME_CDP_URL or BEARS_BROWSER_CDP_URL pointing to a Chrome/Chromium/Edge CDP endpoint.",
            tool.canonical_name, tool.adapter_method, tool.kind, tool.risk,
        ),
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}

pub fn acp_client_tool_descriptor(tool: &AcpToolDescriptor) -> serde_json::Value {
    debug_assert!(provider_tool_name_is_safe(tool.provider_name));
    match tool.provider_name {
        "fs_read_text_file" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Reads a UTF-8 text file from the user's editor workspace through the local adapter. Use only for user workspace files, not server files.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local file path under the workspace." },
                    "line": { "type": "integer", "minimum": 1, "description": "Optional 1-based starting line." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Optional maximum number of lines." }
                },
                "required": ["path"]
            }
        }),
        "fs_list_directory" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Lists entries in a workspace directory through the local adapter. Use this before reading files when you need to discover paths.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local directory path under the workspace." },
                    "recursive": { "type": "boolean", "default": false, "description": "Whether to list recursively. Defaults to false." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum entries to return." },
                    "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." }
                },
                "required": ["path"]
            }
        }),
        "fs_find_paths" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Finds workspace paths matching a glob pattern through the local adapter with bounded results.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "root": { "type": "string", "description": "Optional absolute directory path under the workspace. Defaults to the workspace root." },
                    "glob": { "type": "string", "description": "Glob pattern to match against relative paths, such as **/*.rs or package.json." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Maximum paths to return." },
                    "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." }
                },
                "required": ["glob"]
            }
        }),
        "fs_search_files" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Searches UTF-8 text files under a workspace path through the local adapter with bounded results and bytes. For filename/path discovery, set pattern (for example *notes*) and omit query or set query to an empty string.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local file or directory path under the workspace." },
                    "query": { "type": "string", "description": "Optional literal text to search for inside files. If omitted or empty, pattern is used for filename/path discovery only." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Maximum matches to return." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Maximum total bytes to scan." },
                    "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." },
                    "case_sensitive": { "type": "boolean", "default": true, "description": "Whether literal matching is case-sensitive. Defaults to true." },
                    "pattern": { "type": "string", "description": "Optional simple wildcard pattern matched against relative file paths. Supports `*` and `?`." },
                    "extensions": { "type": "array", "items": { "type": "string" }, "maxItems": 10, "description": "Optional list of file extensions to include, such as [\"rs\", \"ts\"]." }
                },
                "required": ["path"]
            }
        }),
        "fs_stat" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Returns metadata for a workspace file or directory without reading file contents.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local path under the workspace." },
                    "include_symlink_target": { "type": "boolean", "default": false, "description": "Include symlink target when the path is a symlink." }
                },
                "required": ["path"]
            }
        }),
        "fs_replace_text" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Replaces exact UTF-8 text in an existing workspace file through the local adapter. Approval is required and sensitive paths are denied.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local file path under the workspace." },
                    "old_text": { "type": "string", "description": "Exact text to replace. Must occur exactly once by default." },
                    "new_text": { "type": "string", "description": "Replacement text." },
                    "expected_replacements": { "type": "integer", "minimum": 1, "maximum": 1, "description": "Expected replacement count. Currently only 1 is allowed." },
                    "allow_multiple": { "type": "boolean", "default": false, "description": "Reserved for future use; currently must be false." },
                    "create_if_missing": { "type": "boolean", "default": false, "description": "Reserved for future use; currently must be false." }
                },
                "required": ["path", "old_text", "new_text"]
            }
        }),
        "fs_create_text_file" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Creates a new UTF-8 text file in the workspace through the local adapter. Approval is required; overwrite is disabled by default and sensitive paths are denied.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local file path under the workspace." },
                    "content": { "type": "string", "description": "UTF-8 text content for the new file." },
                    "create_parent_dirs": { "type": "boolean", "default": false, "description": "Create parent directories if needed. Defaults to false." },
                    "overwrite": { "type": "boolean", "default": false, "description": "Reserved for future use; currently must be false." }
                },
                "required": ["path", "content"]
            }
        }),
        "fs_create_directory" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Creates a directory in the workspace through the local adapter. Approval is required; sensitive and hidden paths are denied by policy.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local directory path under the workspace." },
                    "parents": { "type": "boolean", "default": false, "description": "Create parent directories if needed. Defaults to false." },
                    "allow_existing": { "type": "boolean", "default": false, "description": "Treat an existing directory as success. Defaults to false." }
                },
                "required": ["path"]
            }
        }),
        "fs_move_path" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Moves or renames a workspace file or directory through the local adapter. Approval is required; sensitive and hidden paths are denied by policy.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "source_path": { "type": "string", "description": "Absolute local source path under the workspace." },
                    "destination_path": { "type": "string", "description": "Absolute local destination path under the workspace." },
                    "overwrite": { "type": "boolean", "default": false, "description": "Overwrite destination when it already exists. Defaults to false." },
                    "expected_kind": { "type": "string", "enum": ["file", "directory", "any"], "description": "Optional expected source path kind. Defaults to any." }
                },
                "required": ["source_path", "destination_path"]
            }
        }),
        "fs_copy_path" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Copies a workspace file or directory through the local adapter. Approval is required; sensitive and hidden paths are denied by policy.",
                tool.canonical_name, "acp_client", tool.adapter_method, tool.client_method, tool.kind, tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "source_path": { "type": "string", "description": "Absolute local source path under the workspace." },
                    "destination_path": { "type": "string", "description": "Absolute local destination path under the workspace." },
                    "overwrite": { "type": "boolean", "default": false },
                    "recursive": { "type": "boolean", "default": false },
                    "expected_kind": { "type": "string", "enum": ["file", "directory", "any"] }
                },
                "required": ["source_path", "destination_path"]
            }
        }),
        "fs_apply_patch" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Applies a simple unified diff patch to workspace text files through the local adapter. This is not a fuzzy patch engine: provide full intended file content via context and added lines for each affected file. Approval is required; sensitive and hidden paths are denied by policy.",
                tool.canonical_name, "acp_client", tool.adapter_method, tool.client_method, tool.kind, tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "patch": { "type": "string", "description": "Unified diff patch." },
                    "base_path": { "type": "string", "description": "Optional absolute workspace directory path used to resolve relative patch paths." },
                    "dry_run": { "type": "boolean", "default": false },
                    "allow_create": { "type": "boolean", "default": true },
                    "allow_delete": { "type": "boolean", "default": false }
                },
                "required": ["patch"]
            }
        }),
        "fs_delete_path" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Deletes an existing workspace file or directory through the local adapter. Approval is required; sensitive paths and workspace roots are denied.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute local file or directory path under the workspace." },
                    "recursive": { "type": "boolean", "default": false, "description": "Required to delete non-empty directories." },
                    "expected_kind": { "type": "string", "enum": ["file", "directory", "any"], "description": "Optional expected path kind. Defaults to any." },
                    "allow_missing": { "type": "boolean", "default": false, "description": "If true, a missing path is treated as success." }
                },
                "required": ["path"]
            }
        }),
        "git_status" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Returns git status for a workspace repository through the local adapter.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Optional absolute path under the workspace. Defaults to the workspace root." },
                    "include_untracked": { "type": "boolean", "default": true, "description": "Include untracked files. Defaults to true." }
                },
                "required": []
            }
        }),
        "git_diff" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Returns a bounded git diff for a workspace repository through the local adapter.",
                tool.canonical_name,
                "acp_client",
                tool.adapter_method,
                tool.client_method,
                tool.kind,
                tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Optional absolute path under the workspace. Defaults to the workspace root." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Optional paths under the repository to limit the diff." },
                    "staged": { "type": "boolean", "default": false, "description": "Return staged diff instead of unstaged working-tree diff." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144, "description": "Maximum diff bytes to return." }
                },
                "required": []
            }
        }),
        "git_log" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Returns a bounded git commit log for a workspace repository through the local adapter.",
                tool.canonical_name, "acp_client", tool.adapter_method, tool.client_method, tool.kind, tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Optional absolute path under the workspace. Defaults to the workspace root." },
                    "max_count": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum commits to return." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Optional paths under the repository to limit the log." }
                },
                "required": []
            }
        }),
        "git_show" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Shows a bounded git revision or file at revision for a workspace repository through the local adapter.",
                tool.canonical_name, "acp_client", tool.adapter_method, tool.client_method, tool.kind, tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Optional absolute path under the workspace. Defaults to the workspace root." },
                    "revision": { "type": "string", "description": "Git revision, such as HEAD or a commit SHA." },
                    "path": { "type": "string", "description": "Optional path under the repository to show at the revision." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144, "description": "Maximum output bytes to return." }
                },
                "required": ["revision"]
            }
        }),
        "git_add" => json!({
            "name": tool.provider_name,
            "description": "Stages explicit workspace repository paths with git add. Approval is required.",
            "parameters": { "type": "object", "properties": {
                "repo_path": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            }, "required": ["paths"] }
        }),
        "git_restore" => json!({
            "name": tool.provider_name,
            "description": "Restores explicit workspace repository paths with git restore. Approval is required because this can discard worktree or staged changes.",
            "parameters": { "type": "object", "properties": {
                "repo_path": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "staged": { "type": "boolean", "default": false },
                "worktree": { "type": "boolean", "default": true },
                "source": { "type": "string" }
            }, "required": ["paths"] }
        }),
        "git_commit" => json!({
            "name": tool.provider_name,
            "description": "Creates a git commit from already staged changes. Approval is required.",
            "parameters": { "type": "object", "properties": {
                "repo_path": { "type": "string" },
                "message": { "type": "string" },
                "allow_empty": { "type": "boolean", "default": false }
            }, "required": ["message"] }
        }),
        "git_stash" => json!({
            "name": tool.provider_name,
            "description": "Creates a git stash for workspace repository changes. Approval is required.",
            "parameters": { "type": "object", "properties": {
                "repo_path": { "type": "string" },
                "message": { "type": "string" },
                "include_untracked": { "type": "boolean", "default": false }
            }, "required": [] }
        }),
        "process_run" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Runs a bounded non-interactive process in an explicit workspace cwd through the local adapter. Approval is required.",
                tool.canonical_name, "acp_client", tool.adapter_method, tool.client_method, tool.kind, tool.risk,
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Executable name or absolute executable path. Shell strings are not accepted." },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Command arguments." },
                    "cwd": { "type": "string", "description": "Absolute working directory under the workspace." },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 120000 },
                    "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 65536 },
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Optional non-secret environment values." }
                },
                "required": ["command", "cwd"]
            }
        }),
        "chrome_open" => {
            chrome_descriptor(tool, json!({ "url": { "type": "string" } }), vec!["url"])
        }
        "chrome_snapshot" => chrome_descriptor(tool, json!({}), Vec::<&str>::new()),
        "chrome_console_messages" => chrome_descriptor(
            tool,
            json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 500 } }),
            Vec::<&str>::new(),
        ),
        "chrome_network_requests" => chrome_descriptor(
            tool,
            json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 500 } }),
            Vec::<&str>::new(),
        ),
        "chrome_screenshot" => chrome_descriptor(
            tool,
            json!({ "format": { "type": "string", "enum": ["png", "jpeg"] } }),
            Vec::<&str>::new(),
        ),
        _ => unreachable!("unknown ACP tool descriptor: {}", tool.provider_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_are_safe() {
        for tool in AcpToolName::all() {
            assert!(provider_tool_name_is_safe(tool.descriptor().provider_name));
        }
        assert!(!provider_tool_name_is_safe("fs.read_text_file"));
        assert!(!provider_tool_name_is_safe("fs/read_text_file"));
    }

    #[test]
    fn descriptors_use_provider_name_only() {
        let descriptors = acp_client_tool_descriptors();
        let descriptors = descriptors.as_array().expect("descriptor array");
        assert_eq!(descriptors.len(), AcpToolName::all().len());
        for descriptor in descriptors {
            let name = descriptor["name"].as_str().expect("descriptor name");
            assert!(provider_tool_name_is_safe(name));
            let tool = AcpToolName::from_provider_alias(name).expect("known provider name");
            assert_eq!(name, tool.descriptor().provider_name);
            assert_ne!(name, tool.descriptor().canonical_name);
            assert_ne!(name, tool.descriptor().client_method);
        }
        let serialized = serde_json::to_string(&descriptors).expect("serialize descriptors");
        assert!(!serialized.contains("\"name\":\"fs.read_text_file\""));
        assert!(!serialized.contains("\"name\":\"fs/read_text_file\""));
    }

    #[test]
    fn descriptors_filter_by_adapter_direct_tools() {
        let descriptors = acp_client_tool_descriptors_for_client_context(&json!({
            "direct_tools": {
                "fs_read_text_file": true,
                "fs_list_directory": true,
                "fs_find_paths": true,
                "fs_search_files": true,
                "fs_stat": true,
                "git_status": true,
                "git_diff": true,
                "fs_delete_path": true
            }
        }));
        let names = descriptors
            .as_array()
            .expect("descriptor array")
            .iter()
            .map(|descriptor| descriptor["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"fs_list_directory"));
        assert!(names.contains(&"fs_find_paths"));
        assert!(names.contains(&"fs_search_files"));
        assert!(names.contains(&"fs_stat"));
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_diff"));
        assert!(names.contains(&"fs_delete_path"));
        assert!(!names.contains(&"fs_replace_text"));
    }

    #[test]
    fn descriptors_filter_by_structured_adapter_capabilities() {
        let descriptors = acp_client_tool_descriptors_for_client_context(&json!({
            "adapter": {
                "name": "bears-acp-adapter",
                "version": "0.1.0",
                "direct_tools": {
                    "fs_read_text_file": { "supported": true, "version": 1 },
                    "fs_find_paths": { "supported": true, "version": 1 },
                    "fs_stat": { "supported": true, "version": 1 },
                    "git_status": { "supported": true, "version": 1 },
                    "git_diff": { "supported": true, "version": 1 },
                    "git_log": { "supported": true, "version": 1 },
                    "git_show": { "supported": true, "version": 1 },
                    "fs_replace_text": { "supported": true, "version": 1 },
                    "fs_create_text_file": { "supported": true, "version": 1 },
                    "fs_create_directory": { "supported": true, "version": 1 },
                    "fs_move_path": { "supported": true, "version": 1 },
                    "fs_delete_path": { "supported": true, "version": 1 }
                }
            }
        }));
        let names = descriptors
            .as_array()
            .expect("descriptor array")
            .iter()
            .map(|descriptor| descriptor["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "fs_read_text_file",
                "fs_find_paths",
                "fs_stat",
                "fs_replace_text",
                "fs_create_text_file",
                "fs_create_directory",
                "fs_move_path",
                "fs_delete_path",
                "git_status",
                "git_diff",
                "git_log",
                "git_show"
            ]
        );
    }

    #[test]
    fn missing_direct_tools_defaults_to_read_text_only() {
        let descriptors = acp_client_tool_descriptors_for_client_context(&json!({}));
        let names = descriptors
            .as_array()
            .expect("descriptor array")
            .iter()
            .map(|descriptor| descriptor["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["fs_read_text_file"]);
    }

    #[test]
    fn read_text_file_descriptor_wrapper_still_works() {
        let descriptor = acp_read_text_file_client_tool_descriptor();
        assert_eq!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.provider_name);
    }

    #[test]
    fn tool_policy_includes_authoritative_limits_and_scope() {
        let list_policy = acp_tool_policy_json_for_provider("fs_list_directory");
        assert_eq!(list_policy["scope_basis"], "acp:tools");
        assert_eq!(list_policy["role_basis"], "pair_agent");
        assert_eq!(
            list_policy["allowed_roots_basis"],
            "acp_session.workspace_roots"
        );
        assert_eq!(list_policy["max_entries"], 1000);
        assert_eq!(list_policy["include_hidden_default"], false);

        let find_policy = acp_tool_policy_json_for_provider("fs_find_paths");
        assert_eq!(find_policy["max_results"], 500);
        assert_eq!(find_policy["include_hidden_default"], false);

        let search_policy = acp_tool_policy_json_for_provider("fs_search_files");
        assert_eq!(search_policy["max_results"], 200);
        assert_eq!(search_policy["max_bytes"], 1_048_576);
        assert_eq!(search_policy["approval_required"], true);

        let stat_policy = acp_tool_policy_json_for_provider("fs_stat");
        assert_eq!(stat_policy["risk"], "read_only");
        assert_eq!(stat_policy["approval_required"], true);

        let git_status_policy = acp_tool_policy_json_for_provider("git_status");
        assert_eq!(git_status_policy["risk"], "read_only");
        assert_eq!(git_status_policy["max_results"], 500);
        assert_eq!(git_status_policy["max_bytes"], 262_144);

        let git_diff_policy = acp_tool_policy_json_for_provider("git_diff");
        assert_eq!(git_diff_policy["risk"], "read_only");
        assert_eq!(git_diff_policy["max_bytes"], 262_144);

        let git_log_policy = acp_tool_policy_json_for_provider("git_log");
        assert_eq!(git_log_policy["risk"], "read_only");
        assert_eq!(git_log_policy["max_results"], 100);
        assert_eq!(git_log_policy["max_bytes"], 262_144);

        let git_show_policy = acp_tool_policy_json_for_provider("git_show");
        assert_eq!(git_show_policy["risk"], "read_only");
        assert_eq!(git_show_policy["max_bytes"], 262_144);

        let replace_policy = acp_tool_policy_json_for_provider("fs_replace_text");
        assert_eq!(replace_policy["risk"], "writes_workspace");
        assert_eq!(
            replace_policy["sensitive_path_policy"],
            "deny_sensitive_paths"
        );
        assert_eq!(replace_policy["max_replacements"], 1);
        assert_eq!(replace_policy["create_files"], false);
        assert_eq!(replace_policy["allow_multiple"], false);
        assert_eq!(replace_policy["deny_hidden_paths"], true);
        assert!(replace_policy.get("max_results").is_none());
        assert_eq!(replace_policy["approval_required"], true);

        let create_policy = acp_tool_policy_json_for_provider("fs_create_text_file");
        assert_eq!(create_policy["risk"], "writes_workspace");
        assert_eq!(create_policy["create_files"], true);
        assert_eq!(create_policy["max_bytes"], 1_048_576);

        let create_directory_policy = acp_tool_policy_json_for_provider("fs_create_directory");
        assert_eq!(create_directory_policy["risk"], "writes_workspace");
        assert_eq!(create_directory_policy["create_files"], true);
        assert_eq!(create_directory_policy["deny_hidden_paths"], true);

        let move_policy = acp_tool_policy_json_for_provider("fs_move_path");
        assert_eq!(move_policy["risk"], "writes_workspace");
        assert_eq!(move_policy["deny_hidden_paths"], true);

        let delete_policy = acp_tool_policy_json_for_provider("fs_delete_path");
        assert_eq!(delete_policy["risk"], "deletes_workspace");
        assert_eq!(
            delete_policy["sensitive_path_policy"],
            "deny_sensitive_paths"
        );
        assert_eq!(delete_policy["max_entries"], 100);
        assert_eq!(delete_policy["deny_hidden_paths"], true);
    }

    #[test]
    fn all_advertised_tools_require_approval_and_adapter_path_containment() {
        for tool in AcpToolName::all() {
            let descriptor = tool.descriptor();
            let policy = acp_tool_policy(*tool).to_json(descriptor);
            assert_eq!(
                policy["approval_required"], true,
                "{}",
                descriptor.provider_name
            );
            assert!(
                matches!(
                    policy["path_containment"].as_str(),
                    Some(
                        "adapter_enforced_absolute_path_under_allowed_roots"
                            | "adapter_enforced_absolute_cwd_under_allowed_roots"
                            | "adapter_enforced_url_host_scope"
                            | "adapter_enforced_chrome_cdp_endpoint"
                    )
                ),
                "{}",
                descriptor.provider_name
            );
            assert!(
                matches!(
                    policy["allowed_roots_basis"].as_str(),
                    Some("acp_session.workspace_roots" | "url.host" | "chrome_cdp_endpoint")
                ),
                "{}",
                descriptor.provider_name
            );
            assert!(
                policy["permission_timeout_ms"].as_u64().unwrap()
                    <= policy["total_timeout_ms"].as_u64().unwrap(),
                "permission timeout must fit inside tool timeout for {}",
                descriptor.provider_name
            );
        }
    }

    #[test]
    fn mutating_tools_deny_sensitive_and_hidden_paths_by_policy() {
        for name in [
            "fs_replace_text",
            "fs_create_text_file",
            "fs_create_directory",
            "fs_move_path",
            "fs_delete_path",
        ] {
            let policy = acp_tool_policy_json_for_provider(name);
            assert_eq!(
                policy["sensitive_path_policy"], "deny_sensitive_paths",
                "{name}"
            );
            assert_eq!(policy["deny_hidden_paths"], true, "{name}");
            assert!(
                matches!(
                    policy["risk"].as_str(),
                    Some("writes_workspace" | "deletes_workspace")
                ),
                "{name} must have mutating risk"
            );
        }
    }

    #[test]
    fn milestone_1_descriptor_schemas_are_present() {
        let find = acp_client_tool_descriptor(&ACP_FIND_PATHS_TOOL);
        assert_eq!(find["parameters"]["required"], json!(["glob"]));
        assert!(find["parameters"]["properties"].get("root").is_some());
        assert!(find["parameters"]["properties"]
            .get("include_hidden")
            .is_some());

        let stat = acp_client_tool_descriptor(&ACP_STAT_TOOL);
        assert_eq!(stat["parameters"]["required"], json!(["path"]));
        assert!(stat["parameters"]["properties"]
            .get("include_symlink_target")
            .is_some());

        let create_directory = acp_client_tool_descriptor(&ACP_CREATE_DIRECTORY_TOOL);
        assert_eq!(create_directory["parameters"]["required"], json!(["path"]));
        assert!(create_directory["parameters"]["properties"]
            .get("parents")
            .is_some());
        assert!(create_directory["parameters"]["properties"]
            .get("allow_existing")
            .is_some());

        let move_path = acp_client_tool_descriptor(&ACP_MOVE_PATH_TOOL);
        assert_eq!(
            move_path["parameters"]["required"],
            json!(["source_path", "destination_path"])
        );
        assert!(move_path["parameters"]["properties"]
            .get("overwrite")
            .is_some());
        assert!(move_path["parameters"]["properties"]
            .get("expected_kind")
            .is_some());

        let git_status = acp_client_tool_descriptor(&ACP_GIT_STATUS_TOOL);
        assert_eq!(git_status["parameters"]["required"], json!([]));
        assert!(git_status["parameters"]["properties"]
            .get("repo_path")
            .is_some());

        let git_diff = acp_client_tool_descriptor(&ACP_GIT_DIFF_TOOL);
        assert_eq!(git_diff["parameters"]["required"], json!([]));
        assert!(git_diff["parameters"]["properties"].get("paths").is_some());
        assert!(git_diff["parameters"]["properties"].get("staged").is_some());

        let git_log = acp_client_tool_descriptor(&ACP_GIT_LOG_TOOL);
        assert_eq!(git_log["parameters"]["required"], json!([]));
        assert!(git_log["parameters"]["properties"]
            .get("max_count")
            .is_some());
        assert!(git_log["parameters"]["properties"].get("paths").is_some());

        let git_show = acp_client_tool_descriptor(&ACP_GIT_SHOW_TOOL);
        assert_eq!(git_show["parameters"]["required"], json!(["revision"]));
        assert!(git_show["parameters"]["properties"].get("path").is_some());
        assert!(git_show["parameters"]["properties"]
            .get("max_bytes")
            .is_some());
    }

    #[test]
    fn descriptor_schemas_keep_search_query_optional_for_path_discovery() {
        let descriptor = acp_client_tool_descriptor(&ACP_SEARCH_FILES_TOOL);
        let required = descriptor["parameters"]["required"].as_array().unwrap();
        assert_eq!(required, &vec![json!("path")]);
        assert!(descriptor["parameters"]["properties"]
            .get("pattern")
            .is_some());
        assert!(descriptor["parameters"]["properties"]
            .get("extensions")
            .is_some());
        assert!(descriptor["parameters"]["properties"]
            .get("case_sensitive")
            .is_some());
    }
}
