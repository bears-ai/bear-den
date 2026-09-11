//! Headless mode: run one Den work order inside a sandbox, no editor attached.
//!
//! The armature keeps its normal shape — BearWire client + local tool
//! executor — but stdin carries no ACP client. ACP notifications still go to
//! stdout (they land in the container log, which is useful), and the single
//! human-coupled seam, `send_permission_request`, is answered by
//! [`decide_permission_headless`] instead of an editor round-trip.
//!
//! Env contract (injected by the sandbox provider):
//! - `DEN_API_URL`, `BEAR_SLUG`, `DEN_TOKEN` — standard armature connection.
//! - `DEN_WORK_ORDER_ID` — the Den work run to check out and execute.
//! - `DEN_WORKSPACE` — workspace root inside the sandbox (default `/workspace`).
//! - `DEN_HEADLESS_DEADLINE_SECS` — self-kill margin under the container
//!   timeout (default 840).
//!
//! The process exit code is the provider-visible failure signal: 0 when the
//! turn reached a terminal state, nonzero on checkout/RPC/deadline failure.

use crate::approvals::{ApprovalScope, PermissionDecision};
use crate::{
    adapter_capabilities_context, adapter_version, bearwire, direct_tools_context,
    AdapterSharedState, AdapterState, Config, RuntimeConfig, SessionContext, MODE_WRITE,
};
use agent_client_protocol::schema::RequestPermissionRequest;
use anyhow::{anyhow, Context, Result};
use bearwire_protocol::compatibility::CompatibilityManifest;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex as TokioMutex};
use uuid::Uuid;

pub(crate) struct HeadlessEnv {
    pub work_order_id: String,
    pub workspace: String,
    pub deadline: Duration,
}

impl HeadlessEnv {
    pub(crate) fn from_env() -> Result<Self> {
        let work_order_id = std::env::var("DEN_WORK_ORDER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow!("headless mode requires DEN_WORK_ORDER_ID (the Den work run to execute)")
            })?;
        let workspace = std::env::var("DEN_WORKSPACE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/workspace".to_string());
        let deadline_secs = std::env::var("DEN_HEADLESS_DEADLINE_SECS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(840);
        Ok(Self {
            work_order_id,
            workspace,
            deadline: Duration::from_secs(deadline_secs.max(30)),
        })
    }
}

pub(crate) async fn run_headless(http: &reqwest::Client, runtime: &RuntimeConfig) -> Result<()> {
    let Some(config) = runtime.config.clone() else {
        return Err(anyhow!(runtime.configuration_error_message()));
    };
    let env = HeadlessEnv::from_env()?;
    crate::set_headless_mode();

    eprintln!(
        "bear-armature: headless mode work_order_id={} workspace={} deadline_secs={}",
        env.work_order_id,
        env.workspace,
        env.deadline.as_secs()
    );

    bearwire::validate_code_token(http, &config)
        .await
        .context("headless: Den BearWire preflight failed")?;

    let session_id = format!("headless-{}", Uuid::new_v4().simple());
    let (mut adapter_state, shared_state) = headless_adapter_state();
    let context = headless_session_context(&env);
    adapter_state
        .session_contexts
        .insert(session_id.clone(), context.clone());
    shared_state
        .session_contexts
        .lock()
        .await
        .insert(session_id.clone(), context.clone());

    tracing::info!(
        work_order_id = %env.work_order_id,
        session_id = %session_id,
        "headless requesting work checkout"
    );
    let checkout = checkout_work_order(http, &config, &session_id, &env).await?;
    let (execution_attempt_id, execution_attempt_fence_epoch) = checkout_attempt(&checkout)?;
    let prompt = checkout
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("work.checkout returned no prompt"))?
        .to_string();
    let deadline = checkout
        .get("deadline_secs")
        .and_then(Value::as_u64)
        .map(|secs| Duration::from_secs(secs.max(30)))
        .unwrap_or(env.deadline)
        .min(env.deadline);

    eprintln!(
        "bear-armature: headless checkout ok session_id={} prompt_chars={} deadline_secs={}",
        session_id,
        prompt.len(),
        deadline.as_secs()
    );

    tracing::info!(
        work_order_id = %env.work_order_id,
        session_id = %session_id,
        execution_attempt_id = %execution_attempt_id,
        fence_epoch = execution_attempt_fence_epoch,
        "headless checkout received canonical Work attempt"
    );

    // Headless bypasses ACP's session/prompt request handler, so register the
    // turn explicitly. Without this, streamed output and armature-local tool
    // updates are incorrectly discarded as stale.
    let turn_token = Uuid::new_v4();
    let response = crate::PromptResponseGuard::new(Value::Null);
    crate::register_prompt_turn_for_session(
        &shared_state,
        &session_id,
        turn_token,
        None,
        response.clone(),
    )
    .await;

    // BearWire has its own delivery fuse; this outer deadline remains the
    // stricter headless checkout boundary and checkpoint trigger.
    let turn = tokio::time::timeout(
        deadline,
        bearwire::handle_prompt(
            http,
            &config,
            &mut adapter_state,
            &shared_state,
            response,
            &session_id,
            &prompt,
            json!({}),
            context.raw.clone(),
            None,
            MODE_WRITE,
            turn_token,
        ),
    )
    .await;

    let (status_hint, summary, outcome) = match &turn {
        Ok(Ok(())) => (
            "completed",
            "headless turn reached a terminal run event".to_string(),
            Ok(()),
        ),
        Ok(Err(err)) => (
            "failed",
            format!("headless turn failed: {err:#}"),
            Err(anyhow!("headless turn failed: {err:#}")),
        ),
        Err(_) => {
            let summary = format!(
                "headless turn exceeded its {}s deadline; checkpoint requested before sandbox exit",
                deadline.as_secs()
            );
            request_work_checkpoint(
                http,
                &config,
                execution_attempt_id,
                execution_attempt_fence_epoch,
                "near_ko",
                &summary,
            )
            .await;
            (
                "deadline_exceeded",
                summary,
                Err(anyhow!("headless deadline exceeded")),
            )
        }
    };

    report_work_order(http, &config, &session_id, &env, status_hint, &summary).await;
    eprintln!("bear-armature: headless finished status={status_hint}");
    outcome
}

