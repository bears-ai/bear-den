use crate::{
    paths::{
        ensure_path_allowed_for_session, is_hidden_path_component, is_sensitive_path,
        normalize_requested_tool_path, session_workspace_roots,
    },
    truncate_for_log, SessionContext, ToolPolicy,
};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub(crate) async fn handle_read_text_file(
    context: &SessionContext,
    session_id: &str,
    params: Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("bears/read_text_file params missing path"))?;
    let line = params
        .get("line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let policy_max_lines = policy.max_lines.unwrap_or(2_000).clamp(1, 2_000);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_lines as u64) as usize)
        .unwrap_or(400.min(policy_max_lines));
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let raw = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read text file {}", path.display()))?;
    let total_lines = raw.lines().count();
    let selected: Vec<&str> = raw
        .lines()
        .skip(line.saturating_sub(1))
        .take(limit)
        .collect();
    let truncated = line.saturating_sub(1) + selected.len() < total_lines;
    let mut content = selected.join("\n");
    if raw.ends_with('\n') && !content.is_empty() && !truncated {
        content.push('\n');
    }
    eprintln!(
        "bears-acp-adapter: read_text_file session_id={} path={} line={} limit={} bytes={} total_lines={} returned_lines={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        line,
        limit,
        raw.len(),
        total_lines,
        selected.len(),
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "content": content,
        "line": line,
        "returned_lines": selected.len(),
        "total_lines": total_lines,
        "truncated": truncated,
        "bytes": raw.len(),
        "policy": {
            "max_lines": policy_max_lines,
            "applied_limit": limit,
        },
    }))
}

pub(crate) async fn handle_list_directory(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_list_directory args missing path"))?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .or(policy.recursive_default)
        .unwrap_or(false);
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .or(policy.include_hidden_default)
        .unwrap_or(false);
    let policy_max_entries = policy.max_entries.unwrap_or(1_000).clamp(1, 1_000);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_entries as u64) as usize)
        .unwrap_or(200.min(policy_max_entries));
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let mut entries = Vec::new();
    let mut total_entries_seen = 0usize;
    let mut truncated = false;
    let mut queue = VecDeque::from([path.clone()]);
    while let Some(dir) = queue.pop_front() {
        ensure_path_allowed_for_session(context, &dir)?;
        let mut dir_entries = fs::read_dir(&dir)
            .with_context(|| format!("list directory {}", dir.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        dir_entries.sort_by_key(|entry| entry.path());
        for entry in dir_entries {
            let entry_path = entry.path();
            if !include_hidden && is_hidden_path_component(&entry_path, &path) {
                continue;
            }
            ensure_path_allowed_for_session(context, &entry_path)?;
            total_entries_seen += 1;
            let metadata = entry.metadata().ok();
            let kind = metadata
                .as_ref()
                .map(|m| {
                    if m.is_dir() {
                        "directory"
                    } else if m.is_file() {
                        "file"
                    } else {
                        "other"
                    }
                })
                .unwrap_or("unknown");
            if entries.len() < limit {
                entries.push(json!({
                    "path": entry_path.to_string_lossy(),
                    "name": entry.file_name().to_string_lossy(),
                    "kind": kind,
                    "size": metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                }));
            } else {
                truncated = true;
                break;
            }
            if recursive && metadata.as_ref().is_some_and(|m| m.is_dir()) {
                queue.push_back(entry_path);
            }
        }
    }
    let truncated = truncated || total_entries_seen > entries.len() || !queue.is_empty();
    let content = format_directory_listing(&path, &entries, truncated);
    eprintln!(
        "bears-acp-adapter: list_directory session_id={} path={} recursive={} include_hidden={} limit={} returned_entries={} total_entries_seen={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        recursive,
        include_hidden,
        limit,
        entries.len(),
        total_entries_seen,
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "entries": entries,
        "total_entries_seen": total_entries_seen,
        "returned_entries": entries.len(),
        "truncated": truncated,
        "recursive": recursive,
        "include_hidden": include_hidden,
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_entries": policy_max_entries,
            "applied_limit": limit,
            "recursive_default": policy.recursive_default,
            "include_hidden_default": policy.include_hidden_default,
        },
    }))
}

