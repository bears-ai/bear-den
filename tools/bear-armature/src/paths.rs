use crate::SessionContext;
use anyhow::{anyhow, Result};
use std::{
    fs::Metadata,
    path::{Path, PathBuf},
};

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
    Ok(lexically_normalize_path(PathBuf::from(path)))
}

pub(crate) fn resolve_requested_tool_path(context: &SessionContext, path: &str) -> Result<PathBuf> {
    let path = file_uri_or_path_to_path(path).ok_or_else(|| anyhow!("path must not be empty"))?;
    let resolved = if is_absolute_local_path(&path) {
        PathBuf::from(path)
    } else {
        PathBuf::from(&context.cwd).join(path)
    };
    Ok(lexically_normalize_path(resolved))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FsTargetKind {
    File,
    Directory,
    Missing,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedFsTarget {
    pub(crate) requested_path: String,
    pub(crate) resolved_path: PathBuf,
    pub(crate) workspace_root: PathBuf,
    pub(crate) kind: FsTargetKind,
    pub(crate) metadata: Option<Metadata>,
}

impl ResolvedFsTarget {
    pub(crate) fn is_file(&self) -> bool {
        self.kind == FsTargetKind::File
    }

    pub(crate) fn is_directory(&self) -> bool {
        self.kind == FsTargetKind::Directory
    }

    pub(crate) fn size_bytes(&self) -> Option<u64> {
        self.metadata
            .as_ref()
            .filter(|_| self.is_file())
            .map(Metadata::len)
    }
}

pub(crate) fn resolve_fs_target(
    context: &SessionContext,
    raw_path: &str,
) -> Result<ResolvedFsTarget> {
    let resolved_path = resolve_requested_tool_path(context, raw_path)?;
    ensure_path_allowed_for_session(context, &resolved_path)?;
    let workspace_root = workspace_root_for_path(context, &resolved_path).ok_or_else(|| {
        anyhow!(
            "tool path {} is outside the ACP session workspace roots",
            resolved_path.display()
        )
    })?;
    let metadata = std::fs::metadata(&resolved_path).ok();
    let kind = match metadata.as_ref() {
        Some(metadata) if metadata.is_file() => FsTargetKind::File,
        Some(metadata) if metadata.is_dir() => FsTargetKind::Directory,
        Some(_) => FsTargetKind::Other,
        None => FsTargetKind::Missing,
    };
    Ok(ResolvedFsTarget {
        requested_path: raw_path.to_string(),
        resolved_path,
        workspace_root,
        kind,
        metadata,
    })
}

pub(crate) fn workspace_root_for_path(context: &SessionContext, path: &Path) -> Option<PathBuf> {
    session_workspace_roots(context)
        .into_iter()
        .find(|root| path == root || path.starts_with(root))
}

fn lexically_normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn ensure_path_allowed_for_session(context: &SessionContext, path: &Path) -> Result<()> {
    let roots = session_workspace_roots(context);
    for root in &roots {
        if path != root && !path.starts_with(root) {
            continue;
        }
        let Ok(canonical_root) = std::fs::canonicalize(root) else {
            continue;
        };
        // A not-yet-created target is checked through its closest existing
        // ancestor. This also detects symlinked parents before a write creates
        // the final path; callers must still apply their own effect-time policy.
        let mut existing = path;
        while !existing.exists() {
            let Some(parent) = existing.parent() else {
                break;
            };
            existing = parent;
        }
        if std::fs::canonicalize(existing)
            .is_ok_and(|resolved| resolved.starts_with(&canonical_root))
        {
            return Ok(());
        }
    }
    Err(anyhow!(
        "tool path {} is outside the ACP session workspace roots or crosses a symlink boundary",
        path.display()
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> SessionContext {
        SessionContext {
            cwd: "/workspace".to_string(),
            roots: vec!["/workspace".to_string()],
            ..Default::default()
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bear-armature-paths-{label}-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn resolve_requested_tool_path_accepts_workspace_relative_paths() {
        let path = resolve_requested_tool_path(&context(), "README.md").unwrap();
        assert_eq!(path, PathBuf::from("/workspace/README.md"));
    }

    #[test]
    fn resolve_requested_tool_path_normalizes_dot_segments_before_root_check() {
        let ctx = context();
        let path = resolve_requested_tool_path(&ctx, "docs/../README.md").unwrap();
        assert_eq!(path, PathBuf::from("/workspace/README.md"));
        ensure_path_allowed_for_session(&ctx, &path).unwrap();

        let escaped = resolve_requested_tool_path(&ctx, "../etc/passwd").unwrap();
        assert!(!escaped.starts_with("/workspace"));
        assert!(ensure_path_allowed_for_session(&ctx, &escaped).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn session_root_rejects_symlinks_that_escape_to_another_directory() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink-root");
        let outside = temp_root("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "outside").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        let context = SessionContext {
            cwd: root.to_string_lossy().to_string(),
            roots: vec![root.to_string_lossy().to_string()],
            ..Default::default()
        };
        assert!(
            ensure_path_allowed_for_session(&context, &root.join("linked/secret.txt")).is_err()
        );
        assert!(resolve_fs_target(&context, "linked/secret.txt").is_err());
        assert!(ensure_path_allowed_for_session(&context, &root.join("linked/new.txt")).is_err());
        std::fs::write(root.join("allowed.txt"), "inside").unwrap();
        assert!(resolve_fs_target(&context, "allowed.txt").is_ok());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn resolve_fs_target_classifies_file_directory_and_missing_targets() {
        let root = temp_root("target-kind");
        let file = root.join("file.txt");
        std::fs::write(&file, "hello").unwrap();
        let dir = root.join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = SessionContext {
            cwd: root.to_string_lossy().to_string(),
            roots: vec![root.to_string_lossy().to_string()],
            ..Default::default()
        };

        let file_target = resolve_fs_target(&ctx, "file.txt").unwrap();
        assert_eq!(file_target.kind, FsTargetKind::File);
        assert_eq!(file_target.requested_path, "file.txt");
        assert_eq!(file_target.workspace_root, root);
        assert_eq!(file_target.size_bytes(), Some(5));

        let dir_target = resolve_fs_target(&ctx, "dir").unwrap();
        assert_eq!(dir_target.kind, FsTargetKind::Directory);
        assert!(dir_target.is_directory());

        let missing = resolve_fs_target(&ctx, "missing.txt").unwrap();
        assert_eq!(missing.kind, FsTargetKind::Missing);

        std::fs::remove_dir_all(&ctx.cwd).unwrap();
    }
}
