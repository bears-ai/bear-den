use crate::{
    client_supports_terminal,
    paths::ensure_path_allowed_for_session,
    send_terminal_tool_call_update,
    tools::{
        command_policy::{rtk_wrap_allowed, terminal_command_allowed},
        common::{command_line, output_excerpt, rtk_available},
        rtk::{reduce_with_rtk_summary, ReducerMode, RtkReduction},
    },
    AdapterState, CreateTerminalRequest, CreateTerminalResponse, EnvVariable,
    ReleaseTerminalRequest, Result, SessionContext, TerminalOutputRequest, TerminalOutputResponse,
    ToolPolicy, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{fmt, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCommandValidation {
    Allowlisted,
    Generic,
}

struct TerminalResult<'a> {
    exit_code: Option<i64>,
    signal: Option<&'a str>,
    timed_out: bool,
    elapsed_ms: u128,
    output: &'a str,
    truncated: bool,
    timeout_ms: Option<u64>,
}

impl fmt::Display for TerminalResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.timed_out {
            "timed out".to_string()
        } else if let Some(code) = self.exit_code {
            format!("exit_code={code}")
        } else if let Some(signal) = self.signal {
            format!("signal={signal}")
        } else {
            "unknown status".to_string()
        };
        let timeout_line = self
            .timeout_ms
            .map(|timeout| format!("timeout_ms: {timeout}\n"))
            .unwrap_or_default();
        let output = output_excerpt(self.output, 32 * 1024);
        write!(
            f,
            "status: {status}\n{timeout_line}elapsed_ms: {}\ntruncated: {}\n\nOutput:\n{output}",
            self.elapsed_ms, self.truncated,
        )
    }
}

fn terminal_result_content(
    command: &str,
    args: &[String],
    cwd: &str,
    result: &TerminalResult<'_>,
) -> String {
    format!(
        "Terminal command: {}\ncwd: {cwd}\n{}",
        command_line(command, args),
        result,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Keep command metadata and captured terminal output explicit at the reducer boundary."
)]
async fn reduce_terminal_output_with_rtk(
    reducer_mode: ReducerMode,
    executed_via_rtk: bool,
    command: &str,
    args: &[String],
    cwd: &str,
    exit_code: Option<i64>,
    signal: Option<&str>,
    timed_out: bool,
    output: &str,
) -> Option<RtkReduction> {
    if reducer_mode == ReducerMode::None || executed_via_rtk {
        return None;
    }
    if output.trim().is_empty() {
        return None;
    }
    let raw = format!(
        "terminal_command: {}\ncwd: {cwd}\nexit_code: {}\nsignal: {}\ntimed_out: {timed_out}\n\nOUTPUT:\n{output}\n",
        command_line(command, args),
        exit_code.map(|code| code.to_string()).unwrap_or_else(|| "null".to_string()),
        signal.unwrap_or("null"),
    );
    reduce_with_rtk_summary("BEARS_TERMINAL_RUN_RTK", raw).await
}