pub(crate) async fn handle_find_paths(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let glob = args
        .get("glob")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("fs_find_paths args missing glob"))?;
    let root = args
        .get("root")
        .and_then(Value::as_str)
        .map(normalize_requested_tool_path)
        .transpose()?
        .unwrap_or_else(|| session_workspace_roots(context)[0].clone());
    ensure_path_allowed_for_session(context, &root)?;
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .or(policy.include_hidden_default)
        .unwrap_or(false);
    let policy_max_results = policy.max_results.unwrap_or(500).clamp(1, 500);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_results as u64) as usize)
        .unwrap_or(100.min(policy_max_results));
    let started = std::time::Instant::now();
    let mut matches = Vec::new();
    let mut visited = 0usize;
    let mut skipped_by_filter = 0usize;
    let mut truncated = false;
    collect_find_paths(
        context,
        &root,
        &root,
        glob,
        include_hidden,
        limit,
        &mut visited,
        &mut skipped_by_filter,
        &mut truncated,
        &mut matches,
    )?;
    matches.sort();
    let content = format_find_path_results(glob, &matches, truncated);
    eprintln!(
        "bears-acp-adapter: find_paths session_id={} root={} glob={} limit={} matches={} visited={} truncated={} duration_ms={}",
        session_id,
        root.display(),
        glob,
        limit,
        matches.len(),
        visited,
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "root": root.to_string_lossy(),
        "glob": glob,
        "matches": matches.iter().map(|path| json!({
            "path": path.to_string_lossy(),
            "relative_path": path.strip_prefix(&root).unwrap_or(path).to_string_lossy(),
        })).collect::<Vec<_>>(),
        "returned_matches": matches.len(),
        "visited": visited,
        "skipped_by_filter": skipped_by_filter,
        "truncated": truncated,
        "include_hidden": include_hidden,
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_results": policy_max_results,
            "applied_limit": limit,
            "include_hidden_default": policy.include_hidden_default,
        },
    }))
}

pub(crate) async fn handle_search_files(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_search_files args missing path"))?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let policy_max_results = policy.max_results.unwrap_or(200).clamp(1, 200);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_results as u64) as usize)
        .unwrap_or(50.min(policy_max_results));
    let policy_max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_bytes))
        .unwrap_or(policy_max_bytes);
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .or(policy.include_hidden_default)
        .unwrap_or(false);
    let filters = search_filters_from_args(args)?;
    let path = normalize_requested_tool_path(path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let started = std::time::Instant::now();
    let mut files = Vec::new();
    let mut file_collection_truncated = false;
    let mut skipped_by_filter = 0usize;
    collect_search_files(
        context,
        &path,
        &path,
        include_hidden,
        &filters,
        5_000,
        &mut file_collection_truncated,
        &mut skipped_by_filter,
        &mut files,
    )?;
    files.sort();

    let mut matches = Vec::new();
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0u64;
    let mut truncated = file_collection_truncated;
    for file in files {
        ensure_path_allowed_for_session(context, &file)?;
        let metadata = match fs::metadata(&file) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        if bytes_scanned.saturating_add(metadata.len()) > max_bytes {
            truncated = true;
            break;
        }
        let raw = match fs::read_to_string(&file) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        bytes_scanned = bytes_scanned.saturating_add(metadata.len());
        files_scanned += 1;
        if query.is_empty() {
            matches.push(json!({
                "path": file.to_string_lossy(),
                "line": null,
                "preview": file
                    .strip_prefix(&path)
                    .unwrap_or(&file)
                    .to_string_lossy(),
                "match_type": "path",
            }));
            if matches.len() >= limit {
                truncated = true;
                break;
            }
        } else {
            for (idx, line) in raw.lines().enumerate() {
                if line_matches_query(line, query, filters.case_sensitive) {
                    matches.push(json!({
                        "path": file.to_string_lossy(),
                        "line": idx + 1,
                        "preview": truncate_for_log(line.trim(), 240),
                        "match_type": "content",
                    }));
                    if matches.len() >= limit {
                        truncated = true;
                        break;
                    }
                }
            }
            if matches.len() >= limit {
                break;
            }
        }
    }
    let content = format_search_results(query, &matches, truncated);
    eprintln!(
        "bears-acp-adapter: search_files session_id={} path={} query_len={} limit={} max_bytes={} files_scanned={} bytes_scanned={} matches={} truncated={} duration_ms={}",
        session_id,
        path.display(),
        query.len(),
        limit,
        max_bytes,
        files_scanned,
        bytes_scanned,
        matches.len(),
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "query": query,
        "matches": matches,
        "returned_matches": matches.len(),
        "truncated": truncated,
        "files_scanned": files_scanned,
        "bytes_scanned": bytes_scanned,
        "max_bytes": max_bytes,
        "include_hidden": include_hidden,
        "case_sensitive": filters.case_sensitive,
        "pattern": filters.pattern,
        "extensions": filters.extensions,
        "skipped_by_filter": skipped_by_filter,
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_results": policy_max_results,
            "applied_limit": limit,
            "max_bytes": policy_max_bytes,
            "applied_max_bytes": max_bytes,
            "include_hidden_default": policy.include_hidden_default,
        },
    }))
}

