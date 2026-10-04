//! Origin-derived execution metadata and historical schema projection.
//!
//! This label is not authority: permissions, model settings, loop defaults, and
//! compaction must use a verified execution origin and its canonical source.
//! It cannot reconstruct an origin (for example, browser task sessions and
//! armature conversations share the same historical `pair` label).

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A metadata label projected from a verified execution origin, or read from
/// historical audit/archive records. It is not a user hat, persona, or memory
/// permission, and must never grant execution authority.
///
/// The explicit serde names, [`Self::as_str`], and [`FromStr`] preserve the
/// archived schema's `chat/pair/work/curate/watch` values. Variant names are
/// source-readable; persisted values and archive namespaces require no migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeContextLabel {
    #[serde(rename = "chat")]
    ChannelConversation,
    #[serde(rename = "pair")]
    ArmatureConversation,
    #[serde(rename = "curate")]
    Curation,
    #[serde(rename = "work")]
    JobRun,
    #[serde(rename = "watch")]
    Observation,
}

impl RuntimeContextLabel {
    pub const ALL: [Self; 5] = [
        Self::ChannelConversation,
        Self::ArmatureConversation,
        Self::Curation,
        Self::JobRun,
        Self::Observation,
    ];

    /// Historical schema spelling for audit/archive serialization and namespaces.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChannelConversation => "chat",
            Self::ArmatureConversation => "pair",
            Self::Curation => "curate",
            Self::JobRun => "work",
            Self::Observation => "watch",
        }
    }
}

/// Display uses the historical schema spelling, not the Rust variant name.
impl fmt::Display for RuntimeContextLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decode only the historical schema spelling; this does not verify an origin.
impl FromStr for RuntimeContextLabel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "chat" => Ok(Self::ChannelConversation),
            "pair" => Ok(Self::ArmatureConversation),
            "curate" => Ok(Self::Curation),
            "work" => Ok(Self::JobRun),
            "watch" => Ok(Self::Observation),
            other => Err(format!("unknown execution context label: {other}")),
        }
    }
}

#[cfg(test)]
mod tests;