fn headless_adapter_state() -> (AdapterState, AdapterSharedState) {
    let adapter_state = AdapterState::default();
    let (cancellation_tx, _) = broadcast::channel(64);
    let shared_state = AdapterSharedState {
        transport: adapter_state.transport.clone(),
        client_capabilities: Arc::new(TokioMutex::new(Value::Null)),
        session_contexts: Arc::new(TokioMutex::new(HashMap::new())),
        last_plan_update_hashes: Arc::new(TokioMutex::new(HashMap::new())),
        surface_tool_statuses: Arc::new(TokioMutex::new(HashMap::new())),
        tool_tasks: crate::tool_tasks::ToolTaskRegistry::default(),
        mcp_registry: crate::tools::mcp::McpRegistry::default(),
        approval_cache: crate::approvals::ApprovalCache::default(),
        cancellation_tx,
        active_prompts: Arc::new(TokioMutex::new(HashMap::new())),
        projection_dispatcher: crate::projection_dispatcher::AcpProjectionDispatcher::default(),
        execution_diagnostics: crate::execution_diagnostics::ExecutionDiagnosticStore::default(),
    };
    (adapter_state, shared_state)
}

fn headless_session_context(env: &HeadlessEnv) -> SessionContext {
    let raw = json!({
        "cwd": env.workspace,
        "workspace_roots": [env.workspace],
        "adapter_version": adapter_version(),
        "adapter": adapter_capabilities_context(),
        "direct_tools": direct_tools_context(),
        "mcp_servers": [],
        "stance": "work",
        "headless": true,
    });
    SessionContext {
        cwd: env.workspace.clone(),
        roots: vec![env.workspace.clone()],
        raw,
        mcp_sources: Vec::new(),
        conversation_id: None,
        resolved_conversation_id: None,
        thread_title: None,
        current_mode: Some(MODE_WRITE.to_string()),
    }
}

async fn checkout_work_order(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    env: &HeadlessEnv,
) -> Result<Value> {
    bearwire::rpc_call(
        http,
        config,
        "work.checkout",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "work_order_id": env.work_order_id,
            "cwd": env.workspace,
            "compatibility": CompatibilityManifest::armature(),
        }),
    )
    .await
    .context("BearWire work.checkout failed")
}

/// Advisory report; the authoritative outcome is the Den-side run hook plus
/// Docket task state, so failures here are logged, not fatal.
async fn report_work_order(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    env: &HeadlessEnv,
    status_hint: &str,
    summary: &str,
) {
    let result = bearwire::rpc_call(
        http,
        config,
        "work.report",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "work_order_id": env.work_order_id,
            "status_hint": status_hint,
            "summary": summary,
        }),
    )
    .await;
    if let Err(err) = result {
        eprintln!("bear-armature: headless work.report failed (advisory): {err:#}");
    }
}

