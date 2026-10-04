//! The `MemoryReviewStore` capability seam: reflection/curation review surface
//! (observations + proposals + enqueue + projections).
//!
//! The granular den proposal/observation row + param types carry lifetimes and
//! span several `den` modules; rather than migrate them wholesale, this seam
//! exposes owned request/record value types and lets the `den` impl compose the
//! underlying `create_observation` / `create_proposal` / enqueue / mark calls.
//! See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`.

use crate::{DenError, RuntimeContextLabel};
use serde_json::Value;
use uuid::Uuid;

/// Closed actions used when requesting a memory review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySuggestedAction {
    Unspecified,
    ProposeHat,
    CabinetUpdate,
    SkillReview,
    RetainProfileLocal,
    DeleteAfterReview,
    HumanReview,
    ArchiveIndex,
    TaskContext,
}

impl MemorySuggestedAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::ProposeHat => "propose_hat",
            Self::CabinetUpdate => "cabinet_update",
            Self::SkillReview => "skill_review",
            Self::RetainProfileLocal => "retain_profile_local",
            Self::DeleteAfterReview => "delete_after_review",
            Self::HumanReview => "human_review",
            Self::ArchiveIndex => "archive_index",
            Self::TaskContext => "task_context",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unspecified" => Some(Self::Unspecified),
            "propose_hat" => Some(Self::ProposeHat),
            "cabinet_update" => Some(Self::CabinetUpdate),
            "skill_review" => Some(Self::SkillReview),
            "retain_profile_local" => Some(Self::RetainProfileLocal),
            "delete_after_review" => Some(Self::DeleteAfterReview),
            "human_review" => Some(Self::HumanReview),
            "archive_index" => Some(Self::ArchiveIndex),
            "task_context" => Some(Self::TaskContext),
            _ => None,
        }
    }
}

/// Closed sensitivity labels used when requesting a memory review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySensitivity {
    Normal,
    Person,
    SecretRisk,
    ExternalUntrusted,
    Unknown,
}

impl MemorySensitivity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Person => "person",
            Self::SecretRisk => "secret_risk",
            Self::ExternalUntrusted => "external_untrusted",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "person" => Some(Self::Person),
            "secret_risk" => Some(Self::SecretRisk),
            "external_untrusted" => Some(Self::ExternalUntrusted),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Closed persisted states accepted by the memory-proposal list filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProposalStatus {
    Pending,
    Rejected,
    RetainedLocal,
    Deferred,
    Superseded,
    NeedsHumanReview,
}

impl MemoryProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Rejected => "rejected",
            Self::RetainedLocal => "retained_local",
            Self::Deferred => "deferred",
            Self::Superseded => "superseded",
            Self::NeedsHumanReview => "needs_human_review",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "rejected" => Some(Self::Rejected),
            "retained_local" => Some(Self::RetainedLocal),
            "deferred" => Some(Self::Deferred),
            "superseded" => Some(Self::Superseded),
            "needs_human_review" => Some(Self::NeedsHumanReview),
            _ => None,
        }
    }
}

/// Closed terminal states accepted when resolving a memory proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProposalResolution {
    Rejected,
    RetainedLocal,
    Deferred,
    Superseded,
    NeedsHumanReview,
}

impl MemoryProposalResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::RetainedLocal => "retained_local",
            Self::Deferred => "deferred",
            Self::Superseded => "superseded",
            Self::NeedsHumanReview => "needs_human_review",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rejected" => Some(Self::Rejected),
            "retained_local" => Some(Self::RetainedLocal),
            "deferred" => Some(Self::Deferred),
            "superseded" => Some(Self::Superseded),
            "needs_human_review" => Some(Self::NeedsHumanReview),
            _ => None,
        }
    }
}

/// Closed lifecycle states accepted when marking a memory record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLifecycleStatus {
    Active,
    Stale,
    Superseded,
    Archived,
    ArchiveCandidate,
}

impl MemoryLifecycleStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
            Self::ArchiveCandidate => "archive-candidate",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "stale" => Some(Self::Stale),
            "superseded" => Some(Self::Superseded),
            "archived" => Some(Self::Archived),
            "archive-candidate" => Some(Self::ArchiveCandidate),
            _ => None,
        }
    }
}

