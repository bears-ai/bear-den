use std::str::FromStr;

use den_core::{DenError, TurnExecutionOrigin};

use super::RuntimeCompactionPolicy;

/// Runtime compaction rollout mode (`COMPACTION_MODE` env).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionMode {
    /// Compaction evaluation disabled.
    Off,
    /// Evaluate policy and record events; do not mutate prompt assembly.
    Observe,
    /// Evaluate, record events, persist artifacts, and bound prompt transcript.
    Active,
}

impl FromStr for CompactionMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "false" | "0" => Ok(Self::Off),
            "observe" | "" => Ok(Self::Observe),
            "active" | "on" | "true" | "1" => Ok(Self::Active),
            other => Err(format!("unsupported compaction mode: {other}")),
        }
    }
}

impl CompactionMode {
    pub fn parse(raw: &str) -> Self {
        raw.parse().unwrap_or_else(|err| {
            tracing::warn!(%err, "invalid COMPACTION_MODE; defaulting to observe");
            Self::Observe
        })
    }
}

/// When compaction writes run relative to turn assembly (`COMPACTION_TIMING` env).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTiming {
    /// Evaluate and persist during turn assembly (legacy).
    Sync,
    /// Read artifacts at assembly; enqueue WRITE after turn completes (default).
    Async,
}

impl FromStr for CompactionTiming {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "sync" | "inline" | "turn_start" => Ok(Self::Sync),
            "async" | "post_turn" | "" => Ok(Self::Async),
            other => Err(format!("unsupported compaction timing: {other}")),
        }
    }
}

impl CompactionTiming {
    pub fn parse(raw: &str) -> Self {
        raw.parse().unwrap_or_else(|err| {
            tracing::warn!(%err, "invalid COMPACTION_TIMING; defaulting to async");
            Self::Async
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct CompactionPolicyDefaults {
    policy_version: &'static str,
    protected_recent_group_count: usize,
    max_groups_before_compaction: usize,
    max_transcript_chars: usize,
}

impl CompactionPolicyDefaults {
    fn into_policy(self) -> RuntimeCompactionPolicy {
        RuntimeCompactionPolicy {
            policy_version: self.policy_version.into(),
            protected_recent_group_count: self.protected_recent_group_count,
            max_groups_before_compaction: self.max_groups_before_compaction,
            max_transcript_chars: self.max_transcript_chars,
        }
    }
}

const PAIR_POLICY_DEFAULTS: CompactionPolicyDefaults = CompactionPolicyDefaults {
    policy_version: "pair-v1",
    protected_recent_group_count: 4,
    max_groups_before_compaction: 8,
    max_transcript_chars: 24_000,
};
const CHAT_POLICY_DEFAULTS: CompactionPolicyDefaults = CompactionPolicyDefaults {
    policy_version: "chat-v1",
    protected_recent_group_count: 3,
    max_groups_before_compaction: 10,
    max_transcript_chars: 16_000,
};
const WORK_POLICY_DEFAULTS: CompactionPolicyDefaults = CompactionPolicyDefaults {
    policy_version: "work-v1",
    protected_recent_group_count: 3,
    max_groups_before_compaction: 8,
    max_transcript_chars: 20_000,
};
const BACKGROUND_POLICY_DEFAULTS: CompactionPolicyDefaults = CompactionPolicyDefaults {
    policy_version: "background-v1",
    protected_recent_group_count: 2,
    max_groups_before_compaction: 6,
    max_transcript_chars: 12_000,
};

/// The worker owns maintenance compaction; queued audit metadata cannot grant a turn origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionSource {
    Turn(TurnExecutionOrigin),
    ContextMaintenance,
}

pub fn compaction_policy_for_source(
    source: CompactionSource,
) -> Result<RuntimeCompactionPolicy, DenError> {
    let defaults = match source {
        CompactionSource::Turn(origin) => {
            origin.require_ordinary_session()?;
            match origin {
                TurnExecutionOrigin::ChannelConversation => CHAT_POLICY_DEFAULTS,
                TurnExecutionOrigin::BrowserTaskSession
                | TurnExecutionOrigin::ArmatureConversation(_) => PAIR_POLICY_DEFAULTS,
                TurnExecutionOrigin::AuthorizedWorkRun(_) => WORK_POLICY_DEFAULTS,
                TurnExecutionOrigin::InternalCuration | TurnExecutionOrigin::InboundObservation => {
                    unreachable!("ordinary origin checked above")
                }
            }
        }
        CompactionSource::ContextMaintenance => BACKGROUND_POLICY_DEFAULTS,
    };
    Ok(defaults.into_policy())
}

#[cfg(test)]
mod tests;
