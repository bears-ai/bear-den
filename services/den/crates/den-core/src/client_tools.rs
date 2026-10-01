use serde_json::json;

use crate::{BearCapability, EffectivePolicy, Governance, TurnExecutionOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    ReadOnly,
    WorkspaceMutation,
    Execution,
    Browser,
}

/// Whether a client tool must be preceded by a runtime checkpoint under a
/// control profile that enables pre-risk enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreRiskCheckpointClass {
    None,
    Broad,
    Destructive,
}

impl ToolClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceMutation => "workspace_mutation",
            Self::Execution => "execution",
            Self::Browser => "browser",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEnablementState {
    ReadOnly,
    AllTools,
}

impl ToolEnablementState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::AllTools => "all_tools",
        }
    }

    pub fn enables_non_read_tools(self) -> bool {
        matches!(self, Self::AllTools)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedSessionPolicy {
    pub mode_label: &'static str,
    pub tool_enablement: ToolEnablementState,
    pub plan_mode_state: Option<String>,
}

/// Compiled authority view for one turn.
///
/// `TurnAuthority` compiles verified turn origin, current supervision, and the
/// Den-owned session mode/plan state for tool advertisement, routing, prompt
/// assembly, and client projection. Rendered labels cannot grant authority.
#[derive(Debug, Clone)]
pub struct TurnAuthority {
    policy: EffectivePolicy,
    pub session_policy: ResolvedSessionPolicy,
}

impl TurnAuthority {
    pub fn for_origin(
        origin: TurnExecutionOrigin,
        governance: Governance,
        current_mode: &str,
        plan_mode_state: Option<&str>,
    ) -> Self {
        Self {
            policy: EffectivePolicy::compile_for_origin(origin, governance),
            session_policy: resolve_session_policy_for_mode(current_mode, plan_mode_state),
        }
    }

    pub fn has_armature_tools(&self) -> bool {
        self.policy
            .capabilities
            .contains(BearCapability::UseArmatureTools)
    }

    pub fn allows_tool(&self, tool: ClientToolName) -> bool {
        self.has_armature_tools() && self.session_policy.allows_tool(tool)
    }

    pub fn mode_label(&self) -> &'static str {
        self.session_policy.mode_label
    }

    pub fn tool_enablement(&self) -> ToolEnablementState {
        self.session_policy.tool_enablement
    }

    pub fn allowed_tool_classes(&self) -> Vec<&'static str> {
        if self.has_armature_tools() {
            self.session_policy.allowed_tool_classes()
        } else {
            Vec::new()
        }
    }

    pub fn denied_tool_classes(&self) -> Vec<&'static str> {
        if self.has_armature_tools() {
            self.session_policy.denied_tool_classes()
        } else {
            vec!["read_only", "workspace_mutation", "execution", "browser"]
        }
    }

    pub fn read_only_runtime_context(&self) -> Option<serde_json::Value> {
        if !self.has_armature_tools() {
            return Some(serde_json::json!({
                "permission_mode": self.mode_label(),
                "tool_enablement": "unavailable",
                "allowed_tool_classes": self.allowed_tool_classes(),
                "denied_tool_classes": self.denied_tool_classes(),
            }));
        }
        if self.tool_enablement().enables_non_read_tools() {
            return None;
        }

        Some(serde_json::json!({
            "permission_mode": self.mode_label(),
            "tool_enablement": "read_only",
            "allowed_tool_classes": self.allowed_tool_classes(),
            "denied_tool_classes": self.denied_tool_classes(),
        }))
    }
}

impl ResolvedSessionPolicy {
    pub fn allows_tool(&self, tool: ClientToolName) -> bool {
        self.tool_enablement.enables_non_read_tools()
            || matches!(tool_class(tool), ToolClass::ReadOnly)
    }

    pub fn allowed_tool_classes(&self) -> Vec<&'static str> {
        if self.tool_enablement.enables_non_read_tools() {
            vec![
                ToolClass::ReadOnly.as_str(),
                ToolClass::WorkspaceMutation.as_str(),
                ToolClass::Execution.as_str(),
                ToolClass::Browser.as_str(),
            ]
        } else {
            vec![ToolClass::ReadOnly.as_str()]
        }
    }

    pub fn denied_tool_classes(&self) -> Vec<&'static str> {
        if self.tool_enablement.enables_non_read_tools() {
            Vec::new()
        } else {
            vec![
                ToolClass::WorkspaceMutation.as_str(),
                ToolClass::Execution.as_str(),
                ToolClass::Browser.as_str(),
            ]
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "mode_label": self.mode_label,
            "tool_enablement": {
                "state": self.tool_enablement.as_str(),
                "source": "den",
                "enables_non_read_tools": self.tool_enablement.enables_non_read_tools(),
            },
            "plan_mode_state": self.plan_mode_state,
            "allowed_tool_classes": self.allowed_tool_classes(),
            "denied_tool_classes": self.denied_tool_classes(),
            "policy_note": "Ask and Plan expose read-only tools. Write enables mutation/execution/browser tools, which still require Den policy and armature client approval."
        })
    }
}

pub fn resolve_session_policy(plan_mode_state: Option<&str>) -> ResolvedSessionPolicy {
    resolve_session_policy_for_mode("ask", plan_mode_state)
}

pub fn resolve_session_policy_for_mode(
    current_mode: &str,
    plan_mode_state: Option<&str>,
) -> ResolvedSessionPolicy {
    let normalized_current_mode = current_mode.trim().to_ascii_lowercase();
    let resolved_mode = match plan_mode_state {
        Some("approved") => "write",
        Some("active" | "submitted") => "plan",
        Some("rejected") => "ask",
        _ => normalized_current_mode.as_str(),
    };
    match resolved_mode {
        "plan" => ResolvedSessionPolicy {
            mode_label: "Plan",
            tool_enablement: ToolEnablementState::ReadOnly,
            plan_mode_state: plan_mode_state.map(str::to_string),
        },
        "write" => ResolvedSessionPolicy {
            mode_label: "Write",
            tool_enablement: ToolEnablementState::AllTools,
            plan_mode_state: plan_mode_state.map(str::to_string),
        },
        _ => ResolvedSessionPolicy {
            mode_label: "Ask",
            tool_enablement: ToolEnablementState::ReadOnly,
            plan_mode_state: plan_mode_state.map(str::to_string),
        },
    }
}

