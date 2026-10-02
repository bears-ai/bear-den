//! Trusted creation intent supplied by authenticated application routes.

use den_core::{BearCapability, DenError, EffectivePolicy, Governance, TurnExecutionOrigin};

/// Control-plane authority, separate from the persisted creator audit label.
/// This is deliberately not deserializable from model or HTTP arguments.
#[derive(Debug, Clone, Copy)]
pub enum DocketJobCreationAuthority {
    /// An authenticated human explicitly requested creation through the UI.
    HumanRequest,
    /// A native tool call retains its server-verified turn policy.
    NativeTurn {
        origin: TurnExecutionOrigin,
        governance: Governance,
    },
}

impl DocketJobCreationAuthority {
    pub(crate) fn require_create_job(self) -> Result<(), DenError> {
        match self {
            Self::HumanRequest => Ok(()),
            Self::NativeTurn { origin, governance } => {
                EffectivePolicy::compile_for_origin(origin, governance)
                    .capabilities
                    .require(BearCapability::CreateJob)
            }
        }
    }
}

#[cfg(test)]
#[path = "creation_authority_tests.rs"]
mod tests;
