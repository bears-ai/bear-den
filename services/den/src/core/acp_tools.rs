use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpToolName {
    ReadTextFile,
}

impl AcpToolName {
    pub fn descriptor(self) -> &'static AcpToolDescriptor {
        match self {
            Self::ReadTextFile => &ACP_READ_TEXT_FILE_TOOL,
        }
    }

    pub fn from_provider_alias(raw: &str) -> Option<Self> {
        match raw {
            "bears/read_text_file"
            | "fs.read_text_file"
            | "fs_read_text_file"
            | "read_text_file" => Some(Self::ReadTextFile),
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

pub const ACP_READ_TEXT_FILE_TOOL: AcpToolDescriptor = AcpToolDescriptor {
    provider_name: "fs_read_text_file",
    canonical_name: "acp.fs.read_text_file",
    adapter_method: "bears/read_text_file",
    client_method: "fs/read_text_file",
    title: "Read file",
    kind: "read",
    risk: "read_only",
};

pub fn provider_tool_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn acp_read_text_file_client_tool_descriptor() -> serde_json::Value {
    debug_assert!(provider_tool_name_is_safe(
        ACP_READ_TEXT_FILE_TOOL.provider_name
    ));
    json!({
        "name": ACP_READ_TEXT_FILE_TOOL.provider_name,
        "description": format!(
            "ACP local workspace tool ({}, target={}, adapter={}, client={}, kind={}, risk={}). Reads a UTF-8 text file from the user's editor workspace through the local adapter. Use only for user workspace files, not server files.",
            ACP_READ_TEXT_FILE_TOOL.canonical_name,
            "acp_client",
            ACP_READ_TEXT_FILE_TOOL.adapter_method,
            ACP_READ_TEXT_FILE_TOOL.client_method,
            ACP_READ_TEXT_FILE_TOOL.kind,
            ACP_READ_TEXT_FILE_TOOL.risk,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_text_file_provider_name_is_safe() {
        assert!(provider_tool_name_is_safe(
            ACP_READ_TEXT_FILE_TOOL.provider_name
        ));
        assert!(!provider_tool_name_is_safe("fs.read_text_file"));
        assert!(!provider_tool_name_is_safe("fs/read_text_file"));
    }

    #[test]
    fn read_text_file_descriptor_uses_provider_name_only() {
        let descriptor = acp_read_text_file_client_tool_descriptor();
        assert_eq!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.provider_name);
        assert_ne!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.canonical_name);
        assert_ne!(descriptor["name"], ACP_READ_TEXT_FILE_TOOL.client_method);
        assert!(!descriptor
            .to_string()
            .contains("\"name\":\"fs.read_text_file\""));
    }
}
