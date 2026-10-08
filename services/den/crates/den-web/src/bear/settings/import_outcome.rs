//! Safe, typed creation outcomes. Database/provider causes are never placed in
//! the response or a query string; retained storage is not treated as rollback.
use crate::errors::CustomError;
use den_core::{ids::BearId, DenError};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CreationStage {
    Handle,
    Creation,
    Birthday,
    Memory,
    Hats,
    Models,
    Receipt,
    KnowledgeMapping,
    Entities,
    Initialization,
    Membership,
    Procedures,
}
impl CreationStage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Handle => "Handle allocation",
            Self::Creation => "Bear creation",
            Self::Birthday => "Birthday",
            Self::Memory => "Memory installation",
            Self::Hats => "Hat installation",
            Self::Models => "Model configuration",
            Self::Receipt => "Import receipt",
            Self::KnowledgeMapping => "Knowledge mapping",
            Self::Entities => "Entity resolution reset",
            Self::Initialization => "Initialization",
            Self::Membership => "Importer membership",
            Self::Procedures => "Procedure staging",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum CreationFailure {
    Unconfirmed {
        stage: CreationStage,
        safe_error: String,
    },
    RolledBack {
        bear_id: BearId,
        stage: CreationStage,
        storage_cleanup_pending: bool,
        safe_error: String,
    },
    Retained {
        bear_id: BearId,
        slug: String,
        stage: CreationStage,
        safe_error: String,
    },
}
impl CreationFailure {
    pub(super) fn unconfirmed(stage: CreationStage) -> Self {
        Self::Unconfirmed { stage, safe_error: format!("{} did not complete. Check Bears and ask a Den operator to verify the outcome before retrying.", stage.label()) }
    }
    pub(super) fn stage(&self) -> CreationStage {
        match self {
            Self::Unconfirmed { stage, .. }
            | Self::RolledBack { stage, .. }
            | Self::Retained { stage, .. } => *stage,
        }
    }
}

/// Cleanup is deliberately lazy: if deletion fails, it is never polled or
/// invoked, so a surviving Bear keeps its SQLite files and open pool intact.
pub(super) async fn compensate<D, C, F>(
    bear_id: Uuid,
    slug: String,
    stage: CreationStage,
    delete: D,
    cleanup: C,
) -> CreationFailure
where
    D: std::future::Future<Output = Result<(), DenError>>,
    C: FnOnce() -> F,
    F: std::future::Future<Output = Result<(), CustomError>>,
{
    match delete.await {
        Ok(()) | Err(DenError::NotFound(_)) => {
            let storage_cleanup_pending = cleanup().await.is_err();
            CreationFailure::RolledBack {
                bear_id: BearId::new(bear_id), stage, storage_cleanup_pending,
                safe_error: format!("{} failed. The incomplete Bear record was removed. {}", stage.label(),
                    if storage_cleanup_pending { "Private memory cleanup still needs a Den operator; use the Bear reference below." } else { "Upload again only after repairing the failing stage." }),
            }
        }
        Err(_) => CreationFailure::Retained {
            bear_id: BearId::new(bear_id), slug, stage,
            safe_error: format!("{} failed and the incomplete Bear could not be removed. Its private memory files and pool were preserved. Ask a Den operator to repair or remove this Bear before retrying; do not grant members or enable Work.", stage.label()),
        },
    }
}

#[cfg(test)]
#[path = "tests/import_outcome.rs"]
mod tests;
