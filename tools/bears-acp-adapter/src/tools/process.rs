use crate::{
    paths::{
        ensure_path_allowed_for_session, is_absolute_local_path, normalize_requested_tool_path,
    },
    SessionContext, ToolPolicy,
};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, fmt, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub(crate) async fn handle_process_run(
    context: &SessionContext,
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("process_run args missing command"))?;
    validate_command(command)?;
    let cwd_raw = args
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("process_run args missing cwd"))?;
    let cwd = normalize_requested_tool_path(cwd_raw)?;
    ensure_path_allowed_for_session(context, &cwd)?;
    if !cwd.is_dir() {
        return Err(anyhow!("process_run cwd must be an existing directory"));
    }
    let command_args = args
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| anyhow!("process_run args entries must be strings"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    for arg in &command_args {
        if arg.contains('\0') {
            return Err(anyhow!("process_run args must not contain NUL bytes"));
        }
    }
    let policy_timeout_ms = policy.total_timeout_ms.unwrap_or(120_000).clamp(1, 120_000);
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000)
        .clamp(1, policy_timeout_ms);
    let policy_max_output = policy.max_bytes.unwrap_or(65_536).clamp(1, 1_048_576) as usize;
    let max_output_bytes = args
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_output as u64) as usize)
        .unwrap_or(policy_max_output);
    let env = parse_env(args)?;

    let started = std::time::Instant::now();
    let mut child = Command::new(command)
        .args(&command_args)
        .current_dir(&cwd)
        .envs(env.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn process_run command {command:?}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("process_run missing stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("process_run missing stderr pipe"))?;
    let stdout_task = tokio::spawn(read_limited(stdout, max_output_bytes));
    let stderr_task = tokio::spawn(read_limited(stderr, max_output_bytes));
    let status = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(status) => {
            status.with_context(|| format!("wait for process_run command {command:?}"))?
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let stdout_result = stdout_task
                .await
                .unwrap_or_else(|err| Err(anyhow!("stdout task failed: {err}")))?;
            let stderr_result = stderr_task
                .await
                .unwrap_or_else(|err| Err(anyhow!("stderr task failed: {err}")))?;
            let stdout_text = String::from_utf8_lossy(&stdout_result.bytes).to_string();
            let stderr_text = String::from_utf8_lossy(&stderr_result.bytes).to_string();
            let result = ProcessResult {
                exit_code: None,
                timed_out: true,
                elapsed_ms: started.elapsed().as_millis(),
                stdout: &stdout_text,
                stderr: &stderr_text,
                truncated: stdout_result.truncated || stderr_result.truncated,
                timeout_ms: Some(timeout_ms),
            };
            let content =
                process_result_content(command, &command_args, &cwd.to_string_lossy(), &result);
            return Ok(json!({
                "ok": false,
                "command": command,
                "args": command_args,
                "cwd": cwd.to_string_lossy(),
                "exit_code": null,
                "stdout": stdout_text,
                "stderr": stderr_text,
                "stdout_truncated": stdout_result.truncated,
                "stderr_truncated": stderr_result.truncated,
                "truncated": stdout_result.truncated || stderr_result.truncated,
                "timed_out": true,
                "elapsed_ms": started.elapsed().as_millis(),
                "source": "adapter_local",
                "content": content,
                "policy": { "timeout_ms": timeout_ms, "max_output_bytes": max_output_bytes }
            }));
        }
    };
    let stdout_result = stdout_task
        .await
        .unwrap_or_else(|err| Err(anyhow!("stdout task failed: {err}")))?;
    let stderr_result = stderr_task
        .await
        .unwrap_or_else(|err| Err(anyhow!("stderr task failed: {err}")))?;
    let stdout_text = String::from_utf8_lossy(&stdout_result.bytes).to_string();
    let stderr_text = String::from_utf8_lossy(&stderr_result.bytes).to_string();
    let exit_code = status.code();
    let ok = status.success();
    let result = ProcessResult {
        exit_code: exit_code.map(|code| code as i64),
        timed_out: false,
        elapsed_ms: started.elapsed().as_millis(),
        stdout: &stdout_text,
        stderr: &stderr_text,
        truncated: stdout_result.truncated || stderr_result.truncated,
        timeout_ms: None,
    };
    let content = process_result_content(command, &command_args, &cwd.to_string_lossy(), &result);
    eprintln!(
        "bears-acp-adapter: process_run session_id={} command={} args={} cwd={} exit_code={:?} timed_out=false duration_ms={}",
        session_id,
        command,
        command_args.len(),
        cwd.display(),
        exit_code,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": ok,
        "command": command,
        "args": command_args,
        "cwd": cwd.to_string_lossy(),
        "exit_code": exit_code,
        "stdout": stdout_text,
        "stderr": stderr_text,
        "stdout_truncated": stdout_result.truncated,
        "stderr_truncated": stderr_result.truncated,
        "truncated": stdout_result.truncated || stderr_result.truncated,
        "timed_out": false,
        "elapsed_ms": started.elapsed().as_millis(),
        "source": "adapter_local",
        "content": content,
        "policy": { "timeout_ms": timeout_ms, "max_output_bytes": max_output_bytes }
    }))
}

