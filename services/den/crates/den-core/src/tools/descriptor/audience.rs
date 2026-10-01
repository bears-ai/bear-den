//! Closed execution audiences for Den-hosted tool descriptors. A verified
//! turn origin selects one audience; a compatibility profile is only a
//! projection for older catalogs and prompt metadata.

use serde::Serialize;

#[cfg(test)]
#[path = "audience/tests.rs"]
mod tests;

use crate::{BearProfile, TurnExecutionOrigin};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAudience {
    ChannelConversation,
    BrowserTaskSession,
    ArmatureConversation,
    AuthorizedWorkRun,
    InternalCuration,
    InboundObservation,
}

impl ToolAudience {
    pub fn from_origin(origin: TurnExecutionOrigin) -> Self {
        match origin {
            TurnExecutionOrigin::ChannelConversation => Self::ChannelConversation,
            TurnExecutionOrigin::BrowserTaskSession => Self::BrowserTaskSession,
            TurnExecutionOrigin::ArmatureConversation(_) => Self::ArmatureConversation,
            TurnExecutionOrigin::AuthorizedWorkRun(_) => Self::AuthorizedWorkRun,
            TurnExecutionOrigin::InternalCuration => Self::InternalCuration,
            TurnExecutionOrigin::InboundObservation => Self::InboundObservation,
        }
    }

    pub const fn compatibility_profile(self) -> BearProfile {
        match self {
            Self::ChannelConversation => BearProfile::Chat,
            Self::BrowserTaskSession | Self::ArmatureConversation => BearProfile::Pair,
            Self::AuthorizedWorkRun => BearProfile::Work,
            Self::InternalCuration => BearProfile::Curate,
            Self::InboundObservation => BearProfile::Watch,
        }
    }
}