async fn request_work_checkpoint(
    http: &reqwest::Client,
    config: &Config,
    execution_attempt_id: Uuid,
    fence_epoch: i64,
    signal: &str,
    summary: &str,
) {
    let result = bearwire::rpc_call(
        http,
        config,
        "work.boundary",
        json!({
            "execution_attempt_id": execution_attempt_id.to_string(),
            "fence_epoch": fence_epoch,
            "boundary_key": Uuid::new_v4().to_string(),
            "signal": signal,
        }),
    )
    .await;
    let gate = match result {
        Ok(value) => value.get("gate").cloned().unwrap_or(Value::Null),
        Err(err) => {
            eprintln!("bear-armature: headless work.boundary failed (advisory): {err:#}");
            return;
        }
    };
    if gate.get("disposition").and_then(Value::as_str) != Some("require_checkpoint") {
        return;
    }
    // The server derives the work run from this exact fenced attempt and
    // creates/link/validates the immutable evidence before acknowledging it.
    let directive_id = match gate.get("directive_id").and_then(Value::as_str) {
        Some(value) => value,
        None => {
            eprintln!("bear-armature: work.boundary require_checkpoint omitted directive_id");
            return;
        }
    };
    if let Err(err) = bearwire::rpc_call(
        http, config, "work.checkpoint_evidence",
        json!({ "directive_id": directive_id, "execution_attempt_id": execution_attempt_id.to_string(),
                "fence_epoch": fence_epoch, "summary": summary }),
    ).await {
        eprintln!("bear-armature: work.checkpoint_evidence failed: {err:#}");
    }
}

fn checkout_attempt(checkout: &Value) -> Result<(Uuid, i64)> {
    let attempt_id = checkout
        .get("execution_attempt_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("work.checkout returned no execution attempt id"))?
        .parse()
        .context("work.checkout returned invalid execution attempt id")?;
    let fence_epoch = checkout
        .get("execution_attempt_fence_epoch")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("work.checkout returned no execution attempt fence epoch"))?;
    Ok((attempt_id, fence_epoch))
}

#[cfg(test)]
mod tests {
    use super::checkout_attempt;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn checkout_attempt_requires_canonical_attempt_and_fence() {
        let attempt_id = Uuid::new_v4();
        assert_eq!(
            checkout_attempt(&json!({
                "execution_attempt_id": attempt_id.to_string(),
                "execution_attempt_fence_epoch": 4,
            }))
            .expect("canonical checkout attempt"),
            (attempt_id, 4)
        );
        assert!(checkout_attempt(&json!({})).is_err());
    }
}

/// Auto-resolve a permission request with no human present.
///
/// The container is the primary enforcement boundary and workspace-root path
/// allowlisting already applies at execution; this policy is the second belt:
/// local filesystem/git/process work proceeds, browser surfaces and anything
/// aimed at an external URL are denied.
///
/// ponytail: coarse allow/deny; upgrade path is a per-task policy delivered
/// in the work.checkout response.
pub(crate) fn decide_permission_headless(
    request: &RequestPermissionRequest,
) -> Result<PermissionDecision> {
    let value = serde_json::to_value(request).unwrap_or(Value::Null);
    let meta = value
        .get("_meta")
        .or_else(|| value.get("meta"))
        .cloned()
        .unwrap_or(Value::Null);
    let tool_name = meta
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let target_url = meta.get("targetUrl").and_then(Value::as_str);

    let denied_reason = if tool_name.starts_with("chrome_") || tool_name == "browser_bridge" {
        Some("browser tools are not available in headless sandbox runs")
    } else if target_url.is_some() {
        Some("external URL targets are denied by the headless policy")
    } else {
        None
    };

    let approved = denied_reason.is_none();
    eprintln!(
        "bear-armature: permission auto-decision decided_by=headless_policy tool_name={} approved={}{}",
        if tool_name.is_empty() { "<unknown>" } else { &tool_name },
        approved,
        denied_reason.map(|r| format!(" reason={r}")).unwrap_or_default(),
    );
    if approved {
        Ok(PermissionDecision {
            approved: true,
            remember: false,
            scope: ApprovalScope::Workspace,
        })
    } else {
        Ok(PermissionDecision {
            approved: false,
            remember: false,
            scope: ApprovalScope::Workspace,
        })
    }
}