#[allow(
    clippy::too_many_arguments,
    reason = "Keep execution policy and tool-card presentation explicit at the ACP terminal boundary."
)]
pub(crate) async fn handle_terminal_run_command(
    adapter_state: &mut AdapterState,
    context: &SessionContext,
    session_id: &str,
    tool_call_id: Option<&str>,
    tool_title: Option<String>,
    args: &Value,
    policy: &ToolPolicy,
    validation: TerminalCommandValidation,
) -> Result<Value> {
    if !client_supports_terminal(adapter_state) {
        return Err(anyhow!(
            "terminal_run_command requires ACP client terminal support, but this client did not advertise terminal=true"
        ));
    }

    let command = args
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("terminal_run_command args missing command"))?;
    validate_terminal_command(command, validation)?;

    let cwd_raw = args
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("terminal_run_command args missing cwd"))?;
    let cwd = crate::normalize_requested_tool_path(cwd_raw)?;
    ensure_path_allowed_for_session(context, &cwd)?;
    if !cwd.is_dir() {
        return Err(anyhow!(
            "terminal_run_command cwd must be an existing directory"
        ));
    }

    let command_args = command_args(args)?;
    validate_terminal_command_args(command, &command_args, validation)?;
    let env = terminal_env(args)?;
    let timeout_ms = terminal_timeout_ms(args, policy);
    let output_byte_limit = terminal_output_byte_limit(args, policy);
    let reducer_mode = ReducerMode::from_args(args);
    let rtk_wrap_allowed = rtk_wrap_allowed(command, &command_args);
    let rtk_execute =
        reducer_mode == ReducerMode::ExecuteViaRtk && rtk_wrap_allowed && rtk_available().await;
    let effective_command = if rtk_execute { "rtk" } else { command };
    let effective_args = if rtk_execute {
        let mut args = Vec::with_capacity(command_args.len() + 1);
        args.push(command.to_string());
        args.extend(command_args.clone());
        args
    } else {
        command_args.clone()
    };

    let started = std::time::Instant::now();
    let create = CreateTerminalRequest::new(session_id.to_string(), effective_command.to_string())
        .args(effective_args.clone())
        .env(env)
        .cwd(Some(cwd.clone()))
        .output_byte_limit(Some(output_byte_limit));
    let create_response = adapter_state
        .transport
        .request(
            "terminal/create",
            serde_json::to_value(create)?,
            Duration::from_secs(30),
        )
        .await?;
    if let Some(error) = create_response.get("error") {
        return Err(anyhow!("terminal/create failed: {error}"));
    }
    let create_result = create_response
        .get("result")
        .cloned()
        .unwrap_or(Value::Null);
    let create_result = serde_json::from_value::<CreateTerminalResponse>(create_result)
        .with_context(|| "parse terminal/create response")?;
    let terminal_id = create_result.terminal_id.to_string();
    if let Some(tool_call_id) = tool_call_id {
        let title = tool_title.unwrap_or_else(|| {
            format!(
                "Run terminal command: {}",
                command_line(effective_command, &effective_args)
            )
        });
        let _ = send_terminal_tool_call_update(
            session_id,
            tool_call_id,
            "terminal_run_command",
            title,
            format!(
                "Running `{}` in `{}`. Live terminal output is attached below.",
                command_line(effective_command, &effective_args),
                cwd.display()
            ),
            terminal_id.clone(),
        )
        .await;
    }

    let wait = WaitForTerminalExitRequest::new(session_id.to_string(), terminal_id.clone());
    let wait_response = adapter_state
        .transport
        .request(
            "terminal/wait_for_exit",
            serde_json::to_value(wait)?,
            Duration::from_millis(timeout_ms.saturating_add(5_000)),
        )
        .await;

    let (exit_code, signal, timed_out) = match wait_response {
        Ok(response) => {
            if let Some(error) = response.get("error") {
                let _ = release_terminal(adapter_state, session_id, &terminal_id).await;
                return Err(anyhow!("terminal/wait_for_exit failed: {error}"));
            }
            let result = response.get("result").cloned().unwrap_or(Value::Null);
            let result = serde_json::from_value::<WaitForTerminalExitResponse>(result)
                .with_context(|| "parse terminal/wait_for_exit response")?;
            (
                result.exit_status.exit_code.map(|code| code as i64),
                result.exit_status.signal,
                false,
            )
        }
        Err(_err) => {
            let _ = release_terminal(adapter_state, session_id, &terminal_id).await;
            let output = terminal_output(adapter_state, session_id, &terminal_id)
                .await
                .unwrap_or_else(|_| json!({ "output": "", "truncated": false }));
            let output_text = output.get("output").and_then(Value::as_str).unwrap_or("");
            let truncated = output
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let reduced = reduce_terminal_output_with_rtk(
                reducer_mode,
                rtk_execute,
                command,
                &command_args,
                &cwd.to_string_lossy(),
                None,
                None,
                true,
                output_text,
            )
            .await;
            let display_output = reduced
                .as_ref()
                .map(|r| r.output.as_str())
                .unwrap_or(output_text);
            let result = TerminalResult {
                exit_code: None,
                signal: None,
                timed_out: true,
                elapsed_ms: started.elapsed().as_millis(),
                output: display_output,
                truncated,
                timeout_ms: Some(timeout_ms),
            };
            let content =
                terminal_result_content(command, &command_args, &cwd.to_string_lossy(), &result);
            return Ok(json!({
                "ok": false,
                "command": command,
                "args": command_args,
                "cwd": cwd.to_string_lossy(),
                "terminal_id": terminal_id,
                "exit_code": null,
                "signal": null,
                "timed_out": true,
                "elapsed_ms": started.elapsed().as_millis(),
                "source": "acp_terminal",
                "output": output.get("output").cloned().unwrap_or_else(|| json!("")),
                "truncated": truncated,
                "content": content,
                "reduction": reduced.map(|r| r.to_json()),
                "effective_command": effective_command,
                "effective_args": effective_args,
                "execution_wrapper": if rtk_execute { json!("rtk") } else { Value::Null },
                "reducer_mode": reducer_mode.as_str(),
                "rtk_wrap_allowed": rtk_wrap_allowed,
                "policy": { "timeout_ms": timeout_ms, "max_output_bytes": output_byte_limit }
            }));
        }
    };

    let output = terminal_output(adapter_state, session_id, &terminal_id).await?;
    release_terminal(adapter_state, session_id, &terminal_id).await?;
    let ok = !timed_out && exit_code == Some(0) && signal.is_none();
    let truncated = output
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output_text = output.get("output").and_then(Value::as_str).unwrap_or("");
    let reduced = reduce_terminal_output_with_rtk(
        reducer_mode,
        rtk_execute,
        command,
        &command_args,
        &cwd.to_string_lossy(),
        exit_code,
        signal.as_deref(),
        false,
        output_text,
    )
    .await;
    let display_output = reduced
        .as_ref()
        .map(|r| r.output.as_str())
        .unwrap_or(output_text);
    let result = TerminalResult {
        exit_code,
        signal: signal.as_deref(),
        timed_out: false,
        elapsed_ms: started.elapsed().as_millis(),
        output: display_output,
        truncated,
        timeout_ms: None,
    };
    let content = terminal_result_content(command, &command_args, &cwd.to_string_lossy(), &result);
    Ok(json!({
        "ok": ok,
        "command": command,
        "args": command_args,
        "cwd": cwd.to_string_lossy(),
        "terminal_id": terminal_id,
        "exit_code": exit_code,
        "signal": signal,
        "timed_out": false,
        "elapsed_ms": started.elapsed().as_millis(),
        "source": "acp_terminal",
        "output": output.get("output").cloned().unwrap_or_else(|| json!("")),
        "truncated": truncated,
        "content": content,
        "reduction": reduced.map(|r| r.to_json()),
        "effective_command": effective_command,
        "effective_args": effective_args,
        "execution_wrapper": if rtk_execute { json!("rtk") } else { Value::Null },
        "reducer_mode": reducer_mode.as_str(),
        "rtk_wrap_allowed": rtk_wrap_allowed,
        "policy": { "timeout_ms": timeout_ms, "max_output_bytes": output_byte_limit }
    }))
}