pub(crate) async fn handle_stat(
    context: &SessionContext,
    args: &Value,
    _policy: &ToolPolicy,
) -> Result<Value> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_stat args missing path"))?;
    let include_symlink_target = args
        .get("include_symlink_target")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let path = normalize_requested_tool_path(raw_path)?;
    ensure_path_allowed_for_session(context, &path)?;
    let metadata =
        fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    };
    let modified_at_unix_secs = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let symlink_target = if include_symlink_target && file_type.is_symlink() {
        fs::read_link(&path)
            .ok()
            .map(|target| target.to_string_lossy().to_string())
    } else {
        None
    };
    let content = format!("{}\t{}\t{} bytes", kind, path.display(), metadata.len());
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "exists": true,
        "kind": kind,
        "size_bytes": metadata.len(),
        "readonly": metadata.permissions().readonly(),
        "modified_at_unix_secs": modified_at_unix_secs,
        "symlink_target": symlink_target,
        "source": "adapter_local",
        "content": content,
    }))
}

pub(crate) async fn handle_replace_text(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let args = ReplaceTextArgs::from_value(args, policy)?;
    let started = std::time::Instant::now();
    let plan = ReplaceTextPlan::preflight(context, args, policy)?;
    let applied = plan.apply(context, policy)?;
    eprintln!(
        "bears-acp-adapter: replace_text session_id={} path={} bytes_before={} bytes_after={} duration_ms={}",
        session_id,
        applied["path"].as_str().unwrap_or(""),
        applied["bytes_before"].as_u64().unwrap_or(0),
        applied["bytes_after"].as_u64().unwrap_or(0),
        started.elapsed().as_millis(),
    );
    Ok(applied)
}

pub(crate) async fn handle_create_text_file(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_create_text_file args missing path"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_create_text_file args missing content"))?;
    if args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(anyhow!(
            "fs_create_text_file does not support overwrite yet"
        ));
    }
    let create_parent_dirs = args
        .get("create_parent_dirs")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
    if content.len() as u64 > max_bytes {
        return Err(anyhow!(
            "fs_create_text_file content exceeds policy max_bytes: {} > {}",
            content.len(),
            max_bytes
        ));
    }
    let path = normalize_requested_tool_path(raw_path)?;
    ensure_path_allowed_for_session(context, &path)?;
    ensure_replace_text_path_allowed(&path, policy)?;
    if path.exists() {
        return Err(anyhow!("fs_create_text_file path already exists"));
    }
    if let Some(parent) = path.parent() {
        ensure_path_allowed_for_session(context, parent)?;
        if !parent.exists() {
            if create_parent_dirs {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create parent directories {}", parent.display()))?;
            } else {
                return Err(anyhow!(
                    "fs_create_text_file parent directory does not exist; set create_parent_dirs=true"
                ));
            }
        }
    }
    let started = std::time::Instant::now();
    fs::write(&path, content.as_bytes())
        .with_context(|| format!("create text file {}", path.display()))?;
    let result = json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "bytes": content.len(),
        "created": true,
        "source": "adapter_local",
        "content": format!("Created text file {} ({} bytes)", path.display(), content.len()),
        "policy": {
            "max_bytes": max_bytes,
            "create_files": policy.create_files.unwrap_or(true),
            "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            "sensitive_path_policy": policy.sensitive_path_policy,
        }
    });
    eprintln!(
        "bears-acp-adapter: create_text_file session_id={} path={} bytes={} duration_ms={}",
        session_id,
        path.display(),
        content.len(),
        started.elapsed().as_millis(),
    );
    Ok(result)
}