/// The observation fields the `observation_write` payload needs.
#[derive(Debug, Clone)]
pub struct ObservationRecord {
    pub bear_id: Uuid,
    pub observation_id: String,
    pub summary: String,
    pub salience: String,
    pub payload_ref: Option<String>,
    pub logical_path: String,
    pub status: String,
    pub proposal_id: Option<Uuid>,
}

/// A validated observation ready to persist + enqueue for curate review.
#[derive(Debug, Clone)]
pub struct ObservationWriteRequest {
    pub bear_id: Uuid,
    pub binding_id: String,
    pub observation_id: String,
    pub summary: String,
    pub salience: String,
    pub payload_ref: Option<String>,
    pub source: Value,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
}

/// Shared `conversation_events` projection inputs, computed by the executor.
#[derive(Debug, Clone)]
pub struct ProposalProjection {
    pub user_id: i32,
    pub conversation_id: Option<String>,
    pub scope_id: String,
}

/// A validated proposal resolution (`resolve_proposal`).
#[derive(Debug, Clone)]
pub struct ResolveProposalRequest {
    pub bear_id: Uuid,
    pub reviewer_profile: RuntimeContextLabel,
    pub binding_id: String,
    pub proposal_id: Uuid,
    pub status: MemoryProposalResolution,
    pub review_notes: Option<String>,
    pub decision_summary: Option<String>,
    pub projection: ProposalProjection,
}

/// A validated review request (`request_review`).
#[derive(Debug, Clone)]
pub struct RequestReviewRequest {
    pub bear_id: Uuid,
    pub source_profile: RuntimeContextLabel,
    pub binding_id: Option<String>,
    pub source_paths: Vec<String>,
    /// Locator supplied by a model; the Den store verifies it against the bound
    /// canonical source and derives the hat before persisting any trusted link.
    pub source_memory_id: Option<Uuid>,
    pub source_refs: Value,
    pub suggested_action: MemorySuggestedAction,
    pub target_ref: Option<String>,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub proposed_content: Option<String>,
    pub proposed_patch: Option<String>,
    pub refs: Value,
    pub sensitivity: MemorySensitivity,
    pub requires_human: bool,
    pub projection: ProposalProjection,
}

/// A validated lifecycle marker for an existing memory record.
#[derive(Debug, Clone)]
pub struct MarkMemoryLifecycleRequest {
    pub bear_id: Uuid,
    pub reviewer_profile: RuntimeContextLabel,
    pub binding_id: String,
    pub memory_id: String,
    pub status: MemoryLifecycleStatus,
    pub reason: Option<String>,
}

// Native async fn in trait: workspace-internal, consumed via generic bounds /
// concrete impls only (never `dyn`), so Send flows through monomorphization.
#[allow(async_fn_in_trait)]
pub trait MemoryReviewStore: Send + Sync {
    /// Idempotency check: an existing observation with this id, if any.
    async fn find_observation(
        &self,
        bear_id: Uuid,
        observation_id: &str,
    ) -> Result<Option<ObservationRecord>, DenError>;

    /// Persist the observation and enqueue it for memory-curate review.
    async fn record_observation(
        &self,
        request: ObservationWriteRequest,
    ) -> Result<ObservationRecord, DenError>;

    /// Proposals matching the closed persisted status domain, if filtered.
    async fn list_proposals(
        &self,
        bear_id: Uuid,
        status: Option<MemoryProposalStatus>,
        limit: i64,
    ) -> Result<Value, DenError>;

    /// A single proposal as serialized JSON, if found.
    async fn get_proposal(
        &self,
        bear_id: Uuid,
        proposal_id: Uuid,
    ) -> Result<Option<Value>, DenError>;

    /// Resolve a proposal and project the event; returns the proposal JSON.
    async fn resolve_proposal(&self, request: ResolveProposalRequest) -> Result<Value, DenError>;

    /// Create a review-request proposal and project the event; returns the proposal JSON.
    async fn request_review(&self, request: RequestReviewRequest) -> Result<Value, DenError>;

    /// Mark an existing memory record's lifecycle status; returns the updated record JSON.
    async fn mark_memory_lifecycle(
        &self,
        request: MarkMemoryLifecycleRequest,
    ) -> Result<Value, DenError>;
}
