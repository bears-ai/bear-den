//! Closed execution audiences for Den-hosted tool descriptors. A verified
//! turn origin selects one audience; a runtime context label is only a
//! derived projection for historical catalogs and audit metadata.

use serde::Serialize;

#[cfg(test)]
#[path = "audience/tests.rs"]
mod tests;

use crate::{RuntimeContextLabel, TurnExecutionOrigin};

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

    pub const fn context_label(self) -> RuntimeContextLabel {
        match self {
            Self::ChannelConversation => RuntimeContextLabel::ChannelConversation,
            Self::BrowserTaskSession | Self::ArmatureConversation => {
                RuntimeContextLabel::ArmatureConversation
            }
            Self::AuthorizedWorkRun => RuntimeContextLabel::JobRun,
            Self::InternalCuration => RuntimeContextLabel::Curation,
            Self::InboundObservation => RuntimeContextLabel::Observation,
        }
    }
}
