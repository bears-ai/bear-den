//! The `RoleMemoryStore` capability seam: canonical per-Bear, per-role memory.
//!
//! Hides the SQLite `BearMemoryStore` behind
//! a trait returning already-shaped tool JSON. The `den` crate implements it over
//! `MemoryStoreManager` + `memory::tools::sqlite_*`. Methods are grown as executor
//! groups migrate (read surface first; write surface follows).
//! See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`.

use crate::{BearProfile, DenError};
use serde_json::Value;

use super::RoleMemoryEntryWrite;
use crate::tools::{context::DenToolInvocationContext, prompt_memory::PromptMemoryVisibility};

// Native async fn in trait: workspace-internal, consumed via generic bounds /
// concrete impls only (never `dyn`), so Send flows through monomorphization.
#[allow(async_fn_in_trait)]
pub trait RoleMemoryStore: Send + Sync {
    /// Read records at a logical path (tool-shaped JSON).
    async fn read(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
        path: &str,
    ) -> Result<Value, DenError>;

    /// Browse the role memory tree (tool-shaped JSON).
    async fn browse(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
    ) -> Result<Value, DenError>;

    /// Search role memory (tool-shaped JSON). `limit` is already clamped.
    async fn search(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
        query: &str,
        limit: i64,
    ) -> Result<Value, DenError>;

    /// Base memory-status JSON **without** the prompt-memory diagnostic; the
    /// `memory_status` executor composes that diagnostic on top.
    async fn status_base(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
    ) -> Result<(Value, PromptMemoryVisibility), DenError>;

    /// Persist a role-memory entry; returns tool-shaped JSON.
    async fn write_entry(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
        entry: RoleMemoryEntryWrite,
    ) -> Result<Value, DenError>;
}
