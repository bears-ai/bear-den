use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolName {
    ReadTextFile,
    ListDirectory,
    SearchFiles,
}

impl AcpToolName {
    pub fn descriptor(self) -> &'static AcpToolDescriptor {
        match self {
            Self::ReadTextFile => &ACP_READ_TEXT_FILE_TOOL,
            Self::ListDirectory => &ACP_LIST_DIRECTORY_TOOL,
            Self::SearchFiles => &ACP_SEARCH_FILES_TOOL,
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::ReadTextFile, Self::ListDirectory, Self::SearchFiles]
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
};

pub fn acp_tool_policy(tool: AcpToolName) -> AcpToolPolicy {
    match tool {
        AcpToolName::ReadTextFile => ACP_READ_TEXT_FILE_POLICY,
        AcpToolName::ListDirectory => ACP_LIST_DIRECTORY_POLICY,
        AcpToolName::SearchFiles => ACP_SEARCH_FILES_POLICY,
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
                    "include_hidden": { "type": "boolean", "default": false, "description": "Include hidden dotfiles and dot-directories. Defaults to false." }
                },
                "required": ["path", "query"]
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
        assert_eq!(descriptors.len(), 3);
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
    fn read_text_file_descriptor_wrapper_still_works() {
        let descriptor = acp_read_text_file_client_tool_descriptor();
        assert_eq!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.provider_name);
    }

    #[test]
    fn tool_policy_includes_authoritative_limits_and_scope() {
        let list_policy = acp_tool_policy_json_for_provider("fs_list_directory");
        assert_eq!(list_policy["scope_basis"], "acp:tools");
        assert_eq!(list_policy["role_basis"], "pair_agent");
        assert_eq!(list_policy["allowed_roots_basis"], "acp_session.workspace_roots");
        assert_eq!(list_policy["max_entries"], 1000);
        assert_eq!(list_policy["include_hidden_default"], false);

        let search_policy = acp_tool_policy_json_for_provider("fs_search_files");
        assert_eq!(search_policy["max_results"], 200);
        assert_eq!(search_policy["max_bytes"], 1_048_576);
        assert_eq!(search_policy["approval_required"], true);
    }
}
