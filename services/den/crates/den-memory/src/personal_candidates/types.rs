use std::fmt;

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use uuid::Uuid;

use crate::AuthenticatedUserEntity;

/// Deliberately not serializable or automatically printable into diagnostics/model context.
#[derive(Clone, PartialEq, Eq)]
pub struct PreferredName(String);

impl PreferredName {
    pub fn new(value: &str) -> Result<Self, DenError> {
        let has_control_characters = value.chars().any(char::is_control);
        let value = value.trim();
        if value.is_empty() || value.len() > 128 || has_control_characters {
            return Err(DenError::ValidationError(
                "Preferred-name candidates require 1–128 UTF-8 bytes without control characters."
                    .into(),
            ));
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PreferredName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreferredName([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalCandidateId(pub Uuid);

impl fmt::Display for PersonalCandidateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateFactType {
    PreferredName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateEvidence {
    ExplicitUserRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateRetention {
    Durable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateState {
    Pending,
    Withdrawn,
    Quarantined,
    /// Derived from the immutable publication decision, not a separately writable status.
    Published,
}

/// Only construct at the trusted service boundary after verifying the canonical conversation
/// belongs to the authenticated principal. Keep its source/rights locks through staging.
/// The SQLite boundary rechecks both the identity snapshot and exact source-note membership.
#[derive(Clone)]
pub struct VerifiedPersonalSource {
    pub(crate) binding: AuthenticatedUserEntity,
    pub(crate) conversation_id: Uuid,
    pub(crate) memory_id: Uuid,
}

impl VerifiedPersonalSource {
    pub fn for_authorized_conversation(
        binding: &AuthenticatedUserEntity,
        conversation_id: Uuid,
        source_memory_id: Uuid,
    ) -> Self {
        Self {
            binding: binding.clone(),
            conversation_id,
            memory_id: source_memory_id,
        }
    }
}

/// Owner-authorized projection. No generic Serialize implementation: callers must deliberately
/// decide which protected fields belong in a future inspection or model projection.
#[derive(Clone, Debug)]
pub struct PersonalCandidate {
    pub(super) id: PersonalCandidateId,
    pub(super) person_entity_id: String,
    pub(super) preferred_name: PreferredName,
    pub(super) state: CandidateState,
    pub(super) source_conversation_id: Uuid,
    pub(super) source_memory_id: Uuid,
    pub(super) source_bear_id: BearId,
    pub(super) source_user_id: UserId,
    pub(super) source_binding_generation: i64,
    pub(super) created_at: String,
    pub(super) withdrawn_at: Option<String>,
}

impl PersonalCandidate {
    pub fn id(&self) -> PersonalCandidateId {
        self.id
    }
    pub fn person_entity_id(&self) -> &str {
        &self.person_entity_id
    }
    pub fn preferred_name(&self) -> &PreferredName {
        &self.preferred_name
    }
    pub fn state(&self) -> CandidateState {
        self.state
    }
    pub fn fact_type(&self) -> CandidateFactType {
        CandidateFactType::PreferredName
    }
    pub fn evidence(&self) -> CandidateEvidence {
        CandidateEvidence::ExplicitUserRequest
    }
    pub fn retention(&self) -> CandidateRetention {
        CandidateRetention::Durable
    }
    pub fn source_conversation_id(&self) -> Uuid {
        self.source_conversation_id
    }
    pub fn source_memory_id(&self) -> Uuid {
        self.source_memory_id
    }
    pub fn source_bear_id(&self) -> BearId {
        self.source_bear_id
    }
    pub fn source_user_id(&self) -> UserId {
        self.source_user_id
    }
    pub fn source_binding_generation(&self) -> i64 {
        self.source_binding_generation
    }
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
    pub fn withdrawn_at(&self) -> Option<&str> {
        self.withdrawn_at.as_deref()
    }
}
