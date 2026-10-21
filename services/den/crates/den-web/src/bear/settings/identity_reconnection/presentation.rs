//! A derived review fingerprint, never independent identity authority or browser input.
use super::{
    authority::{AccountLabel, LockedAuthority},
    session_review::unavailable,
};
use crate::errors::CustomError;
use den_memory::identity_reconnection::{IdentityReconnectionReview, UserIdentitySummary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IdentityStamp {
    entity_id: String,
    sha256: [u8; 32],
}

impl IdentityStamp {
    fn new(identity: &UserIdentitySummary) -> Result<Self, CustomError> {
        let bytes = serde_json::to_vec(identity).map_err(|_| {
            CustomError::System("Could not fingerprint identity review metadata.".into())
        })?;
        Ok(Self {
            entity_id: identity.entity_id.clone(),
            sha256: Sha256::digest(bytes).into(),
        })
    }
}

/// Bind what the human sees to the exact prepared metadata, including epochs and
/// safe account labels. Core still owns version validation and durable replay rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReviewedMetadata {
    actor: AccountLabel,
    destination: Option<AccountLabel>,
    selected: IdentityStamp,
    displaced: Option<IdentityStamp>,
}

impl ReviewedMetadata {
    pub fn capture(
        actor: AccountLabel,
        destination: Option<AccountLabel>,
        entity_id: &str,
        identities: &[UserIdentitySummary],
    ) -> Result<Self, CustomError> {
        let selected = identities
            .iter()
            .find(|identity| identity.entity_id == entity_id)
            .ok_or_else(unavailable)?;
        let displaced = destination.as_ref().and_then(|account| {
            identities
                .iter()
                .find(|identity| identity.active_user_id == Some(account.user_id))
        });
        Ok(Self {
            actor,
            destination,
            selected: IdentityStamp::new(selected)?,
            displaced: displaced.map(IdentityStamp::new).transpose()?,
        })
    }

    pub fn authorize(
        &self,
        locked: &LockedAuthority,
        expected: &IdentityReconnectionReview,
        identities: &[UserIdentitySummary],
    ) -> Result<(), CustomError> {
        if self.actor.user_id != expected.actor_user_id()
            || self.destination.as_ref().map(|account| account.user_id)
                != expected.destination_user_id()
            || self.selected.entity_id != expected.entity_id()
            || self
                .displaced
                .as_ref()
                .map(|identity| identity.entity_id.as_str())
                != expected.displaced_entity_id()
            || *self
                != Self::capture(
                    locked.actor.clone(),
                    locked.destination.clone(),
                    expected.entity_id(),
                    identities,
                )?
        {
            return Err(CustomError::ValidationError(
                "Identity metadata, account labels, or mapping versions changed since preparation. Nothing was confirmed by this request. Return to the identity list and prepare a fresh review.".into(),
            ));
        }
        Ok(())
    }
}
