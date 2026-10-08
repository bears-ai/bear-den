//! Session-local rationale only; fingerprints and audience consent are always reviewed afresh.

use axum_login::tower_sessions::Session;
use den_core::ids::{BearId, HatId, UserId};
use serde::{Deserialize, Serialize};

use crate::errors::CustomError;

#[derive(Deserialize, Serialize)]
pub(super) struct WorkReviewDraft {
    pub rationale: String,
}

pub(super) struct WorkReviewDraftScope {
    bear_id: BearId,
    hat_id: HatId,
    actor_id: UserId,
}

impl WorkReviewDraftScope {
    pub fn new(bear_id: BearId, hat_id: HatId, actor_id: UserId) -> Self {
        Self {
            bear_id,
            hat_id,
            actor_id,
        }
    }

    fn key(&self) -> String {
        format!(
            "hat_work_review_rationale:{}:{}:{}",
            self.bear_id, self.hat_id, self.actor_id
        )
    }

    pub async fn load(&self, session: &Session) -> Result<Option<WorkReviewDraft>, CustomError> {
        Ok(session.get(&self.key()).await?)
    }

    pub async fn save(&self, session: &Session, rationale: &str) -> Result<(), CustomError> {
        session
            .insert(
                &self.key(),
                WorkReviewDraft {
                    rationale: rationale.to_owned(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn clear(&self, session: &Session) -> Result<(), CustomError> {
        session.remove::<WorkReviewDraft>(&self.key()).await?;
        Ok(())
    }
}