pub mod diag_phase {
    pub const DESCRIPTOR_ADVERTISED: &str = "descriptor_advertised";
    pub const RUNTIME_TOOL_CALL_MAPPED: &str = "runtime_tool_call_mapped";
    pub const TOOL_REQUEST_REGISTERED: &str = "tool_request_registered";
    pub const ADAPTER_PERMISSION_REQUESTED: &str = "adapter_permission_requested";
    pub const ADAPTER_PERMISSION_DENIED: &str = "adapter_permission_denied";
    pub const ADAPTER_EXECUTION_STARTED: &str = "adapter_execution_started";
    pub const ADAPTER_EXECUTION_FAILED: &str = "adapter_execution_failed";
    pub const ADAPTER_RESULT_POSTED: &str = "adapter_result_posted";
    pub const DEN_RESULT_DELIVERED: &str = "den_result_delivered";
    pub const RUNTIME_CONTINUATION_STARTED: &str = "runtime_continuation_started";
    pub const RUNTIME_CONTINUATION_FAILED: &str = "runtime_continuation_failed";
    pub const TOOL_RESULT_TIMEOUT: &str = "tool_result_timeout";
    pub const RECENTLY_SETTLED_RESULT: &str = "recently_settled_result";
}

// Adapter-executed armature local tools. Adding to this enum is not enough:
// the adapter must implement and advertise the same provider_name in
// `adapter.direct_tools`, and Den must keep descriptor advertisement gated by
// `adapter_supports_tool`. Do not bump the Den↔adapter contract for additive
// local tools unless the tool requires an incompatible message-shape change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientToolName {
    ReadTextFile,
    ListDirectory,
    FindPaths,
    SearchFiles,
    Stat,
    EditFile,
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
    RunCommand,
    ProcessRun,
    TerminalRunCommand,
    ChromeOpen,
    ChromeSnapshot,
    ChromeConsoleMessages,
    ChromeNetworkRequests,
    ChromeScreenshot,
    McpCallTool,
}

impl ClientToolName {
    pub fn descriptor(self) -> &'static ClientToolDescriptor {
        match self {
            Self::ReadTextFile => &READ_TEXT_FILE_TOOL,
            Self::ListDirectory => &LIST_DIRECTORY_TOOL,
            Self::FindPaths => &FIND_PATHS_TOOL,
            Self::SearchFiles => &SEARCH_FILES_TOOL,
            Self::Stat => &STAT_TOOL,
            Self::EditFile => &EDIT_FILE_TOOL,
            Self::CreateTextFile => &CREATE_TEXT_FILE_TOOL,
            Self::CreateDirectory => &CREATE_DIRECTORY_TOOL,
            Self::MovePath => &MOVE_PATH_TOOL,
            Self::CopyPath => &COPY_PATH_TOOL,
            Self::ApplyPatch => &APPLY_PATCH_TOOL,
            Self::DeletePath => &DELETE_PATH_TOOL,
            Self::GitStatus => &GIT_STATUS_TOOL,
            Self::GitDiff => &GIT_DIFF_TOOL,
            Self::GitLog => &GIT_LOG_TOOL,
            Self::GitShow => &GIT_SHOW_TOOL,
            Self::GitAdd => &GIT_ADD_TOOL,
            Self::GitRestore => &GIT_RESTORE_TOOL,
            Self::GitCommit => &GIT_COMMIT_TOOL,
            Self::GitStash => &GIT_STASH_TOOL,
            Self::RunCommand => &RUN_COMMAND_TOOL,
            Self::ProcessRun => &PROCESS_RUN_TOOL,
            Self::TerminalRunCommand => &TERMINAL_RUN_COMMAND_TOOL,
            Self::ChromeOpen => &CHROME_OPEN_TOOL,
            Self::ChromeSnapshot => &CHROME_SNAPSHOT_TOOL,
            Self::ChromeConsoleMessages => &CHROME_CONSOLE_MESSAGES_TOOL,
            Self::ChromeNetworkRequests => &CHROME_NETWORK_REQUESTS_TOOL,
            Self::ChromeScreenshot => &CHROME_SCREENSHOT_TOOL,
            Self::McpCallTool => &MCP_CALL_TOOL,
        }
    }

    pub fn all() -> &'static [Self] {
        // Keep this list in sync with `descriptor`, `from_provider_alias`, policy,
        // descriptors, and the adapter's direct tool advertisement. Old adapters
        // remain compatible because `provider_tool_names_for_client_context`
        // filters this list by adapter support.
        &[
            Self::ReadTextFile,
            Self::ListDirectory,
            Self::FindPaths,
            Self::SearchFiles,
            Self::Stat,
            Self::EditFile,
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
            Self::RunCommand,
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
                .is_none_or(|s| !self.allow_empty_required_string(arg) && s.trim().is_empty())
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
            Self::EditFile => &["path", "old_text", "new_text"],
            Self::CreateTextFile => &["path", "content"],
            Self::CreateDirectory | Self::DeletePath => &["path"],
            Self::MovePath | Self::CopyPath => &["source_path", "destination_path"],
            Self::ApplyPatch => &["patch"],
            Self::GitStatus | Self::GitDiff | Self::GitLog => &[],
            Self::GitShow => &["revision"],
            Self::GitAdd | Self::GitRestore => &["paths"],
            Self::GitCommit => &["message"],
            Self::GitStash => &[],
            Self::RunCommand | Self::ProcessRun | Self::TerminalRunCommand => &["command", "cwd"],
            Self::ChromeOpen => &["url"],
            Self::ChromeSnapshot
            | Self::ChromeConsoleMessages
            | Self::ChromeNetworkRequests
            | Self::ChromeScreenshot => &[],
            Self::McpCallTool => &[],
        }
    }

    fn allow_empty_required_string(self, arg: &str) -> bool {
        matches!(self, Self::EditFile | Self::CreateTextFile)
            && matches!(arg, "old_text" | "new_text" | "content")
    }

    pub fn from_provider_alias(raw: &str) -> Option<Self> {
        for tool in Self::all() {
            let descriptor = tool.descriptor();
            if descriptor.provider_name == raw
                || descriptor.canonical_name == raw
                || descriptor.provider_aliases.contains(&raw)
            {
                return Some(*tool);
            }
        }
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
            "bears/edit_file" | "fs/edit_file" | "fs.edit_file" | "fs_edit_file" | "edit_file"
            | "bears/replace_text" | "fs/replace_text" | "fs.replace_text" | "fs_replace_text"
            | "replace_text" => Some(Self::EditFile),
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
            "bears/run_command" | "command/run" | "command.run" | "run_command" => {
                Some(Self::RunCommand)
            }
            "bears/process_run" | "process/run" | "process.run" | "process_run" => {
                Some(Self::RunCommand)
            }
            "bears/terminal_run_command"
            | "terminal/run_command"
            | "terminal.run_command"
            | "terminal_run_command" => Some(Self::RunCommand),
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
            raw if raw.starts_with("mcp__") => Some(Self::McpCallTool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Ok,
    Error,
    Cancelled,
    Timeout,
    PermissionDenied,
    Unsupported,
}

impl ToolStatus {
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
pub struct ClientToolDescriptor {
    pub provider_name: &'static str,
    pub provider_aliases: &'static [&'static str],
    pub canonical_name: &'static str,
    pub title: &'static str,
    pub kind: &'static str,
    pub risk: &'static str,
    pub permission_class: &'static str,
}

// `ToolDisplayDescriptor` now lives in `den-tools` (the descriptor authority);
// re-exported here so existing `crate::core::client_tools::ToolDisplayDescriptor`
// paths keep resolving. See docs/roadmap/DEN_CRATE_SPLIT_PLAN.md (Phase A).
pub use crate::tools::ToolDisplayDescriptor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTargetPolicy {
    ArmatureLocal,
    DenHosted,
}

impl ExecutionTargetPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArmatureLocal => "armature_local",
            Self::DenHosted => "den_hosted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Never,
    Required,
    RequiredForSensitiveTargets,
}

