use crate::{paths::{ensure_path_allowed_for_session, is_absolute_local_path, normalize_requested_tool_path, session_workspace_roots}, SessionContext, ToolPolicy};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, process::Command};

pub(crate) async fn handle_git_status(
    context: &SessionContext,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let repo = git_repo_path_from_args(context, args)?;
    let include_untracked = args
        .get("include_untracked")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut command_args = vec![
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "-b".to_string(),
    ];
    if !include_untracked {
        command_args.push("-uno".to_string());
    }
    let max_bytes = policy.max_bytes.unwrap_or(262_144).clamp(1, 1_048_576) as usize;
    let output = run_git_command(&repo, &command_args, max_bytes)?;
    let parsed = parse_git_status_porcelain(&output.stdout);
    let content = format_git_status_content(&parsed, output.truncated);
    Ok(json!({
        "ok": true,
        "repo_path": repo.to_string_lossy(),
        "branch": parsed.branch,
        "upstream": parsed.upstream,
        "ahead": parsed.ahead,
        "behind": parsed.behind,
        "clean": parsed.entries.is_empty(),
        "entries": parsed.entries,
        "include_untracked": include_untracked,
        "exit_code": output.exit_code,
        "stderr": output.stderr,
        "truncated": output.truncated,
        "source": "adapter_local",
        "content": content,
        "policy": { "max_bytes": max_bytes },
    }))
}

pub(crate) async fn handle_git_diff(
    context: &SessionContext,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let repo = git_repo_path_from_args(context, args)?;
    let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
    let policy_max_bytes = policy.max_bytes.unwrap_or(262_144).clamp(1, 1_048_576);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_bytes) as usize)
        .unwrap_or(policy_max_bytes as usize);
    let mut command_args = vec!["diff".to_string(), "--no-ext-diff".to_string()];
    if staged {
        command_args.push("--staged".to_string());
    }
    let paths = git_paths_from_args(context, &repo, args)?;
    if !paths.is_empty() {
        command_args.push("--".to_string());
        command_args.extend(paths.iter().map(|path| path.to_string_lossy().to_string()));
    }
    let output = run_git_command(&repo, &command_args, max_bytes)?;
    let content = if output.stdout.trim().is_empty() {
        "No git diff.".to_string()
    } else if output.truncated {
        format!("{}\n... truncated", output.stdout)
    } else {
        output.stdout.clone()
    };
    Ok(json!({
        "ok": true,
        "repo_path": repo.to_string_lossy(),
        "staged": staged,
        "paths": paths.iter().map(|path| path.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "diff": output.stdout,
        "exit_code": output.exit_code,
        "stderr": output.stderr,
        "truncated": output.truncated,
        "source": "adapter_local",
        "content": content,
        "policy": { "max_bytes": policy_max_bytes, "applied_max_bytes": max_bytes },
    }))
}

#[derive(Debug)]
struct GitCommandOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    truncated: bool,
}

#[derive(Debug)]
struct ParsedGitStatus {
    branch: Option<String>,
    upstream: Option<String>,
    ahead: Option<u32>,
    behind: Option<u32>,
    entries: Vec<Value>,
}

fn git_repo_path_from_args(context: &SessionContext, args: &Value) -> Result<PathBuf> {
    let requested = args
        .get("repo_path")
        .and_then(Value::as_str)
        .map(normalize_requested_tool_path)
        .transpose()?
        .unwrap_or_else(|| session_workspace_roots(context)[0].clone());
    ensure_path_allowed_for_session(context, &requested)?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&requested)
        .output()
        .with_context(|| format!("resolve git repository root for {}", requested.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} is not inside a git work tree: {}",
            requested.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let repo = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    ensure_path_allowed_for_session(context, &repo)?;
    Ok(repo)
}

fn git_paths_from_args(context: &SessionContext, repo: &Path, args: &Value) -> Result<Vec<PathBuf>> {
    let Some(paths) = args.get("paths").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for value in paths.iter().take(100) {
        let raw = value
            .as_str()
            .ok_or_else(|| anyhow!("git_diff paths entries must be strings"))?;
        let path = if is_absolute_local_path(raw) {
            normalize_requested_tool_path(raw)?
        } else {
            repo.join(raw)
        };
        ensure_path_allowed_for_session(context, &path)?;
        let relative = path.strip_prefix(repo).map(Path::to_path_buf).map_err(|_| {
            anyhow!(
                "git_diff path {} is outside repo {}",
                path.display(),
                repo.display()
            )
        })?;
        out.push(relative);
    }
    Ok(out)
}

fn run_git_command(repo: &Path, args: &[String], max_stdout_bytes: usize) -> Result<GitCommandOutput> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), repo.display()))?;
    let exit_code = output.status.code().unwrap_or(-1);
    let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let truncated = stdout.len() > max_stdout_bytes;
    if truncated {
        stdout.truncate(max_stdout_bytes);
    }
    let stderr = crate::truncate_for_log(&String::from_utf8_lossy(&output.stderr), 8_192);
    if !output.status.success() {
        return Err(anyhow!(
            "git {} failed with exit code {}: {}",
            args.join(" "),
            exit_code,
            stderr.trim()
        ));
    }
    Ok(GitCommandOutput {
        stdout,
        stderr,
        exit_code,
        truncated,
    })
}

fn parse_git_status_porcelain(raw: &str) -> ParsedGitStatus {
    let mut branch = None;
    let mut upstream = None;
    let mut ahead = None;
    let mut behind = None;
    let mut entries = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let mut parts = rest.splitn(2, "...");
            branch = parts.next().map(str::to_string).filter(|s| !s.is_empty());
            if let Some(upstream_part) = parts.next() {
                let mut upstream_part = upstream_part.to_string();
                if let Some(idx) = upstream_part.find(" [") {
                    let meta = upstream_part[idx + 2..].trim_end_matches(']');
                    for item in meta.split(',').map(str::trim) {
                        if let Some(n) = item.strip_prefix("ahead ") {
                            ahead = n.parse().ok();
                        } else if let Some(n) = item.strip_prefix("behind ") {
                            behind = n.parse().ok();
                        }
                    }
                    upstream_part.truncate(idx);
                }
                upstream = Some(upstream_part).filter(|s| !s.is_empty());
            }
            continue;
        }
        if line.len() >= 3 {
            let xy = &line[..2];
            let path = line[3..].to_string();
            entries.push(json!({
                "xy": xy,
                "index_status": &xy[0..1],
                "worktree_status": &xy[1..2],
                "path": path,
            }));
        }
    }
    ParsedGitStatus {
        branch,
        upstream,
        ahead,
        behind,
        entries,
    }
}

fn format_git_status_content(status: &ParsedGitStatus, truncated: bool) -> String {
    let mut lines = vec![format!(
        "On branch {}",
        status.branch.as_deref().unwrap_or("<unknown>")
    )];
    if let Some(upstream) = status.upstream.as_deref() {
        lines.push(format!("Upstream: {upstream}"));
    }
    if status.entries.is_empty() {
        lines.push("Working tree clean.".to_string());
    } else {
        for entry in &status.entries {
            lines.push(format!(
                "{} {}",
                entry.get("xy").and_then(Value::as_str).unwrap_or("??"),
                entry.get("path").and_then(Value::as_str).unwrap_or("")
            ));
        }
    }
    if truncated {
        lines.push("... truncated".to_string());
    }
    lines.join("\n")
}
