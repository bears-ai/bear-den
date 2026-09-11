//! Tool-dispatch abstraction for the native runtime.
//!
//! The native runtime executes builtin Den tools mid-turn, but concrete tool
//! capabilities live in the `den` binary. The binary installs one process-wide
//! invoker composed with the canonical application state.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::Value;

use den_core::tools::context::DenToolInvocationContext;
use den_core::{DenError, EffectivePolicy};

use crate::turn_ids::{ToolCallId, TurnRunId};

/// One resolved builtin-tool invocation from a native agent loop.
#[derive(Debug, Clone)]
pub struct RuntimeToolInvocation {
    pub tool_name: String,
    pub arguments: Value,
    pub context: DenToolInvocationContext,
    pub effective_policy: EffectivePolicy,
    pub origin_run_id: Option<TurnRunId>,
    pub tool_call_id: ToolCallId,
}

/// Dispatches builtin Den tools through capabilities composed by the `den` binary.
#[async_trait]
pub trait RuntimeToolInvoker: Send + Sync {
    async fn invoke(&self, invocation: RuntimeToolInvocation) -> Result<Value, DenError>;
}

static TOOL_INVOKER: OnceLock<Arc<dyn RuntimeToolInvoker>> = OnceLock::new();

/// Install the process-wide builtin-Den-tool invoker.
///
/// The invoker is composed only after the canonical process state exists. The
/// first install wins so request paths cannot replace lifecycle authority.
pub fn set_tool_invoker(invoker: Arc<dyn RuntimeToolInvoker>) {
    if TOOL_INVOKER.set(invoker).is_err() {
        tracing::warn!("builtin Den tool invoker already initialized; keeping first instance");
    }
}

pub fn tool_invoker() -> Option<Arc<dyn RuntimeToolInvoker>> {
    TOOL_INVOKER.get().cloned()
}