pub(crate) async fn handle_delete_path(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let raw_path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fs_delete_path args missing path"))?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .or(policy.recursive_default)
        .unwrap_or(false);
    let allow_missing = args
        .get("allow_missing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expected_kind = args
        .get("expected_kind")
        .and_then(Value::as_str)
        .unwrap_or("any");
    let max_entries = policy.max_entries.unwrap_or(100).clamp(1, 1_000);
    let path = normalize_requested_tool_path(raw_path)?;
    ensure_path_allowed_for_session(context, &path)?;
    ensure_delete_path_allowed(context, &path, policy)?;
    let started = std::time::Instant::now();
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && allow_missing => {
            return Ok(json!({
                "ok": true,
                "path": path.to_string_lossy(),
                "deleted": false,
                "missing": true,
                "source": "adapter_local",
                "content": format!("Path {} was already missing.", path.display()),
            }));
        }
        Err(err) => return Err(anyhow!(err).context(format!("stat {}", path.display()))),
    };
    let kind = if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "other"
    };
    if expected_kind != "any" && expected_kind != kind {
        return Err(anyhow!(
            "fs_delete_path expected kind {expected_kind}, found {kind}"
        ));
    }
    let mut entries = Vec::new();
    if metadata.is_dir() {
        collect_delete_entries(&path, &mut entries, max_entries + 1)?;
        if entries.len() > max_entries {
            return Err(anyhow!(
                "fs_delete_path directory has more than policy max_entries={max_entries}"
            ));
        }
        if !recursive && !entries.is_empty() {
            return Err(anyhow!(
                "fs_delete_path requires recursive=true for non-empty directories"
            ));
        }
        if recursive {
            fs::remove_dir_all(&path)
                .with_context(|| format!("delete directory {}", path.display()))?;
        } else {
            fs::remove_dir(&path)
                .with_context(|| format!("delete directory {}", path.display()))?;
        }
    } else if metadata.is_file() {
        fs::remove_file(&path).with_context(|| format!("delete file {}", path.display()))?;
    } else {
        return Err(anyhow!("fs_delete_path only deletes files or directories"));
    }
    let content = format!(
        "Deleted {kind} {}{}",
        path.display(),
        if metadata.is_dir() {
            format!(" ({} entries)", entries.len())
        } else {
            String::new()
        }
    );
    eprintln!(
        "bears-acp-adapter: delete_path session_id={} path={} kind={} recursive={} entries={} duration_ms={}",
        session_id,
        path.display(),
        kind,
        recursive,
        entries.len(),
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "deleted": true,
        "kind": kind,
        "recursive": recursive,
        "entries": entries.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "source": "adapter_local",
        "content": content,
        "policy": {
            "max_entries": max_entries,
            "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            "sensitive_path_policy": policy.sensitive_path_policy,
        }
    }))
}

#[derive(Clone, Debug)]
pub(crate) struct ReplaceTextArgs {
    pub(crate) path: String,
    pub(crate) old_text: String,
    pub(crate) new_text: String,
    pub(crate) expected_replacements: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ReplaceTextPlan {
    pub(crate) args: ReplaceTextArgs,
    pub(crate) path: PathBuf,
    replacements: usize,
    bytes_before: usize,
    bytes_after: usize,
    pub(crate) preview: String,
    policy_max_bytes: u64,
    policy_max_replacements: usize,
    policy_create_files: bool,
    policy_allow_multiple: bool,
}

impl ReplaceTextArgs {
    pub(crate) fn from_value(args: &Value, policy: &ToolPolicy) -> Result<Self> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing path"))?
            .to_string();
        let old_text = args
            .get("old_text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing old_text"))?
            .to_string();
        let new_text = args
            .get("new_text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs_replace_text args missing new_text"))?
            .to_string();
        if old_text.is_empty() {
            return Err(anyhow!("fs_replace_text old_text must not be empty"));
        }
        let create_if_missing = args
            .get("create_if_missing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let policy_create_files = policy.create_files.unwrap_or(false);
        if create_if_missing && !policy_create_files {
            return Err(anyhow!("fs_replace_text does not create files yet"));
        }
        let allow_multiple = args
            .get("allow_multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let policy_allow_multiple = policy.allow_multiple.unwrap_or(false);
        if allow_multiple && !policy_allow_multiple {
            return Err(anyhow!(
                "fs_replace_text does not allow multiple replacements yet"
            ));
        }
        let expected_replacements = args
            .get("expected_replacements")
            .and_then(Value::as_u64)
            .unwrap_or(1) as usize;
        let policy_max_replacements = policy.max_replacements.unwrap_or(1).clamp(1, 100);
        if expected_replacements == 0 || expected_replacements > policy_max_replacements {
            return Err(anyhow!(
                "fs_replace_text expected_replacements exceeds policy max_replacements={policy_max_replacements}"
            ));
        }
        if expected_replacements != 1 || allow_multiple || policy_allow_multiple {
            return Err(anyhow!(
                "fs_replace_text currently supports exactly one replacement"
            ));
        }
        Ok(Self {
            path,
            old_text,
            new_text,
            expected_replacements,
        })
    }
}

