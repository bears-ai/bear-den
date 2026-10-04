use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{DenError, Governance, RuntimeContextLabel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearCapability {
    Converse,
    OwnSessionTasks,
    SelectSessionTask,
    ExecuteFocusedTask,
    ExecuteJob,
    CreateJob,
    DispatchWork,
    UseArmatureTools,
    UseWorkSurfaces,
    ManageWorkSurfaces,
    ProposeProfileMemory,
    CurateMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<BearCapability>);

impl CapabilitySet {
    pub fn from_capabilities(capabilities: impl IntoIterator<Item = BearCapability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    pub fn contains(&self, capability: BearCapability) -> bool {
        self.0.contains(&capability)
    }

    pub fn require(&self, capability: BearCapability) -> Result<(), DenError> {
        if self.contains(capability) {
            Ok(())
        } else {
            Err(DenError::Authorization(format!(
                "effective policy does not grant capability {capability:?}"
            )))
        }
    }

    fn remove(&mut self, capability: BearCapability) {
        self.0.remove(&capability);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmatureAvailability {
    Connected,
    Absent,
}

/// A Den-verified execution surface, not a client-supplied stance or hat label.
/// Construct this only after authenticating the channel/armature or resolving a
/// Docket Work assignment. The execution context label is a projection of this
/// verified origin, not an independent authority input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnExecutionOrigin {
    ChannelConversation,
    /// The browser's server-derived session task controls are Pair-equivalent
    /// today, but have no trusted local armature tools.
    BrowserTaskSession,
    ArmatureConversation(ArmatureAvailability),
    AuthorizedWorkRun(ArmatureAvailability),
    InternalCuration,
    InboundObservation,
}

impl TurnExecutionOrigin {
    /// Generic conversational sessions require an ordinary conversation or Job
    /// source. System operations use their own verified, narrow entry points.
    pub fn require_ordinary_session(self) -> Result<(), DenError> {
        match self {
            Self::InternalCuration | Self::InboundObservation => Err(DenError::Authorization(
                "system execution requires a dedicated source-verified operation".into(),
            )),
            Self::ChannelConversation
            | Self::BrowserTaskSession
            | Self::ArmatureConversation(_)
            | Self::AuthorizedWorkRun(_) => Ok(()),
        }
    }

    fn policy_inputs(self) -> (RuntimeContextLabel, ArmatureAvailability) {
        match self {
            Self::ChannelConversation => (
                RuntimeContextLabel::ChannelConversation,
                ArmatureAvailability::Absent,
            ),
            Self::BrowserTaskSession => (
                RuntimeContextLabel::ArmatureConversation,
                ArmatureAvailability::Absent,
            ),
            Self::ArmatureConversation(armature) => {
                (RuntimeContextLabel::ArmatureConversation, armature)
            }
            Self::AuthorizedWorkRun(armature) => (RuntimeContextLabel::JobRun, armature),
            Self::InternalCuration => (RuntimeContextLabel::Curation, ArmatureAvailability::Absent),
            Self::InboundObservation => (
                RuntimeContextLabel::Observation,
                ArmatureAvailability::Absent,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicy {
    /// Origin-derived metadata only; never an input to capability compilation.
    pub context_label: RuntimeContextLabel,
    pub governance: Governance,
    pub capabilities: CapabilitySet,
}

impl EffectivePolicy {
    pub fn compile_for_origin(origin: TurnExecutionOrigin, governance: Governance) -> Self {
        let (context_label, armature) = origin.policy_inputs();
        use BearCapability::{
            Converse, CreateJob, CurateMemory, DispatchWork, ExecuteFocusedTask, ExecuteJob,
            ManageWorkSurfaces, OwnSessionTasks, ProposeProfileMemory, SelectSessionTask,
            UseArmatureTools, UseWorkSurfaces,
        };

        let base = match origin {
            TurnExecutionOrigin::ChannelConversation => {
                [Converse, CreateJob, DispatchWork].as_slice()
            }
            TurnExecutionOrigin::BrowserTaskSession
            | TurnExecutionOrigin::ArmatureConversation(_) => [
                Converse,
                OwnSessionTasks,
                SelectSessionTask,
                ExecuteFocusedTask,
                ExecuteJob,
                CreateJob,
                DispatchWork,
                UseArmatureTools,
                UseWorkSurfaces,
                ManageWorkSurfaces,
                ProposeProfileMemory,
            ]
            .as_slice(),
            TurnExecutionOrigin::AuthorizedWorkRun(_) => [
                ExecuteFocusedTask,
                ExecuteJob,
                UseArmatureTools,
                UseWorkSurfaces,
                ProposeProfileMemory,
            ]
            .as_slice(),
            TurnExecutionOrigin::InternalCuration => [CurateMemory].as_slice(),
            TurnExecutionOrigin::InboundObservation => [].as_slice(),
        };
        let mut capabilities = CapabilitySet::from_capabilities(base.iter().copied());

        if armature == ArmatureAvailability::Absent || governance != Governance::Interactive {
            capabilities.remove(UseArmatureTools);
        }
        if governance != Governance::Interactive {
            capabilities.remove(CreateJob);
            capabilities.remove(DispatchWork);
            capabilities.remove(ManageWorkSurfaces);
        }
        if matches!(governance, Governance::Observational | Governance::Frozen) {
            for capability in [
                OwnSessionTasks,
                SelectSessionTask,
                ExecuteFocusedTask,
                ExecuteJob,
                CreateJob,
                DispatchWork,
                ManageWorkSurfaces,
                ProposeProfileMemory,
                CurateMemory,
            ] {
                capabilities.remove(capability);
            }
        }

        Self {
            context_label,
            governance,
            capabilities,
        }
    }
}

#[cfg(test)]
mod tests;