impl ApprovalPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Required => "required",
            Self::RequiredForSensitiveTargets => "required_for_sensitive_targets",
        }
    }

    pub const fn requires_unconditional_approval(self) -> bool {
        matches!(self, Self::Required)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsTargetKindPolicy {
    File,
    Directory,
}

impl FsTargetKindPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitivePathPolicy {
    Deny,
    FilterResults,
    RequireApproval,
    Allow,
}

impl SensitivePathPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny_sensitive_paths",
            Self::FilterResults => "filter_results",
            Self::RequireApproval => "client_permission_required",
            Self::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPolicy {
    WorkspacePath {
        arg: &'static str,
        required_kind: Option<FsTargetKindPolicy>,
    },
    WorkspaceRootOrPath {
        arg: Option<&'static str>,
        default_to_workspace_root: bool,
        required_kind: Option<FsTargetKindPolicy>,
    },
    SourceDestination {
        source_arg: &'static str,
        destination_arg: &'static str,
    },
    Command {
        command_arg: &'static str,
        cwd_arg: &'static str,
    },
    Url {
        arg: &'static str,
    },
    None,
}

impl TargetPolicy {
    pub fn to_json(self) -> serde_json::Value {
        match self {
            Self::WorkspacePath { arg, required_kind } => json!({
                "kind": "workspace_path",
                "arg": arg,
                "required_kind": required_kind.map(FsTargetKindPolicy::as_str),
            }),
            Self::WorkspaceRootOrPath {
                arg,
                default_to_workspace_root,
                required_kind,
            } => json!({
                "kind": "workspace_root_or_path",
                "arg": arg,
                "default_to_workspace_root": default_to_workspace_root,
                "required_kind": required_kind.map(FsTargetKindPolicy::as_str),
            }),
            Self::SourceDestination {
                source_arg,
                destination_arg,
            } => json!({
                "kind": "source_destination",
                "source_arg": source_arg,
                "destination_arg": destination_arg,
            }),
            Self::Command {
                command_arg,
                cwd_arg,
            } => json!({
                "kind": "command",
                "command_arg": command_arg,
                "cwd_arg": cwd_arg,
            }),
            Self::Url { arg } => json!({
                "kind": "url",
                "arg": arg,
            }),
            Self::None => json!({ "kind": "none" }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolPolicy {
    pub scope_basis: &'static str,
    pub role_basis: &'static str,
    pub allowed_roots_basis: &'static str,
    pub path_containment: &'static str,
    pub execution_target: ExecutionTargetPolicy,
    pub approval_policy: ApprovalPolicy,
    pub sensitive_path_policy: SensitivePathPolicy,
    pub target_policy: TargetPolicy,
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

impl ToolPolicy {
    pub fn to_json(self, descriptor: &ClientToolDescriptor) -> serde_json::Value {
        let mut policy = json!({
            "scope_basis": self.scope_basis,
            "role_basis": self.role_basis,
            "allowed_roots_basis": self.allowed_roots_basis,
            "path_containment": self.path_containment,
            "execution_target": self.execution_target.as_str(),
            "risk": descriptor.risk,
            "approval_policy": self.approval_policy.as_str(),
            "approval_required": self.approval_policy.requires_unconditional_approval(),
            "sensitive_path_policy": self.sensitive_path_policy.as_str(),
            "target_policy": self.target_policy.to_json(),
            "canonical_tool": descriptor.canonical_name,
            "provider_tool": descriptor.provider_name,
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

pub const MCP_CALL_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "mcp__dynamic_tool",
    provider_aliases: &[],
    canonical_name: "armature.mcp.call_tool",
    title: "MCP tool",
    kind: "tool",
    risk: "external_tool",
    permission_class: "external_tool",
};

pub const READ_TEXT_FILE_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_read_text_file",
    provider_aliases: &[],
    canonical_name: "armature.fs.read_text_file",
    title: "Read file",
    kind: "read",
    risk: "read_only",
    permission_class: "read_files",
};

pub const LIST_DIRECTORY_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_list_directory",
    provider_aliases: &[],
    canonical_name: "armature.fs.list_directory",
    title: "List directory",
    kind: "read",
    risk: "read_only",
    permission_class: "read_files",
};

pub const FIND_PATHS_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_find_paths",
    provider_aliases: &[],
    canonical_name: "armature.fs.find_paths",
    title: "Find paths",
    kind: "search",
    risk: "read_only",
    permission_class: "read_files",
};

pub const SEARCH_FILES_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_search_files",
    provider_aliases: &[],
    canonical_name: "armature.fs.search_files",
    title: "Search files",
    kind: "search",
    risk: "read_only",
    permission_class: "read_files",
};

pub const STAT_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_stat",
    provider_aliases: &[],
    canonical_name: "armature.fs.stat",
    title: "Stat path",
    kind: "read",
    risk: "read_only",
    permission_class: "read_files",
};

pub const EDIT_FILE_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_edit_file",
    provider_aliases: &[
        "fs_replace_text",
        "replace_text",
        "fs/replace_text",
        "fs.replace_text",
        "bears/replace_text",
    ],
    canonical_name: "armature.fs.edit_file",
    title: "Edit file",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const CREATE_TEXT_FILE_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_create_text_file",
    provider_aliases: &[],
    canonical_name: "armature.fs.create_text_file",
    title: "Create text file",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const CREATE_DIRECTORY_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_create_directory",
    provider_aliases: &[],
    canonical_name: "armature.fs.create_directory",
    title: "Create directory",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const MOVE_PATH_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_move_path",
    provider_aliases: &[],
    canonical_name: "armature.fs.move_path",
    title: "Move path",
    kind: "move",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const COPY_PATH_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_copy_path",
    provider_aliases: &[],
    canonical_name: "armature.fs.copy_path",
    title: "Copy path",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const APPLY_PATCH_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_apply_patch",
    provider_aliases: &[],
    canonical_name: "armature.fs.apply_patch",
    title: "Apply patch",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "edit_files",
};

pub const DELETE_PATH_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "fs_delete_path",
    provider_aliases: &[],
    canonical_name: "armature.fs.delete_path",
    title: "Delete path",
    kind: "delete",
    risk: "deletes_workspace",
    permission_class: "delete_files",
};

pub const GIT_STATUS_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_status",
    provider_aliases: &[],
    canonical_name: "armature.git.status",
    title: "Git status",
    kind: "read",
    risk: "read_only",
    permission_class: "git_read",
};

pub const GIT_DIFF_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_diff",
    provider_aliases: &[],
    canonical_name: "armature.git.diff",
    title: "Git diff",
    kind: "read",
    risk: "read_only",
    permission_class: "git_read",
};

