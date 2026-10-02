use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{DenError, Governance, TrustProfile};

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
/// Docket Work assignment. The compatibility profile is a projection of this
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

    fn policy_inputs(self) -> (TrustProfile, ArmatureAvailability) {
        match self {
            Self::ChannelConversation => (TrustProfile::Chat, ArmatureAvailability::Absent),
            Self::BrowserTaskSession => (TrustProfile::Pair, ArmatureAvailability::Absent),
            Self::ArmatureConversation(armature) => (TrustProfile::Pair, armature),
            Self::AuthorizedWorkRun(armature) => (TrustProfile::Work, armature),
            Self::InternalCuration => (TrustProfile::Curate, ArmatureAvailability::Absent),
            Self::InboundObservation => (TrustProfile::Watch, ArmatureAvailability::Absent),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicy {
    pub trust_profile: TrustProfile,
    pub governance: Governance,
    pub capabilities: CapabilitySet,
}

impl EffectivePolicy {
    pub fn compile_for_origin(origin: TurnExecutionOrigin, governance: Governance) -> Self {
        let (trust_profile, armature) = origin.policy_inputs();
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
            trust_profile,
            governance,
            capabilities,
        }
    }

    /// Compatibility projection for routes that still persist a profile label.
    /// New authority decisions should call `compile_for_origin` with a verified
    /// channel, armature, or Job assignment instead.
    pub fn compile(
        trust_profile: TrustProfile,
        governance: Governance,
        armature: ArmatureAvailability,
    ) -> Self {
        let origin = match trust_profile {
            TrustProfile::Chat => TurnExecutionOrigin::ChannelConversation,
            TrustProfile::Pair => TurnExecutionOrigin::ArmatureConversation(armature),
            TrustProfile::Work => TurnExecutionOrigin::AuthorizedWorkRun(armature),
            TrustProfile::Curate => TurnExecutionOrigin::InternalCuration,
            TrustProfile::Watch => TurnExecutionOrigin::InboundObservation,
        };
        Self::compile_for_origin(origin, governance)
    }
}

#[cfg(test)]
mod tests;
