//! `TrustProfile`: durable trust, memory, prompt, and capability defaults.
//!
//! This is a foundational closed-set enum shared across den crates (runtime,
//! docket, tools, memory, web/api), so it lives in `den-core`. The original
//! home was `core::bears::model`, which now re-exports it for backward
//! compatibility.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::ids::BearId;

/// Durable trust profile applied as policy defaults for a model turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustProfile {
    Chat,
    Pair,
    Curate,
    Work,
    Watch,
}

impl TrustProfile {
    pub const ALL: [Self; 5] = [
        Self::Chat,
        Self::Pair,
        Self::Curate,
        Self::Work,
        Self::Watch,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Pair => "pair",
            Self::Curate => "curate",
            Self::Work => "work",
            Self::Watch => "watch",
        }
    }

    pub fn is_harness_backed(self) -> bool {
        matches!(self, Self::Chat | Self::Work)
    }

    pub fn runtime_family(self) -> &'static str {
        if self.is_harness_backed() {
            "native_harness_backed"
        } else {
            "native_api_direct"
        }
    }

    const COMMON_TAGS: &'static [&'static str] = &["git-memory-enabled"];

    pub fn tags_for_bear(self, bear_id: BearId) -> Vec<String> {
        let stance = self.as_str();
        let mut tags = vec![
            format!("bear:{bear_id}"),
            format!("stance:{stance}"),
            format!("bear:{bear_id}:stance:{stance}"),
        ];
        tags.extend(Self::COMMON_TAGS.iter().map(|tag| (*tag).to_string()));
        tags
    }
}

impl fmt::Display for TrustProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TrustProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "chat" => Ok(Self::Chat),
            "pair" => Ok(Self::Pair),
            "curate" => Ok(Self::Curate),
            "work" => Ok(Self::Work),
            "watch" => Ok(Self::Watch),
            other => Err(format!("unknown bear stance: {other}")),
        }
    }
}

/// Existing code shorthand retained for compatibility with profile-bound APIs.
pub type BearProfile = TrustProfile;
/// UI and protocol shorthand; runtime ownership must not be derived from this label.
pub type BearStance = TrustProfile;
