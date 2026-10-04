//! The `WorkSurfaceOps` capability seam (ADR-0040: "work surface" is canonical).
//!
//! The inference / slug / payload builders are pure `den-tools` free functions;
//! only the scaffold memory I/O is a capability. `ScaffoldRequest` is the
//! runtime-neutral file descriptor the builder emits; the `den` impl maps it onto
//! the native SQLite write path.

use crate::{DenError, RuntimeContextLabel};
use serde_json::Value;
use uuid::Uuid;

use crate::tools::context::DenToolInvocationContext;

/// One scaffold file to write (runtime-neutral; `proposal_id`/`source_paths`
/// don't apply to scaffolds, so they are omitted here).
#[derive(Debug, Clone)]
pub struct ScaffoldRequest {
    pub target_path: String,
    pub mode: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
}

/// The result of writing a scaffold: per-file responses plus the storage tag.
#[derive(Debug, Clone)]
pub struct WorkSurfaceScaffoldOutcome {
    pub storage: Option<String>,
    pub updates: Vec<Value>,
}

// Native async fn in trait: workspace-internal, consumed via generic bounds /
// concrete impls only (never `dyn`), so Send flows through monomorphization.
#[allow(async_fn_in_trait)]
pub trait WorkSurfaceOps: Send + Sync {
    async fn write_scaffold(
        &self,
        bear_id: Uuid,
        role: RuntimeContextLabel,
        slug: &str,
        name: &str,
        requests: Vec<ScaffoldRequest>,
    ) -> Result<WorkSurfaceScaffoldOutcome, DenError>;

    /// Render the work-surface orientation payload.
    ///
    /// The native listing/shaping differs enough that this stays a
    /// coarse capability returning the finished payload.
    async fn orient(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<Value, DenError>;
}