fn output_excerpt(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        raw.to_string()
    } else {
        let omitted = raw.chars().count().saturating_sub(max_chars);
        format!(
            "{}\n... truncated, omitted {omitted} characters",
            raw.chars().take(max_chars).collect::<String>()
        )
    }
}

struct ProcessResult<'a> {
    exit_code: Option<i64>,
    timed_out: bool,
    elapsed_ms: u128,
    stdout: &'a str,
    stderr: &'a str,
    truncated: bool,
    timeout_ms: Option<u64>,
}

impl fmt::Display for ProcessResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.timed_out {
            "timed out".to_string()
        } else if let Some(code) = self.exit_code {
            format!("exit_code={code}")
        } else {
            "unknown status".to_string()
        };
        let timeout_line = self
            .timeout_ms
            .map(|timeout| format!("timeout_ms: {timeout}\n"))
            .unwrap_or_default();
        write!(
            f,
            "status: {status}\n{timeout_line}elapsed_ms: {}\ntruncated: {}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
            self.elapsed_ms,
            self.truncated,
            output_excerpt(self.stdout, 16 * 1024),
            output_excerpt(self.stderr, 16 * 1024),
        )
    }
}

fn process_result_content(
    command: &str,
    args: &[String],
    cwd: &str,
    result: &ProcessResult<'_>,
) -> String {
    format!(
        "Process command: {}{}\ncwd: {cwd}\n{}",
        command,
        if args.is_empty() {
            String::new()
        } else {
            format!(" {}", args.join(" "))
        },
        result,
    )
}

struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_limited<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<LimitedOutput> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }
        let take = remaining.min(n);
        bytes.extend_from_slice(&buf[..take]);
        if take < n {
            truncated = true;
        }
    }
    Ok(LimitedOutput { bytes, truncated })
}

fn parse_env(args: &Value) -> Result<HashMap<String, String>> {
    let Some(obj) = args.get("env").and_then(Value::as_object) else {
        return Ok(HashMap::new());
    };
    let mut env = HashMap::new();
    for (key, value) in obj {
        validate_env_key(key)?;
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("process_run env values must be strings"))?;
        if key.to_ascii_uppercase().contains("SECRET")
            || key.to_ascii_uppercase().contains("TOKEN")
            || key.to_ascii_uppercase().contains("PASSWORD")
            || key.to_ascii_uppercase().contains("KEY")
        {
            return Err(anyhow!("process_run env key {key:?} looks secret-like"));
        }
        env.insert(key.clone(), value.to_string());
    }
    Ok(env)
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow!(
            "process_run env keys must be non-empty ASCII alphanumeric/underscore"
        ));
    }
    Ok(())
}

fn validate_command(command: &str) -> Result<()> {
    if command.contains('\0') || command.contains('/') && !is_absolute_local_path(command) {
        return Err(anyhow!(
            "process_run command must be an executable name or absolute path"
        ));
    }
    if command.contains(' ') || command.contains('\t') || command.contains('\n') {
        return Err(anyhow!(
            "process_run command must not be a shell command string"
        ));
    }
    let denied = ["sudo", "su", "rm", "shutdown", "reboot"];
    if denied.contains(&command) {
        return Err(anyhow!(
            "process_run command {command:?} is denied by adapter policy"
        ));
    }
    Ok(())
}
