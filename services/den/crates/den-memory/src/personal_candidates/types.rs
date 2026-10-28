use std::fmt;

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use uuid::Uuid;

use crate::AuthenticatedUserEntity;

/// Opaque owner-setting CAS token, allocated from the Bear's sequence on every consent write.
/// Zero denotes an absent or migrated pre-revision setting, never an opt-in grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalCaptureRevision(i64);

impl PersonalCaptureRevision {
    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for PersonalCaptureRevision {
    type Error = DenError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            return Err(DenError::ValidationError(
                "Personal-memory capture revisions must be nonnegative.".into(),
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureSetting {
    pub enabled: bool,
    pub revision: PersonalCaptureRevision,
}

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
pub struct CandidateReviewId(pub Uuid);

impl fmt::Display for CandidateReviewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateReviewDecision {
    Reject,
    Defer,
    /// Explicit unpublished-candidate closure, not durable-fact TTL or hard erasure.
    Expire,
    Resume,
}

impl CandidateReviewDecision {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Defer => "defer",
            Self::Expire => "expire",
            Self::Resume => "resume",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, DenError> {
        match value {
            "reject" => Ok(Self::Reject),
            "defer" => Ok(Self::Defer),
            "expire" => Ok(Self::Expire),
            "resume" => Ok(Self::Resume),
            _ => Err(DenError::ValidationError(
                "Invalid protected personal-memory candidate review.".into(),
            )),
        }
    }
}

/// Immutable reference-only decision receipt, not a claim about the candidate's active status.
/// Replaying it never restores consent, source eligibility, or a previously closed candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateReviewReceipt {
    pub review_id: CandidateReviewId,
    pub candidate_id: PersonalCandidateId,
    pub decision: CandidateReviewDecision,
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
    Rejected,
    Deferred,
    Expired,
    /// Derived from the immutable publication decision, not a separately writable status.
    Published,
}

/// Only construct at the trusted service boundary with current membership locks held.
/// Conversation provenance additionally requires verified canonical conversation ownership;
/// direct settings provenance comes only from the distinct `VerifiedOwnerSource` boundary.
/// SQLite rechecks the identity snapshot, consent and any exact source-note membership.
#[derive(Clone)]
pub struct VerifiedPersonalSource {
    pub(crate) binding: AuthenticatedUserEntity,
    pub(crate) source: CandidateSource,
}

impl VerifiedPersonalSource {
    pub fn for_authorized_conversation(
        binding: &AuthenticatedUserEntity,
        conversation_id: Uuid,
        source_memory_id: Uuid,
    ) -> Self {
        Self {
            binding: binding.clone(),
            source: CandidateSource::Conversation {
                conversation_id,
                memory_id: source_memory_id,
            },
        }
    }
}

/// Direct human-settings authority, constructed only after the service locks current membership.
/// A request locator is not itself a grant; SQLite rechecks the exact binding and capture consent.
#[derive(Clone)]
pub struct VerifiedOwnerSource {
    pub(crate) binding: AuthenticatedUserEntity,
    pub(crate) request_id: Uuid,
    pub(crate) capture_revision: Option<PersonalCaptureRevision>,
}

impl VerifiedOwnerSource {
    pub fn for_authorized_owner_request(
        binding: &AuthenticatedUserEntity,
        request_id: Uuid,
    ) -> Self {
        Self {
            binding: binding.clone(),
            request_id,
            capture_revision: None,
        }
    }

    /// Bind a management request to the consent snapshot shown to the owner.
    /// Staging checks this revision before either replaying or inserting a candidate.
    pub fn with_capture_revision(mut self, expected: PersonalCaptureRevision) -> Self {
        self.capture_revision = Some(expected);
        self
    }
}

impl From<&VerifiedOwnerSource> for VerifiedPersonalSource {
    fn from(source: &VerifiedOwnerSource) -> Self {
        Self {
            binding: source.binding.clone(),
            source: CandidateSource::OwnerRequest {
                request_id: source.request_id,
            },
        }
    }
}

/// Immutable protected provenance, not authority inferred from generic memory or transcript text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    Conversation {
        conversation_id: Uuid,
        memory_id: Uuid,
    },
    OwnerRequest {
        request_id: Uuid,
    },
}

/// Owner-authorized projection. No generic Serialize implementation: callers must deliberately
/// decide which protected fields belong in a future inspection or model projection.
#[derive(Clone, Debug)]
pub struct PersonalCandidate {
    pub(super) id: PersonalCandidateId,
    pub(super) person_entity_id: String,
    pub(super) preferred_name: PreferredName,
    pub(super) state: CandidateState,
    pub(super) latest_review: Option<CandidateReviewReceipt>,
    pub(super) source: CandidateSource,
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
    pub fn latest_review_id(&self) -> Option<CandidateReviewId> {
        self.latest_review.map(|review| review.review_id)
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
    pub fn source(&self) -> CandidateSource {
        self.source
    }
    pub fn source_conversation_id(&self) -> Option<Uuid> {
        match self.source {
            CandidateSource::Conversation {
                conversation_id, ..
            } => Some(conversation_id),
            CandidateSource::OwnerRequest { .. } => None,
        }
    }
    pub fn source_memory_id(&self) -> Option<Uuid> {
        match self.source {
            CandidateSource::Conversation { memory_id, .. } => Some(memory_id),
            CandidateSource::OwnerRequest { .. } => None,
        }
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
