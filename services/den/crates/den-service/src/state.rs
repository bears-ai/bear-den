//! Shared application state for the Den HTTP edges.
//!
//! `DenState` lives in `den-service` — below every HTTP edge and below
//! `den-runtime` — so the native runtime implementation is not the owner of
//! shared application wiring. The state holds service handles (database pool,
//! configuration, model metadata client, per-Bear memory stores, and process-local
//! turn coordinators), not runtime execution logic.

use std::sync::Arc;

use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::broadcast;

use den_core::config::Config;

use crate::{
    bifrost::{new_catalog_store, BifrostCatalogStore, BifrostClient},
    tool_turns::ToolTurnCoordinator,
    turn_controller::ActiveTurnCancelRegistry,
};
use den_memory::MemoryStoreManager;

/// Shared state for the Den HTTP surfaces.
///
/// Contains resources needed by every edge: the database connection pool,
/// immutable runtime configuration, the Bifrost model-metadata client, per-Bear
/// SQLite memory stores, and the process-local turn coordinators.
#[derive(Clone)]
pub struct DenState {
    /// Unique identity for this Den process lifetime.
    pub process_epoch_id: uuid::Uuid,
    /// Database connection pool.
    pub sqlx_pool: PgPool,
    /// Shared immutable runtime configuration.
    pub config: Arc<Config>,
    /// Shared Bifrost model metadata client.
    pub bifrost: Arc<BifrostClient>,
    /// Shared runtime model catalog snapshot.
    pub bifrost_catalog: BifrostCatalogStore,
    /// Process-local active direct tool turns.
    pub tool_turns: ToolTurnCoordinator,
    /// Process-local active stream cancellation signals.
    pub turn_cancellations: ActiveTurnCancelRegistry,
    /// Per-Bear SQLite memory stores (native runtime cognition).
    pub memory_stores: MemoryStoreManager,
    /// Best-effort, process-local observations for active BearWire livestreams.
    ///
    /// These observations are deliberately outside the durable BearWire event
    /// sequence: lagged subscribers reconnect from authoritative state instead
    /// of replaying replaceable progress placeholders.
    pub bearwire_livestream: broadcast::Sender<BearWireLivestreamEvent>,
}

/// A non-replayable BearWire observation scoped to one client session.
#[derive(Clone, Debug)]
pub struct BearWireLivestreamEvent {
    pub session_id: String,
    pub event: Value,
}

const BEARWIRE_LIVESTREAM_CAPACITY: usize = 64;

impl DenState {
    /// Build the shared state, initializing the process-local turn coordinators.
    /// Production calls this once at the process composition root; edges and workers
    /// receive clones so they share one epoch and controller registry.
    #[must_use]
    pub fn new(
        sqlx_pool: PgPool,
        config: Arc<Config>,
        bifrost: Arc<BifrostClient>,
        memory_stores: MemoryStoreManager,
    ) -> Self {
        let (bearwire_livestream, _) = broadcast::channel(BEARWIRE_LIVESTREAM_CAPACITY);
        Self {
            process_epoch_id: uuid::Uuid::new_v4(),
            sqlx_pool,
            config,
            bifrost,
            bifrost_catalog: new_catalog_store(),
            tool_turns: ToolTurnCoordinator::new(),
            turn_cancellations: ActiveTurnCancelRegistry::new(),
            memory_stores,
            bearwire_livestream,
        }
    }

    /// Publish a replaceable live observation. No subscriber is required and a
    /// lagged subscriber is expected to refresh its derived snapshot.
    pub fn publish_bearwire_livestream(&self, session_id: impl Into<String>, event: Value) {
        let _ = self.bearwire_livestream.send(BearWireLivestreamEvent {
            session_id: session_id.into(),
            event,
        });
    }
}
