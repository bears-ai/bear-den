//! `DenToolInvocationContext`: the per-call value passed to every tool executor.
//!
//! This is **data, not a capability** (per the Phase B design rules): plain,
//! serializable per-invocation identity/session/channel info. It lives in
//! `den-tools` so executors can move here; the `den` crate re-exports it at
//! `core::tools::session::DenToolInvocationContext` for existing call sites and
//! is responsible for constructing it. See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`.

use crate::RuntimeContextLabel;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::tools::arguments::DenToolChannelContext;
use crate::tools::capability_catalog::SessionCapabilityDescriptor;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DenToolInvocationContext {
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub binding_id: String,
    /// Historical schema field holding a derived audit label, not a capability.
    pub profile: Option<RuntimeContextLabel>,
    pub user_id: i32,
    pub username: Option<String>,
    pub membership_role: Option<String>,
    pub conversation_id: String,
    pub session_id: String,
    /// The active work run, when this invocation originates in a dispatched
    /// work sandbox. This is an authority binding, not a workspace hint.
    #[serde(default)]
    pub work_run_id: Option<Uuid>,
    #[serde(default)]
    pub client_session_id: Option<String>,
    #[serde(default)]
    pub conversation_selection: Option<String>,
    #[serde(default)]
    pub runtime_target: Option<String>,
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Provider tools advertised for this runtime session after adapter and
    /// turn-policy gating. They are not durable or globally invocable.
    #[serde(default)]
    pub session_capabilities: Vec<SessionCapabilityDescriptor>,
    #[serde(default)]
    pub session_policy: Option<Value>,
    #[serde(default)]
    pub activity: Option<Value>,
    #[serde(default)]
    pub runtime: Option<Value>,
    #[serde(default)]
    pub context_budget: Option<Value>,
    #[serde(default)]
    pub projected_memory: Option<Value>,
    #[serde(default)]
    pub recalled_memory: Option<Value>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub channel: DenToolChannelContext,
}