impl ReplaceTextPlan {
    pub(crate) fn preflight(
        context: &SessionContext,
        args: ReplaceTextArgs,
        policy: &ToolPolicy,
    ) -> Result<Self> {
        let policy_max_bytes = policy.max_bytes.unwrap_or(1_048_576).clamp(1, 5_242_880);
        let policy_max_replacements = policy.max_replacements.unwrap_or(1).clamp(1, 100);
        let policy_create_files = policy.create_files.unwrap_or(false);
        let policy_allow_multiple = policy.allow_multiple.unwrap_or(false);
        let path = normalize_requested_tool_path(&args.path)?;
        ensure_path_allowed_for_session(context, &path)?;
        ensure_replace_text_path_allowed(&path, policy)?;
        let raw = read_replace_text_input(&path, policy_max_bytes)?;
        let replacements = raw.matches(&args.old_text).count();
        if replacements != args.expected_replacements {
            return Err(anyhow!(
                "fs_replace_text expected {} match for old_text, found {replacements}",
                args.expected_replacements
            ));
        }
        let updated = raw.replacen(&args.old_text, &args.new_text, args.expected_replacements);
        let preview = replace_text_preview(&path, &args, &raw, &updated);
        Ok(Self {
            args,
            path,
            replacements,
            bytes_before: raw.len(),
            bytes_after: updated.len(),
            preview,
            policy_max_bytes,
            policy_max_replacements,
            policy_create_files,
            policy_allow_multiple,
        })
    }

    pub(crate) fn apply(&self, context: &SessionContext, policy: &ToolPolicy) -> Result<Value> {
        ensure_path_allowed_for_session(context, &self.path)?;
        ensure_replace_text_path_allowed(&self.path, policy)?;
        let raw = read_replace_text_input(&self.path, self.policy_max_bytes)?;
        let replacements = raw.matches(&self.args.old_text).count();
        if replacements != self.args.expected_replacements {
            return Err(anyhow!(
                "fs_replace_text stale preflight: expected {} match for old_text, found {replacements}",
                self.args.expected_replacements
            ));
        }
        let updated = raw.replacen(
            &self.args.old_text,
            &self.args.new_text,
            self.args.expected_replacements,
        );
        fs::write(&self.path, updated.as_bytes())
            .with_context(|| format!("write replaced text file {}", self.path.display()))?;
        let content = format!(
            "Replaced {} occurrence in {} ({} bytes -> {} bytes)",
            self.replacements,
            self.path.display(),
            raw.len(),
            updated.len()
        );
        Ok(json!({
            "ok": true,
            "path": self.path.to_string_lossy(),
            "replacements": self.replacements,
            "bytes_before": raw.len(),
            "bytes_after": updated.len(),
            "source": "adapter_local",
            "content": content,
            "preview": self.preview,
            "policy": {
                "max_bytes": self.policy_max_bytes,
                "sensitive_path_policy": policy.sensitive_path_policy,
                "max_replacements": self.policy_max_replacements,
                "create_files": self.policy_create_files,
                "allow_multiple": self.policy_allow_multiple,
                "deny_hidden_paths": policy.deny_hidden_paths.unwrap_or(true),
            },
        }))
    }

    pub(crate) fn permission_summary(&self, tool_name: &str, reason: &str) -> String {
        format!(
            "{reason}\n\nTool: {tool_name}\nPath: {}\nReplacing {} occurrence\nBytes: {} -> {}\n\nReview the diff below before approving.",
            self.path.display(),
            self.replacements,
            self.bytes_before,
            self.bytes_after,
        )
    }

    #[cfg(test)]
    pub(crate) fn permission_prompt(&self, tool_name: &str, reason: &str) -> String {
        format!(
            "{}\n\n{}",
            self.permission_summary(tool_name, reason),
            self.preview
        )
    }
}

