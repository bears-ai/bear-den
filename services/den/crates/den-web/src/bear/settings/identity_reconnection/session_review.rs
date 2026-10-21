//! Session-owned review state: browser inputs never supply the core expected version.
use super::presentation::ReviewedMetadata;
use crate::errors::CustomError;
use axum_login::tower_sessions::Session;
use den_core::ids::{BearId, UserId};
use den_memory::identity_reconnection::{IdentityReconnectionReview, ReplacementConsent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(super) const PREPARE_KEY: &str = "identity_reconnection_prepare";
pub(super) const REVIEW_KEY: &str = "identity_reconnection_review";
const MAX_AGE_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ReviewNonce(pub Uuid);

impl ReviewNonce {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub(super) fn session_id(session: &Session) -> Result<String, CustomError> {
    session.id().map(|id| id.to_string()).ok_or_else(|| {
        CustomError::Authentication("Sign in again before reviewing identity reconnection.".into())
    })
}

pub(super) fn unavailable() -> CustomError {
    CustomError::ValidationError(
        "This identity review expired, changed, or belongs to another session. Return to the identity list and prepare a fresh review. No change was authorized by this request.".into(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ReviewScope {
    bear_id: BearId,
    actor: UserId,
    session_id: String,
    issued_at: i64,
}

impl ReviewScope {
    pub fn new(bear_id: BearId, actor: UserId, session_id: String) -> Self {
        Self {
            bear_id,
            actor,
            session_id,
            issued_at: now(),
        }
    }

    pub fn authorize(
        &self,
        bear_id: BearId,
        actor: UserId,
        session_id: &str,
        at: i64,
    ) -> Result<(), CustomError> {
        if self.bear_id != bear_id
            || self.actor != actor
            || self.session_id != session_id
            || !at
                .checked_sub(self.issued_at)
                .is_some_and(|age| (0..MAX_AGE_SECONDS).contains(&age))
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PrepareToken {
    pub nonce: ReviewNonce,
    scope: ReviewScope,
}

impl PrepareToken {
    pub fn new(scope: ReviewScope) -> Self {
        Self {
            nonce: ReviewNonce::new(),
            scope,
        }
    }

    pub fn authorize(
        &self,
        nonce: ReviewNonce,
        bear_id: BearId,
        actor: UserId,
        session_id: &str,
    ) -> Result<(), CustomError> {
        if self.nonce != nonce {
            return Err(unavailable());
        }
        self.scope.authorize(bear_id, actor, session_id, now())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PendingReview {
    pub nonce: ReviewNonce,
    // Only exposed by the rendered review, not in the prepare redirect. No GET
    // writes are needed, so a late GET cannot resurrect a replaced session pointer.
    pub confirmation_token: ReviewNonce,
    scope: ReviewScope,
    pub expected: IdentityReconnectionReview,
    pub metadata: ReviewedMetadata,
}

impl PendingReview {
    pub fn new(
        scope: ReviewScope,
        expected: IdentityReconnectionReview,
        metadata: ReviewedMetadata,
    ) -> Self {
        Self {
            nonce: ReviewNonce::new(),
            confirmation_token: ReviewNonce::new(),
            scope,
            expected,
            metadata,
        }
    }

    pub fn authorize(
        &self,
        nonce: ReviewNonce,
        bear_id: BearId,
        actor: UserId,
        session_id: &str,
    ) -> Result<(), CustomError> {
        let at = now();
        self.scope.authorize(bear_id, actor, session_id, at)?;
        if self.nonce != nonce
            || self.expected.bear_id() != bear_id
            || self.expected.actor_user_id() != actor
            || self.expected.expires_at_unix() <= at
        {
            return Err(unavailable());
        }
        Ok(())
    }

    pub fn consent(
        &self,
        token: ReviewNonce,
        acknowledged: bool,
        replace_acknowledged: bool,
    ) -> Result<ReplacementConsent, CustomError> {
        if token != self.confirmation_token {
            return Err(unavailable());
        }
        if !acknowledged {
            return Err(CustomError::ValidationError("Confirm the selected identity/account mapping or revocation and its audience consequences. Nothing was changed.".into()));
        }
        replacement_consent(
            self.expected.displaced_entity_id().is_some(),
            replace_acknowledged,
        )
    }
}

pub(super) fn replacement_consent(
    displaced: bool,
    acknowledged: bool,
) -> Result<ReplacementConsent, CustomError> {
    if displaced {
        if !acknowledged {
            return Err(CustomError::ValidationError("Separately acknowledge parking the destination account's current identity. All records and relations are preserved; no facts are lost or combined. Nothing was changed.".into()));
        }
        Ok(ReplacementConsent::PreserveDisplacedIdentity)
    } else {
        Ok(ReplacementConsent::NoReplacement)
    }
}
