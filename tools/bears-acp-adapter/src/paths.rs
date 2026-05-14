use crate::SessionContext;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

pub(crate) fn session_workspace_roots(context: &SessionContext) -> Vec<PathBuf> {
    if context.roots.is_empty() {
        vec![PathBuf::from(&context.cwd)]
    } else {
        context.roots.iter().map(PathBuf::from).collect()
    }
}

pub(crate) fn is_absolute_local_path(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    Path::new(path).is_absolute()
        || path.starts_with("\\\\")
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
}

pub(crate) fn normalize_requested_tool_path(path: &str) -> Result<PathBuf> {
    let path = file_uri_or_path_to_path(path).ok_or_else(|| anyhow!("path must not be empty"))?;
    if !is_absolute_local_path(&path) {
        return Err(anyhow!(
            "tool path must be an absolute local path; got {path:?}"
        ));
    }
    Ok(PathBuf::from(path))
}

pub(crate) fn ensure_path_allowed_for_session(context: &SessionContext, path: &Path) -> Result<()> {
    let roots = if context.roots.is_empty() {
        vec![context.cwd.as_str()]
    } else {
        context.roots.iter().map(String::as_str).collect::<Vec<_>>()
    };
    let allowed = roots.iter().any(|root| {
        let root_path = Path::new(root);
        path == root_path || path.starts_with(root_path)
    });
    if allowed {
        Ok(())
    } else {
        Err(anyhow!(
            "tool path {} is outside the ACP session workspace roots",
            path.display()
        ))
    }
}

pub(crate) fn is_hidden_path_component(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|s| s.starts_with('.') && s != "." && s != "..")
        })
}

pub(crate) fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.contains("/.git/") || path_str.ends_with("/.git") {
        return true;
    }
    path.components().any(|component| {
        let Some(part) = component.as_os_str().to_str() else {
            return false;
        };
        let lower = part.to_ascii_lowercase();
        lower == ".env"
            || lower.starts_with(".env.")
            || lower.contains("id_rsa")
            || lower.contains("id_ed25519")
            || lower.contains("private_key")
            || lower.contains("secret")
            || lower.contains("token")
            || lower.ends_with(".pem")
            || lower.ends_with(".key")
    })
}

pub(crate) fn file_uri_or_path_to_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with("file://") {
        return Some(trimmed.to_string());
    }
    let without_scheme = trimmed.trim_start_matches("file://");
    #[cfg(windows)]
    let path = without_scheme.trim_start_matches('/').to_string();
    #[cfg(not(windows))]
    let path = format!("/{}", without_scheme.trim_start_matches('/'));
    Some(percent_decode_file_path(&path))
}

fn percent_decode_file_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