fn read_replace_text_input(path: &Path, policy_max_bytes: u64) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!("fs_replace_text path must be an existing file"));
    }
    if metadata.len() > policy_max_bytes {
        return Err(anyhow!(
            "fs_replace_text file exceeds policy max_bytes: {} > {}",
            metadata.len(),
            policy_max_bytes
        ));
    }
    fs::read_to_string(path)
        .with_context(|| format!("read text file for replace {}", path.display()))
}

fn replace_text_preview(path: &Path, args: &ReplaceTextArgs, raw: &str, updated: &str) -> String {
    let before = preview_around(raw, &args.old_text, 320);
    let after = preview_around(updated, &args.new_text, 320);
    format!(
        "Preview for {}\n--- before\n{}\n+++ after\n{}",
        path.display(),
        before,
        after
    )
}

fn preview_around(text: &str, needle: &str, max_chars: usize) -> String {
    let Some(pos) = text.find(needle) else {
        return truncate_chars(text, max_chars);
    };
    let before = truncate_chars_reverse(&text[..pos], max_chars / 2);
    let after_start = pos.saturating_add(needle.len()).min(text.len());
    let after = truncate_chars(&text[after_start..], max_chars / 2);
    let mut out = String::new();
    if before.chars().count() < text[..pos].chars().count() {
        out.push_str("...");
    }
    out.push_str(&before);
    out.push_str(needle);
    out.push_str(&after);
    if after.chars().count() < text[after_start..].chars().count() {
        out.push_str("...");
    }
    out
}

fn truncate_chars_reverse(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        text.to_string()
    } else {
        text.chars().skip(count - max_chars).collect()
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out = text.chars().take(max_chars).collect::<String>();
        out.push_str("...");
        out
    }
}

fn ensure_delete_path_allowed(
    context: &SessionContext,
    path: &Path,
    policy: &ToolPolicy,
) -> Result<()> {
    let roots = session_workspace_roots(context);
    if roots.iter().any(|root| path == root) {
        return Err(anyhow!("fs_delete_path refuses to delete a workspace root"));
    }
    if path.parent().is_none() {
        return Err(anyhow!("fs_delete_path refuses to delete filesystem root"));
    }
    if policy.sensitive_path_policy.as_deref() == Some("deny_sensitive_paths")
        && is_sensitive_path(path)
    {
        return Err(anyhow!(
            "fs_delete_path denied sensitive path {}",
            path.display()
        ));
    }
    if policy.deny_hidden_paths.unwrap_or(true) && is_hidden_path_component(path, Path::new("/")) {
        return Err(anyhow!(
            "fs_delete_path denied hidden path {}",
            path.display()
        ));
    }
    Ok(())
}

fn collect_delete_entries(path: &Path, out: &mut Vec<PathBuf>, limit: usize) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    for entry in fs::read_dir(path).with_context(|| format!("scan directory {}", path.display()))? {
        let entry = entry?;
        let path = entry.path();
        out.push(path.clone());
        if out.len() >= limit {
            return Ok(());
        }
        if entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
            collect_delete_entries(&path, out, limit)?;
        }
    }
    Ok(())
}