pub const GIT_LOG_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_log",
    provider_aliases: &[],
    canonical_name: "armature.git.log",
    title: "Git log",
    kind: "read",
    risk: "read_only",
    permission_class: "git_read",
};

pub const GIT_SHOW_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_show",
    provider_aliases: &[],
    canonical_name: "armature.git.show",
    title: "Git show",
    kind: "read",
    risk: "read_only",
    permission_class: "git_read",
};

pub const GIT_ADD_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_add",
    provider_aliases: &[],
    canonical_name: "armature.git.add",
    title: "Git add",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "git_write",
};

pub const GIT_RESTORE_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_restore",
    provider_aliases: &[],
    canonical_name: "armature.git.restore",
    title: "Git restore",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "git_write",
};

pub const GIT_COMMIT_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_commit",
    provider_aliases: &[],
    canonical_name: "armature.git.commit",
    title: "Git commit",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "git_write",
};

pub const GIT_STASH_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "git_stash",
    provider_aliases: &[],
    canonical_name: "armature.git.stash",
    title: "Git stash",
    kind: "edit",
    risk: "writes_workspace",
    permission_class: "git_write",
};

pub const RUN_COMMAND_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "run_command",
    provider_aliases: &[
        "process_run",
        "process/run",
        "process.run",
        "bears/process_run",
        "terminal_run_command",
        "terminal/run_command",
        "terminal.run_command",
        "bears/terminal_run_command",
    ],
    canonical_name: "armature.command.run",
    title: "Run command",
    kind: "execute",
    risk: "executes_process",
    permission_class: "command_run",
};

pub const PROCESS_RUN_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "process_run",
    provider_aliases: &[],
    canonical_name: "armature.process.run",
    title: "Run process",
    kind: "execute",
    risk: "executes_process",
    permission_class: "command_run",
};

pub const TERMINAL_RUN_COMMAND_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "terminal_run_command",
    provider_aliases: &[],
    canonical_name: "armature.terminal.run_command",
    title: "Run terminal command",
    kind: "execute",
    risk: "executes_process",
    permission_class: "command_run",
};

pub const CHROME_OPEN_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "chrome_open",
    provider_aliases: &[],
    canonical_name: "armature.chrome.open",
    title: "Open page",
    kind: "read",
    risk: "network_fetch",
    permission_class: "browser",
};
pub const CHROME_SNAPSHOT_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "chrome_snapshot",
    provider_aliases: &[],
    canonical_name: "armature.chrome.snapshot",
    title: "Chrome snapshot",
    kind: "read",
    risk: "network_fetch",
    permission_class: "browser",
};
pub const CHROME_CONSOLE_MESSAGES_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "chrome_console_messages",
    provider_aliases: &[],
    canonical_name: "armature.chrome.console_messages",
    title: "Chrome console messages",
    kind: "read",
    risk: "network_fetch",
    permission_class: "browser",
};
pub const CHROME_NETWORK_REQUESTS_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "chrome_network_requests",
    provider_aliases: &[],
    canonical_name: "armature.chrome.network_requests",
    title: "Chrome network requests",
    kind: "read",
    risk: "network_fetch",
    permission_class: "browser",
};
pub const CHROME_SCREENSHOT_TOOL: ClientToolDescriptor = ClientToolDescriptor {
    provider_name: "chrome_screenshot",
    provider_aliases: &[],
    canonical_name: "armature.chrome.screenshot",
    title: "Chrome screenshot",
    kind: "read",
    risk: "network_fetch",
    permission_class: "browser",
};

pub fn provider_tool_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn object_schema(properties: serde_json::Value, required: Vec<&str>) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn generic_object_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": true,
    })
}

