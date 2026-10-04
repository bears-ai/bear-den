//! Governance — the run-scoped supervision dial from ADR-0039.
//!
//! Governance answers *how is this execution being supervised right now?*
//! It is orthogonal to the trust profile ([`crate::RuntimeContextLabel`], the durable
//! trust/memory contract) and mutable over the life of one run. This seeds the
//! code half of ADR-0039 in `den-core`; the concrete `WorkspaceSession` +
//! governance-timeline schema is an explicit follow-up in that ADR.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Run-scoped supervision contract (ADR-0039 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Governance {
    /// Human present (client/web); live collaboration, approvals via client/channel.
    Interactive,
    /// Human recently disconnected; finish in-flight turn, await return.
    Grace,
    /// Human absent and grace expired; continue under executor-leaning policy.
    AutonomousContinuation,
    /// Human present, read-only; inspect/interrogate without owning turns.
    Observational,
    /// Panic/handoff; turn cancelled, worktree checkpointed, awaiting disposition.
    Frozen,
}

impl Governance {
    pub const ALL: [Self; 5] = [
        Self::Interactive,
        Self::Grace,
        Self::AutonomousContinuation,
        Self::Observational,
        Self::Frozen,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Grace => "grace",
            Self::AutonomousContinuation => "autonomous_continuation",
            Self::Observational => "observational",
            Self::Frozen => "frozen",
        }
    }

    /// Whether a supervising human is present this turn (ADR-0039 §2 table).
    ///
    /// `grace` is a transitional "recently disconnected" state and is treated as
    /// not-present for tool/approval gating.
    #[must_use]
    pub const fn human_present(self) -> bool {
        matches!(self, Self::Interactive | Self::Observational)
    }

    /// Projection onto the coarse ADR-0037 `run_mode` axis.
    ///
    /// Only `autonomous_continuation` runs unattended; every other mode keeps the
    /// run on the interactive track (live, awaiting return, inspected, or frozen).
    #[must_use]
    pub const fn run_mode(self) -> RunMode {
        match self {
            Self::AutonomousContinuation => RunMode::Autonomous,
            Self::Interactive | Self::Grace | Self::Observational | Self::Frozen => {
                RunMode::Interactive
            }
        }
    }
}

impl fmt::Display for Governance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Governance {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "interactive" => Ok(Self::Interactive),
            "grace" => Ok(Self::Grace),
            "autonomous_continuation" => Ok(Self::AutonomousContinuation),
            "observational" => Ok(Self::Observational),
            "frozen" => Ok(Self::Frozen),
            other => Err(format!("unknown governance: {other}")),
        }
    }
}

/// Coarse interactive/autonomous run axis (ADR-0037), derived from
/// [`Governance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Interactive,
    Autonomous,
}

impl RunMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Autonomous => "autonomous",
        }
    }
}

impl fmt::Display for RunMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