pub(crate) fn ensure_replace_text_path_allowed(path: &Path, policy: &ToolPolicy) -> Result<()> {
    if policy.sensitive_path_policy.as_deref() == Some("deny_sensitive_paths")
        && is_sensitive_path(path)
    {
        return Err(anyhow!(
            "fs_replace_text denied sensitive path {}",
            path.display()
        ));
    }
    if policy.deny_hidden_paths.unwrap_or(true) && is_hidden_path_component(path, Path::new("/")) {
        return Err(anyhow!(
            "fs_replace_text denied hidden path {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct SearchFilters {
    case_sensitive: bool,
    pattern: Option<String>,
    extensions: Vec<String>,
}

fn collect_find_paths(
    context: &SessionContext,
    root: &Path,
    path: &Path,
    glob: &str,
    include_hidden: bool,
    limit: usize,
    visited: &mut usize,
    skipped_by_filter: &mut usize,
    truncated: &mut bool,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if *truncated {
        return Ok(());
    }
    if !include_hidden && is_hidden_path_component(path, root) {
        return Ok(());
    }
    ensure_path_allowed_for_session(context, path)?;
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if path != root {
        *visited += 1;
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if glob_match(glob, &relative) {
            if out.len() >= limit {
                *truncated = true;
                return Ok(());
            }
            out.push(path.to_path_buf());
        } else {
            *skipped_by_filter += 1;
        }
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("find paths in directory {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_find_paths(
            context,
            root,
            &entry.path(),
            glob,
            include_hidden,
            limit,
            visited,
            skipped_by_filter,
            truncated,
            out,
        )?;
        if *truncated {
            break;
        }
    }
    Ok(())
}

fn collect_search_files(
    context: &SessionContext,
    root: &Path,
    path: &Path,
    include_hidden: bool,
    filters: &SearchFilters,
    max_files: usize,
    truncated: &mut bool,
    skipped_by_filter: &mut usize,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if *truncated {
        return Ok(());
    }
    if !include_hidden && is_hidden_path_component(path, root) {
        return Ok(());
    }
    ensure_path_allowed_for_session(context, path)?;
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if metadata.is_file() {
        if !search_file_passes_filters(root, path, filters) {
            *skipped_by_filter += 1;
            return Ok(());
        }
        if out.len() >= max_files {
            *truncated = true;
        } else {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("search directory {}", path.display()))?
    {
        let entry = entry?;
        collect_search_files(
            context,
            root,
            &entry.path(),
            include_hidden,
            filters,
            max_files,
            truncated,
            skipped_by_filter,
            out,
        )?;
        if *truncated {
            break;
        }
    }
    Ok(())
}

fn search_filters_from_args(args: &Value) -> Result<SearchFilters> {
    let case_sensitive = args
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let extensions = args
        .get("extensions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_extension)
                .filter(|s| !s.is_empty())
                .take(10)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SearchFilters {
        case_sensitive,
        pattern,
        extensions,
    })
}

fn normalize_extension(raw: &str) -> String {
    raw.trim().trim_start_matches('.').to_ascii_lowercase()
}

fn search_file_passes_filters(root: &Path, path: &Path, filters: &SearchFilters) -> bool {
    if !filters.extensions.is_empty() {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();
        if !filters.extensions.iter().any(|allowed| allowed == &ext) {
            return false;
        }
    }
    if let Some(pattern) = filters.pattern.as_deref() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if !glob_match(pattern, &relative) {
            return false;
        }
    }
    true
}

fn line_matches_query(line: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        line.contains(query)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let text = text.replace('\\', "/");
    let pattern_parts = pattern.split('/').collect::<Vec<_>>();
    let text_parts = text.split('/').collect::<Vec<_>>();
    glob_match_parts(&pattern_parts, &text_parts)
}

fn glob_match_parts(pattern: &[&str], text: &[&str]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    if pattern[0] == "**" {
        if glob_match_parts(&pattern[1..], text) {
            return true;
        }
        return !text.is_empty() && glob_match_parts(pattern, &text[1..]);
    }
    !text.is_empty()
        && wildcard_segment_match(pattern[0], text[0])
        && glob_match_parts(&pattern[1..], &text[1..])
}

fn wildcard_segment_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut p = 0usize;
    let mut t = 0usize;
    let mut star = None;
    let mut match_after_star = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            match_after_star = t;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            match_after_star += 1;
            t = match_after_star;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn format_find_path_results(glob: &str, matches: &[PathBuf], truncated: bool) -> String {
    let mut lines = vec![format!("Path matches for {glob:?}")];
    for path in matches {
        lines.push(path.display().to_string());
    }
    if matches.is_empty() {
        lines.push("No paths found.".to_string());
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}

fn format_directory_listing(path: &Path, entries: &[Value], truncated: bool) -> String {
    let mut lines = vec![format!("Directory listing for {}", path.display())];
    for entry in entries {
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let entry_path = entry.get("path").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{kind}\t{entry_path}"));
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}

fn format_search_results(query: &str, matches: &[Value], truncated: bool) -> String {
    let mut lines = if query.is_empty() {
        vec!["Path search results".to_string()]
    } else {
        vec![format!("Search results for {query:?}")]
    };
    for item in matches {
        let path = item.get("path").and_then(Value::as_str).unwrap_or("");
        let preview = item.get("preview").and_then(Value::as_str).unwrap_or("");
        if let Some(line) = item.get("line").and_then(Value::as_u64) {
            lines.push(format!("{path}:{line}: {preview}"));
        } else if preview.is_empty() {
            lines.push(path.to_string());
        } else {
            lines.push(format!("{path}: {preview}"));
        }
    }
    if matches.is_empty() {
        lines.push("No matches found.".to_string());
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}