pub fn provider_tool_descriptor(tool: ClientToolName) -> serde_json::Value {
    let descriptor = tool.descriptor();
    debug_assert!(provider_tool_name_is_safe(descriptor.provider_name));
    let parameters = match tool {
        ClientToolName::ReadTextFile => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file path under the user's workspace." },
                "line": { "type": "integer", "minimum": 1, "description": "Optional 1-based starting line." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Optional maximum number of lines." }
            }),
            vec!["path"],
        ),
        ClientToolName::ListDirectory => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local directory path under the user's workspace." },
                "recursive": { "type": "boolean", "default": false, "description": "Whether to list recursively. Defaults to false." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum entries to return." },
                "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." }
            }),
            vec!["path"],
        ),
        ClientToolName::FindPaths => object_schema(
            json!({
                "root": { "type": "string", "description": "Optional workspace-relative or absolute directory path under the workspace. Defaults to the workspace root." },
                "glob": { "type": "string", "description": "Glob pattern to match against relative paths, such as **/*.rs or package.json." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Maximum paths to return." },
                "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." }
            }),
            vec!["glob"],
        ),
        ClientToolName::SearchFiles => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file/directory path under the workspace." },
                "query": { "type": "string", "description": "Optional literal text to search for inside files. If omitted or empty, pattern is used for filename/path discovery only." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Maximum matches to return." },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Maximum total bytes to scan." },
                "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." },
                "case_sensitive": { "type": "boolean", "default": true, "description": "Whether literal matching is case-sensitive. Defaults to true." },
                "pattern": { "type": "string", "description": "Optional simple wildcard pattern matched against relative file paths. Supports * and ?." },
                "extensions": { "type": "array", "items": { "type": "string" }, "maxItems": 10, "description": "Optional list of file extensions to include, such as [\"rs\", \"ts\"]." }
            }),
            vec!["path"],
        ),
        ClientToolName::Stat => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file/directory path under the workspace." }
            }),
            vec!["path"],
        ),
        ClientToolName::EditFile => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file path under the workspace." },
                "old_text": { "type": "string", "description": "Exact text to replace." },
                "new_text": { "type": "string", "description": "Replacement text." },
                "replace_all": { "type": "boolean", "default": false, "description": "Replace all occurrences when policy allows it. Defaults to false." }
            }),
            vec!["path", "old_text", "new_text"],
        ),
        ClientToolName::CreateTextFile => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file path under the workspace." },
                "content": { "type": "string", "description": "File contents to write." },
                "create_parent_dirs": { "type": "boolean", "default": false, "description": "Create missing parent directories when policy allows it." }
            }),
            vec!["path", "content"],
        ),
        ClientToolName::CreateDirectory => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local directory path under the workspace." }
            }),
            vec!["path"],
        ),
        ClientToolName::MovePath | ClientToolName::CopyPath => object_schema(
            json!({
                "source_path": { "type": "string", "description": "Workspace-relative or absolute local source path under the workspace." },
                "destination_path": { "type": "string", "description": "Workspace-relative or absolute local destination path under the workspace." },
                "overwrite": { "type": "boolean", "default": false, "description": "Overwrite the destination when policy allows it." }
            }),
            vec!["source_path", "destination_path"],
        ),
        ClientToolName::ApplyPatch => object_schema(
            json!({
                "patch": { "type": "string", "description": "Standard unified diff: non-empty paired `--- a/path` and `+++ b/path` headers (or `/dev/null` for creates/deletes) followed by one or more `@@` hunks. Context and removed lines are validated against the current file before writing. Do not send prose or Markdown fences. For one exact replacement in one file, use fs_edit_file instead." },
                "dry_run": { "type": "boolean", "default": false, "description": "Validate without writing changes." }
            }),
            vec!["patch"],
        ),
        ClientToolName::DeletePath => object_schema(
            json!({
                "path": { "type": "string", "description": "Workspace-relative path or absolute local file/directory path under the workspace." },
                "recursive": { "type": "boolean", "default": false, "description": "Required for non-empty directories." }
            }),
            vec!["path"],
        ),
        ClientToolName::GitStatus => object_schema(
            json!({
                "path": { "type": "string", "description": "Optional workspace-relative or absolute path inside the git repository/workspace." }
            }),
            vec![],
        ),
        ClientToolName::GitDiff => object_schema(
            json!({
                "path": { "type": "string", "description": "Optional workspace-relative or absolute path inside the git repository/workspace." },
                "staged": { "type": "boolean", "default": false, "description": "Show staged changes instead of unstaged changes." },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144 }
            }),
            vec![],
        ),
        ClientToolName::GitLog => object_schema(
            json!({
                "path": { "type": "string", "description": "Optional workspace-relative or absolute path inside the git repository/workspace." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            }),
            vec![],
        ),
        ClientToolName::GitShow => object_schema(
            json!({
                "revision": { "type": "string", "description": "Commit/revision to show." },
                "path": { "type": "string", "description": "Optional workspace-relative or absolute path inside the git repository/workspace." },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144 }
            }),
            vec!["revision"],
        ),
        ClientToolName::GitAdd | ClientToolName::GitRestore => object_schema(
            json!({
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative or absolute workspace paths to stage or restore." }
            }),
            vec!["paths"],
        ),
        ClientToolName::GitCommit => object_schema(
            json!({
                "message": { "type": "string", "description": "Commit message." }
            }),
            vec!["message"],
        ),
        ClientToolName::GitStash => object_schema(
            json!({
                "message": { "type": "string", "description": "Optional stash message." },
                "include_untracked": { "type": "boolean", "default": false }
            }),
            vec![],
        ),
        ClientToolName::RunCommand
        | ClientToolName::ProcessRun
        | ClientToolName::TerminalRunCommand => object_schema(
            json!({
                "command": { "type": "string", "description": "Executable name. Shell strings are not accepted." },
                "args": { "type": "array", "items": { "type": "string" }, "description": "Command arguments." },
                "cwd": { "type": "string", "description": "Absolute working directory under the workspace." },
                "bypass_tool_redirect": { "type": "boolean", "description": "Explicitly override dedicated-tool redirects when command execution is truly required." },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000 },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 131072 },
                "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Optional non-secret environment values." },
                "reducer_mode": { "type": "string", "enum": ["execute_via_rtk", "postprocess", "none"], "default": "execute_via_rtk", "description": "Output reduction mode. Defaults to execute_via_rtk, which runs supported commands through RTK for compact output. Use none when the user asks for pure/noisy/raw command results." }
            }),
            vec!["command"],
        ),
        ClientToolName::ChromeOpen => object_schema(
            json!({ "url": { "type": "string", "description": "HTTP/HTTPS URL to open." } }),
            vec!["url"],
        ),
        ClientToolName::ChromeSnapshot
        | ClientToolName::ChromeConsoleMessages
        | ClientToolName::ChromeNetworkRequests
        | ClientToolName::ChromeScreenshot
        | ClientToolName::McpCallTool => generic_object_schema(),
    };
    json!({
        "name": descriptor.provider_name,
        "description": format!(
            "Armature local workspace tool ({}, kind={}, risk={}). {}. Use only for the user's local editor workspace unless the tool explicitly targets browser state.",
            descriptor.canonical_name,
            descriptor.kind,
            descriptor.risk,
            descriptor.title,
        ),
        "parameters": parameters,
    })
}

const ARMATURE_READ_TEXT_FILE_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: Some(FsTargetKindPolicy::File),
    },
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

const ARMATURE_LIST_DIRECTORY_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::FilterResults,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: Some(FsTargetKindPolicy::Directory),
    },
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

const ARMATURE_FIND_PATHS_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::FilterResults,
    target_policy: TargetPolicy::WorkspaceRootOrPath {
        arg: Some("root"),
        default_to_workspace_root: true,
        required_kind: Some(FsTargetKindPolicy::Directory),
    },
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