async fn terminal_output(
    adapter_state: &mut AdapterState,
    session_id: &str,
    terminal_id: &str,
) -> Result<Value> {
    let request = TerminalOutputRequest::new(session_id.to_string(), terminal_id.to_string());
    let response = adapter_state
        .transport
        .request(
            "terminal/output",
            serde_json::to_value(request)?,
            Duration::from_secs(30),
        )
        .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("terminal/output failed: {error}"));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    if serde_json::from_value::<TerminalOutputResponse>(result.clone()).is_ok() {
        Ok(result)
    } else {
        Err(anyhow!("parse terminal/output response"))
    }
}

async fn release_terminal(
    adapter_state: &mut AdapterState,
    session_id: &str,
    terminal_id: &str,
) -> Result<()> {
    let request = ReleaseTerminalRequest::new(session_id.to_string(), terminal_id.to_string());
    let response = adapter_state
        .transport
        .request(
            "terminal/release",
            serde_json::to_value(request)?,
            Duration::from_secs(30),
        )
        .await?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("terminal/release failed: {error}"));
    }
    Ok(())
}

fn command_args(args: &Value) -> Result<Vec<String>> {
    args.get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| anyhow!("terminal_run_command args entries must be strings"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn terminal_env(args: &Value) -> Result<Vec<EnvVariable>> {
    let Some(obj) = args.get("env").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut env = Vec::new();
    for (key, value) in obj {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(anyhow!(
                "terminal_run_command env keys must be non-empty ASCII alphanumeric/underscore"
            ));
        }
        if key.to_ascii_uppercase().contains("SECRET")
            || key.to_ascii_uppercase().contains("TOKEN")
            || key.to_ascii_uppercase().contains("PASSWORD")
            || key.to_ascii_uppercase().contains("KEY")
        {
            return Err(anyhow!(
                "terminal_run_command env key {key:?} looks secret-like"
            ));
        }
        env.push(EnvVariable::new(
            key.clone(),
            value
                .as_str()
                .ok_or_else(|| anyhow!("terminal_run_command env values must be strings"))?
                .to_string(),
        ));
    }
    Ok(env)
}

fn terminal_timeout_ms(args: &Value, policy: &ToolPolicy) -> u64 {
    let policy_timeout_ms = policy.total_timeout_ms.unwrap_or(300_000).clamp(1, 600_000);
    args.get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(120_000)
        .clamp(1, policy_timeout_ms)
}

fn terminal_output_byte_limit(args: &Value, policy: &ToolPolicy) -> u64 {
    let policy_max_output = policy.max_bytes.unwrap_or(131_072).clamp(1, 1_048_576);
    args.get("max_output_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(policy_max_output)
        .clamp(1, policy_max_output)
}

fn validate_terminal_command(command: &str, validation: TerminalCommandValidation) -> Result<()> {
    if command.contains('\0')
        || command.contains('/')
        || command.contains(' ')
        || command.contains('\t')
        || command.contains('\n')
    {
        return Err(anyhow!(
            "terminal_run_command command must be an executable name, not a shell string or path"
        ));
    }
    if validation == TerminalCommandValidation::Allowlisted {
        let allowed_executables = [
            "cargo", "npm", "pnpm", "yarn", "pytest", "python", "python3",
        ];
        if !allowed_executables.contains(&command) {
            return Err(anyhow!(
                "terminal_run_command command {command:?} is not in the build/test allowlist"
            ));
        }
    }
    Ok(())
}

fn validate_terminal_command_args(
    command: &str,
    args: &[String],
    validation: TerminalCommandValidation,
) -> Result<()> {
    for arg in args {
        if arg.contains('\0') {
            return Err(anyhow!(
                "terminal_run_command args must not contain NUL bytes"
            ));
        }
    }
    if validation == TerminalCommandValidation::Allowlisted
        && !terminal_command_allowed(command, args)
    {
        Err(anyhow!(
            "terminal_run_command command `{}` is not allowed by command policy",
            if args.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", command, args.join(" "))
            }
        ))
    } else {
        Ok(())
    }
}
