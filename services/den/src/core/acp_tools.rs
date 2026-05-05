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
    SearchFiles,
    ReplaceText,
    DeletePath,
}

impl AcpToolName {
    pub fn descriptor(self) -> &'static AcpToolDescriptor {
        match self {
            Self::ReadTextFile => &ACP_READ_TEXT_FILE_TOOL,
            Self::ListDirectory => &ACP_LIST_DIRECTORY_TOOL,
            Self::SearchFiles => &ACP_SEARCH_FILES_TOOL,
            Self::ReplaceText => &ACP_REPLACE_TEXT_TOOL,
            Self::DeletePath => &ACP_DELETE_PATH_TOOL,
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ReadTextFile,
            Self::ListDirectory,
            Self::SearchFiles,
            Self::ReplaceText,
            Self::DeletePath,
        ]
    }

    pub fn missing_required_string_arg(self, args: &serde_json::Value) -> Option<&'static str> {
        for arg in self.required_string_args() {
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
            Self::ReadTextFile | Self::ListDirectory => &["path"],
            Self::SearchFiles => &["path", "query"],
            Self::ReplaceText => &["path", "old_text", "new_text"],
            Self::DeletePath => &["path"],
        }
    }

    fn allow_empty_required_string(self, arg: &str) -> bool {
        matches!(self, Self::ReplaceText) && matches!(arg, "old_text" | "new_text")
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
            "bears/search_files" | "fs/search_files" | "fs.search_files" | "fs_search_files"
            | "search_files" => Some(Self::SearchFiles),
            "bears/replace_text" | "fs/replace_text" | "fs.replace_text" | "fs_replace_text"
            | "replace_text" => Some(Self::ReplaceText),
            "bears/delete_path" | "fs/delete_path" | "fs.delete_path" | "fs_delete_path"
            | "delete_path" => Some(Self::DeletePath),
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

pub const ACP_SEARCH_FILES_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_search_files",
    canonical_name: "acp.fs.search_files",
    adapter_method: "bears/search_files",
    client_method: "fs/search_files",
    title: "Search files",
    kind: "search",
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

pub const ACP_DELETE_PATH_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_delete_path",
    canonical_name: "acp.fs.delete_path",
    adapter_method: "bears/delete_path",
    client_method: "fs/delete_path",
    title: "Delete path",
    kind: "delete",
    risk: "deletes_workspace",
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

pub fn acp_tool_policy(tool: AcpToolName) -> AcpToolPolicy {
    match tool {
        AcpToolName::ReadTextFile => ACP_READ_TEXT_FILE_POLICY,
        AcpToolName::ListDirectory => ACP_LIST_DIRECTORY_POLICY,
        AcpToolName::SearchFiles => ACP_SEARCH_FILES_POLICY,
        AcpToolName::ReplaceText => ACP_REPLACE_TEXT_POLICY,
        AcpToolName::DeletePath => ACP_DELETE_PATH_POLICY,
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
        "fs_search_files" => json!({
            "name": tool.provider_name,
            "description": format!(
                "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Searches UTF-8 text files under a workspace path through the local adapter with bounded results and bytes.",
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
                    "query": { "type": "string", "description": "Literal text to search for." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Maximum matches to return." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Maximum total bytes to scan." },
                    "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." },
                    "case_sensitive": { "type": "boolean", "default": true, "description": "Whether literal matching is case-sensitive. Defaults to true." },
                    "pattern": { "type": "string", "description": "Optional simple wildcard pattern matched against relative file paths. Supports `*` and `?`." },
                    "extensions": { "type": "array", "items": { "type": "string" }, "maxItems": 10, "description": "Optional list of file extensions to include, such as [\"rs\", \"ts\"]." }
                },
                "required": ["path", "query"]
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
        assert_eq!(descriptors.len(), 5);
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
                "fs_search_files": true,
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
        assert!(names.contains(&"fs_search_files"));
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
                    "fs_replace_text": { "supported": true, "version": 1 },
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
            vec!["fs_read_text_file", "fs_replace_text", "fs_delete_path"]
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

        let search_policy = acp_tool_policy_json_for_provider("fs_search_files");
        assert_eq!(search_policy["max_results"], 200);
        assert_eq!(search_policy["max_bytes"], 1_048_576);
        assert_eq!(search_policy["approval_required"], true);

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

        let delete_policy = acp_tool_policy_json_for_provider("fs_delete_path");
        assert_eq!(delete_policy["risk"], "deletes_workspace");
        assert_eq!(
            delete_policy["sensitive_path_policy"],
            "deny_sensitive_paths"
        );
        assert_eq!(delete_policy["max_entries"], 100);
        assert_eq!(delete_policy["deny_hidden_paths"], true);
    }
}