const ARMATURE_SEARCH_FILES_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::FilterResults,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: None,
    },
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

const ARMATURE_STAT_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: None,
    },
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

const ARMATURE_EDIT_FILE_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: Some(FsTargetKindPolicy::File),
    },
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

const ARMATURE_CREATE_TEXT_FILE_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: None,
    },
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

const ARMATURE_CREATE_DIRECTORY_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: None,
    },
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

const ARMATURE_MOVE_PATH_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::SourceDestination {
        source_arg: "source_path",
        destination_arg: "destination_path",
    },
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

const ARMATURE_COPY_PATH_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::SourceDestination {
        source_arg: "source_path",
        destination_arg: "destination_path",
    },
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

const ARMATURE_APPLY_PATCH_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspaceRootOrPath {
        arg: Some("base_path"),
        default_to_workspace_root: true,
        required_kind: Some(FsTargetKindPolicy::Directory),
    },
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

const ARMATURE_DELETE_PATH_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::WorkspacePath {
        arg: "path",
        required_kind: None,
    },
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

const ARMATURE_GIT_STATUS_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::None,
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

const ARMATURE_GIT_DIFF_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::None,
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

const ARMATURE_GIT_LOG_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::None,
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

const ARMATURE_GIT_SHOW_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::None,
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

const ARMATURE_GIT_ADD_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Never,
    sensitive_path_policy: SensitivePathPolicy::Deny,
    target_policy: TargetPolicy::None,
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

const ARMATURE_GIT_WRITE_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_path_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::RequireApproval,
    target_policy: TargetPolicy::None,
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

const ARMATURE_PROCESS_RUN_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "client_session.workspace_roots",
    path_containment: "adapter_enforced_absolute_cwd_under_allowed_roots",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::RequireApproval,
    target_policy: TargetPolicy::Command {
        command_arg: "command",
        cwd_arg: "cwd",
    },
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

