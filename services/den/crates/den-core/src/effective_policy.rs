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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicy {
    pub trust_profile: TrustProfile,
    pub governance: Governance,
    pub capabilities: CapabilitySet,
}

impl EffectivePolicy {
    pub fn compile(
        trust_profile: TrustProfile,
        governance: Governance,
        armature: ArmatureAvailability,
    ) -> Self {
        use BearCapability::{
            Converse, CreateJob, CurateMemory, DispatchWork, ExecuteFocusedTask, ExecuteJob,
            ManageWorkSurfaces, OwnSessionTasks, ProposeProfileMemory, SelectSessionTask,
            UseArmatureTools, UseWorkSurfaces,
        };

        let base = match trust_profile {
            TrustProfile::Chat => [Converse, CreateJob, DispatchWork].as_slice(),
            TrustProfile::Pair => [
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
            TrustProfile::Work => [
                ExecuteFocusedTask,
                ExecuteJob,
                UseArmatureTools,
                UseWorkSurfaces,
                ProposeProfileMemory,
            ]
            .as_slice(),
            TrustProfile::Curate => [CurateMemory].as_slice(),
            TrustProfile::Watch => [].as_slice(),
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
}

#[cfg(test)]
mod tests;