const ARMATURE_CHROME_POLICY: ToolPolicy = ToolPolicy {
    scope_basis: "armature:tools",
    role_basis: "pair_agent",
    allowed_roots_basis: "chrome_cdp_endpoint",
    path_containment: "adapter_enforced_chrome_cdp_endpoint",
    execution_target: ExecutionTargetPolicy::ArmatureLocal,
    approval_policy: ApprovalPolicy::Required,
    sensitive_path_policy: SensitivePathPolicy::RequireApproval,
    target_policy: TargetPolicy::None,
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

pub fn client_tool_policy(tool: ClientToolName) -> ToolPolicy {
    match tool {
        ClientToolName::ReadTextFile => ARMATURE_READ_TEXT_FILE_POLICY,
        ClientToolName::ListDirectory => ARMATURE_LIST_DIRECTORY_POLICY,
        ClientToolName::FindPaths => ARMATURE_FIND_PATHS_POLICY,
        ClientToolName::SearchFiles => ARMATURE_SEARCH_FILES_POLICY,
        ClientToolName::Stat => ARMATURE_STAT_POLICY,
        ClientToolName::EditFile => ARMATURE_EDIT_FILE_POLICY,
        ClientToolName::CreateTextFile => ARMATURE_CREATE_TEXT_FILE_POLICY,
        ClientToolName::CreateDirectory => ARMATURE_CREATE_DIRECTORY_POLICY,
        ClientToolName::MovePath => ARMATURE_MOVE_PATH_POLICY,
        ClientToolName::CopyPath => ARMATURE_COPY_PATH_POLICY,
        ClientToolName::ApplyPatch => ARMATURE_APPLY_PATCH_POLICY,
        ClientToolName::DeletePath => ARMATURE_DELETE_PATH_POLICY,
        ClientToolName::GitStatus => ARMATURE_GIT_STATUS_POLICY,
        ClientToolName::GitDiff => ARMATURE_GIT_DIFF_POLICY,
        ClientToolName::GitLog => ARMATURE_GIT_LOG_POLICY,
        ClientToolName::GitShow => ARMATURE_GIT_SHOW_POLICY,
        ClientToolName::GitAdd => ARMATURE_GIT_ADD_POLICY,
        ClientToolName::GitRestore => ARMATURE_GIT_WRITE_POLICY,
        ClientToolName::GitCommit => ARMATURE_GIT_WRITE_POLICY,
        ClientToolName::GitStash => ARMATURE_GIT_WRITE_POLICY,
        ClientToolName::RunCommand
        | ClientToolName::ProcessRun
        | ClientToolName::TerminalRunCommand => ARMATURE_PROCESS_RUN_POLICY,
        ClientToolName::ChromeOpen
        | ClientToolName::ChromeSnapshot
        | ClientToolName::ChromeConsoleMessages
        | ClientToolName::ChromeNetworkRequests
        | ClientToolName::ChromeScreenshot => ARMATURE_CHROME_POLICY,
        ClientToolName::McpCallTool => ARMATURE_PROCESS_RUN_POLICY,
    }
}

pub fn tool_class(tool: ClientToolName) -> ToolClass {
    match tool {
        ClientToolName::ReadTextFile
        | ClientToolName::ListDirectory
        | ClientToolName::FindPaths
        | ClientToolName::SearchFiles
        | ClientToolName::Stat
        | ClientToolName::GitStatus
        | ClientToolName::GitDiff
        | ClientToolName::GitLog
        | ClientToolName::GitShow => ToolClass::ReadOnly,
        ClientToolName::EditFile
        | ClientToolName::CreateTextFile
        | ClientToolName::CreateDirectory
        | ClientToolName::MovePath
        | ClientToolName::CopyPath
        | ClientToolName::ApplyPatch
        | ClientToolName::DeletePath
        | ClientToolName::GitAdd
        | ClientToolName::GitRestore
        | ClientToolName::GitCommit
        | ClientToolName::GitStash => ToolClass::WorkspaceMutation,
        ClientToolName::RunCommand
        | ClientToolName::ProcessRun
        | ClientToolName::TerminalRunCommand => ToolClass::Execution,
        ClientToolName::ChromeOpen
        | ClientToolName::ChromeSnapshot
        | ClientToolName::ChromeConsoleMessages
        | ClientToolName::ChromeNetworkRequests
        | ClientToolName::ChromeScreenshot => ToolClass::Browser,
        ClientToolName::McpCallTool => ToolClass::Execution,
    }
}

pub fn pre_risk_checkpoint_class(tool: ClientToolName) -> PreRiskCheckpointClass {
    match tool {
        ClientToolName::DeletePath => PreRiskCheckpointClass::Destructive,
        ClientToolName::EditFile
        | ClientToolName::CreateTextFile
        | ClientToolName::CreateDirectory
        | ClientToolName::MovePath
        | ClientToolName::CopyPath
        | ClientToolName::ApplyPatch
        | ClientToolName::GitAdd
        | ClientToolName::GitRestore
        | ClientToolName::GitCommit
        | ClientToolName::GitStash
        | ClientToolName::RunCommand
        | ClientToolName::ProcessRun
        | ClientToolName::TerminalRunCommand
        | ClientToolName::McpCallTool => PreRiskCheckpointClass::Broad,
        _ => PreRiskCheckpointClass::None,
    }
}

pub fn provider_tool_allowed_in_policy(tool_name: &str, policy: &ResolvedSessionPolicy) -> bool {
    ClientToolName::from_provider_alias(tool_name)
        .map(|tool| policy.allows_tool(tool))
        .unwrap_or(false)
}

pub fn client_tool_policy_json_for_provider(tool_name: &str) -> serde_json::Value {
    let Some(tool) = ClientToolName::from_provider_alias(tool_name) else {
        return json!({
            "scope_basis": "armature:tools",
            "execution_target": ExecutionTargetPolicy::ArmatureLocal.as_str(),
            "risk": "read_only",
            "approval_policy": ApprovalPolicy::Never.as_str(),
            "approval_required": false,
            "sensitive_path_policy": SensitivePathPolicy::Deny.as_str(),
            "target_policy": TargetPolicy::None.to_json(),
        });
    };
    client_tool_policy(tool).to_json(tool.descriptor())
}

pub fn client_tool_display(tool: ClientToolName) -> ToolDisplayDescriptor {
    match tool {
        ClientToolName::ReadTextFile => ToolDisplayDescriptor {
            label: "Read file",
            category: "filesystem",
            progress_verb: "Reading",
            complete_verb: "Read",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading this workspace file.",
        },
        ClientToolName::ListDirectory => ToolDisplayDescriptor {
            label: "List directory",
            category: "filesystem",
            progress_verb: "Listing",
            complete_verb: "Listed",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow listing this workspace directory.",
        },
        ClientToolName::FindPaths => ToolDisplayDescriptor {
            label: "Find paths",
            category: "filesystem",
            progress_verb: "Finding paths for",
            complete_verb: "Found paths for",
            target_arg_keys: &["glob"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow finding matching workspace paths.",
        },
        ClientToolName::SearchFiles => ToolDisplayDescriptor {
            label: "Search files",
            category: "filesystem",
            progress_verb: "Searching",
            complete_verb: "Searched",
            target_arg_keys: &["query", "pattern", "path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow searching workspace file contents or paths.",
        },
        ClientToolName::Stat => ToolDisplayDescriptor {
            label: "Inspect path",
            category: "filesystem",
            progress_verb: "Inspecting",
            complete_verb: "Inspected",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading metadata for this workspace path.",
        },
        ClientToolName::EditFile => ToolDisplayDescriptor {
            label: "Edit file",
            category: "filesystem",
            progress_verb: "Editing",
            complete_verb: "Edited",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &["old_text", "new_text"],
            approval_summary: "Allow changing this workspace file.",
        },
        ClientToolName::CreateTextFile => ToolDisplayDescriptor {
            label: "Create file",
            category: "filesystem",
            progress_verb: "Creating",
            complete_verb: "Created",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &["content"],
            approval_summary: "Allow creating this workspace file.",
        },
        ClientToolName::CreateDirectory => ToolDisplayDescriptor {
            label: "Create directory",
            category: "filesystem",
            progress_verb: "Creating",
            complete_verb: "Created",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow creating this workspace directory.",
        },
        ClientToolName::MovePath => ToolDisplayDescriptor {
            label: "Move path",
            category: "filesystem",
            progress_verb: "Moving",
            complete_verb: "Moved",
            target_arg_keys: &["source_path", "destination_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow moving or renaming this workspace path.",
        },
        ClientToolName::CopyPath => ToolDisplayDescriptor {
            label: "Copy path",
            category: "filesystem",
            progress_verb: "Copying",
            complete_verb: "Copied",
            target_arg_keys: &["source_path", "destination_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow copying this workspace path.",
        },
        ClientToolName::ApplyPatch => ToolDisplayDescriptor {
            label: "Apply patch",
            category: "filesystem",
            progress_verb: "Applying patch",
            complete_verb: "Applied patch",
            target_arg_keys: &["base_path"],
            sensitive_arg_keys: &["patch"],
            approval_summary: "Allow applying a patch to workspace files.",
        },
        ClientToolName::DeletePath => ToolDisplayDescriptor {
            label: "Delete path",
            category: "filesystem",
            progress_verb: "Deleting",
            complete_verb: "Deleted",
            target_arg_keys: &["path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow deleting this workspace path.",
        },
        ClientToolName::GitStatus => ToolDisplayDescriptor {
            label: "Check git status",
            category: "git",
            progress_verb: "Checking git status in",
            complete_verb: "Checked git status in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading git status for this workspace repository.",
        },
        ClientToolName::GitDiff => ToolDisplayDescriptor {
            label: "Inspect git diff",
            category: "git",
            progress_verb: "Inspecting git diff in",
            complete_verb: "Inspected git diff in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading git diff for this workspace repository.",
        },
        ClientToolName::GitLog => ToolDisplayDescriptor {
            label: "Read git log",
            category: "git",
            progress_verb: "Reading git log in",
            complete_verb: "Read git log in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading git history for this workspace repository.",
        },
        ClientToolName::GitShow => ToolDisplayDescriptor {
            label: "Show git revision",
            category: "git",
            progress_verb: "Showing git revision",
            complete_verb: "Showed git revision",
            target_arg_keys: &["revision", "path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading this git revision or file.",
        },
        ClientToolName::GitAdd => ToolDisplayDescriptor {
            label: "Stage git paths",
            category: "git",
            progress_verb: "Staging git paths in",
            complete_verb: "Staged git paths in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow staging explicit git paths.",
        },
        ClientToolName::GitRestore => ToolDisplayDescriptor {
            label: "Restore git paths",
            category: "git",
            progress_verb: "Restoring git paths in",
            complete_verb: "Restored git paths in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow restoring git paths; this may discard changes.",
        },
        ClientToolName::GitCommit => ToolDisplayDescriptor {
            label: "Create git commit",
            category: "git",
            progress_verb: "Committing",
            complete_verb: "Committed",
            target_arg_keys: &["message"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow creating a git commit from staged changes.",
        },
        ClientToolName::GitStash => ToolDisplayDescriptor {
            label: "Stash git changes",
            category: "git",
            progress_verb: "Stashing git changes in",
            complete_verb: "Stashed git changes in",
            target_arg_keys: &["repo_path"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow creating a git stash.",
        },
        ClientToolName::RunCommand => ToolDisplayDescriptor {
            label: "Run command",
            category: "terminal",
            progress_verb: "Running command",
            complete_verb: "Ran command",
            target_arg_keys: &["command", "cwd"],
            sensitive_arg_keys: &["env"],
            approval_summary: "Allow running this command in the workspace.",
        },
        ClientToolName::ProcessRun => ToolDisplayDescriptor {
            label: "Run process",
            category: "terminal",
            progress_verb: "Running",
            complete_verb: "Ran",
            target_arg_keys: &["command", "cwd"],
            sensitive_arg_keys: &["env"],
            approval_summary: "Allow running this process in the workspace.",
        },
        ClientToolName::TerminalRunCommand => ToolDisplayDescriptor {
            label: "Run terminal command",
            category: "terminal",
            progress_verb: "Running terminal command",
            complete_verb: "Ran terminal command",
            target_arg_keys: &["command", "cwd"],
            sensitive_arg_keys: &["env"],
            approval_summary: "Allow running this terminal command in the workspace.",
        },
        ClientToolName::ChromeOpen => ToolDisplayDescriptor {
            label: "Open browser",
            category: "browser",
            progress_verb: "Opening",
            complete_verb: "Opened",
            target_arg_keys: &["url"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow opening this URL in the browser session.",
        },
        ClientToolName::ChromeSnapshot => ToolDisplayDescriptor {
            label: "Inspect browser page",
            category: "browser",
            progress_verb: "Inspecting browser page",
            complete_verb: "Inspected browser page",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading the current browser page snapshot.",
        },
        ClientToolName::ChromeConsoleMessages => ToolDisplayDescriptor {
            label: "Read browser console",
            category: "browser",
            progress_verb: "Reading browser console",
            complete_verb: "Read browser console",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading browser console messages.",
        },
        ClientToolName::ChromeNetworkRequests => ToolDisplayDescriptor {
            label: "Inspect network requests",
            category: "browser",
            progress_verb: "Inspecting network requests",
            complete_verb: "Inspected network requests",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Allow reading browser network request metadata.",
        },
        ClientToolName::ChromeScreenshot => ToolDisplayDescriptor {
            label: "Capture screenshot",
            category: "browser",
            progress_verb: "Capturing screenshot",
            complete_verb: "Captured screenshot",
            target_arg_keys: &["format"],
            sensitive_arg_keys: &[],
            approval_summary: "Allow capturing a browser screenshot.",
        },
        ClientToolName::McpCallTool => ToolDisplayDescriptor {
            label: "Run MCP tool",
            category: "mcp",
            progress_verb: "Running MCP tool",
            complete_verb: "Ran MCP tool",
            target_arg_keys: &[],
            sensitive_arg_keys: &[],
            approval_summary: "Allow running this client-provided MCP tool.",
        },
    }
}

pub fn client_tool_display_for_provider(
    tool_name: &str,
    args: &serde_json::Value,
) -> serde_json::Value {
    let Some(tool) = ClientToolName::from_provider_alias(tool_name) else {
        return fallback_tool_display(tool_name, args);
    };
    let display = client_tool_display(tool);
    let target = crate::tools::display::tool_target_summary(display.target_arg_keys, args);
    json!({
        "label": display.label,
        "title": target.as_ref().map(|target| format!("{} {target}", display.progress_verb)).unwrap_or_else(|| display.label.to_string()),
        "subtitle": target,
        "category": display.category,
        "status": "requested",
        "progress": display.progress_verb,
        "complete": display.complete_verb,
        "approval_summary": display.approval_summary,
        "arguments_summary": summarize_arguments(args, display.sensitive_arg_keys),
        "target": summarize_target_object(display.target_arg_keys, args),
    })
}

fn fallback_tool_display(tool_name: &str, args: &serde_json::Value) -> serde_json::Value {
    json!({
        "label": tool_name,
        "title": format!("Use {tool_name}"),
        "category": "tool",
        "status": "requested",
        "arguments_summary": summarize_arguments(args, &[]),
    })
}

fn summarize_target_object(keys: &[&str], args: &serde_json::Value) -> serde_json::Value {
    let Some(object) = args.as_object() else {
        return serde_json::Value::Null;
    };
    let mut out = serde_json::Map::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            if crate::tools::display::is_display_path_key(key) {
                if let Some(raw) = value.as_str() {
                    out.insert(
                        (*key).to_string(),
                        json!(crate::tools::display::display_path(raw)),
                    );
                    continue;
                }
            }
            out.insert((*key).to_string(), summarize_value(value, false));
        }
    }
    serde_json::Value::Object(out)
}

fn summarize_arguments(args: &serde_json::Value, sensitive_keys: &[&str]) -> serde_json::Value {
    let Some(object) = args.as_object() else {
        return summarize_value(args, false);
    };
    let mut out = serde_json::Map::new();
    for (key, value) in object {
        out.insert(
            key.clone(),
            summarize_value(
                value,
                sensitive_keys.iter().any(|sensitive| sensitive == key),
            ),
        );
    }
    serde_json::Value::Object(out)
}

fn summarize_value(value: &serde_json::Value, sensitive: bool) -> serde_json::Value {
    if sensitive {
        return json!({ "redacted": true, "kind": value_kind(value), "bytes": value.to_string().len() });
    }
    match value {
        serde_json::Value::String(s) => json!(preview(s, 160)),
        serde_json::Value::Array(items) => {
            json!({ "items": items.len(), "preview": items.iter().take(5).map(|v| summarize_value(v, false)).collect::<Vec<_>>() })
        }
        serde_json::Value::Object(map) => {
            json!({ "keys": map.keys().take(20).cloned().collect::<Vec<_>>() })
        }
        _ => value.clone(),
    }
}

fn value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push('…');
    }
    out
}

pub fn supported_provider_tool_names() -> Vec<&'static str> {
    ClientToolName::all()
        .iter()
        .map(|tool| tool.descriptor().provider_name)
        .collect()
}
#[cfg(test)]
mod tests;
