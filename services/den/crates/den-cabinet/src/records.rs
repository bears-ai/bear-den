//! Minimum Cabinet records: item, immutable version, collection, Mission
//! scope, source link, attachment link, review record, and their enums.
//!
//! Validation lives in constructors and `TryFrom` deserialization shims so an
//! invalid record cannot be represented: missing provenance, empty identity
//! fields, kind/locator mismatches, and version-rule violations all fail at
//! construction time.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::error::ContractViolation;
use crate::refs::{
    parse_prefixed, CabinetAttachmentRef, CabinetCollectionRef, CabinetItemRef, CabinetReviewRef,
    CabinetSourceRef, CabinetVersionRef, MissionRef,
};
use crate::scope::ActorScope;
use den_core::ids::{BearId, UserId};

/// Cabinet item kinds. `Document` is the only Phase 1 kind; the enum is open
/// for later knowledge kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ItemKind {
    Document,
}

impl ItemKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
        }
    }
}

/// Item lifecycle. Deletion is a tombstone: versions remain citable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    Archived,
    Deleted,
}

impl Lifecycle {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

/// Review state of an item version. Phase 1 direct-edit publishes every
/// version with `None`; the remaining states are reserved for Phase 2 policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    None,
    Pending,
    Approved,
    Rejected,
}

impl ReviewState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// Review decision recorded when a `pending` version leaves review (Phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    Rejected,
}

/// What a source link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Url,
    Offline,
    Artifact,
    Conversation,
    ExternalRecord,
}

impl SourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Offline => "offline",
            Self::Artifact => "artifact",
            Self::Conversation => "conversation",
            Self::ExternalRecord => "external_record",
        }
    }
}

/// The relationship a source link asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    Origin,
    Citation,
    Related,
}

impl SourceRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::Citation => "citation",
            Self::Related => "related",
        }
    }
}

/// The role of an attached artifact (open enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachmentRole {
    SourcePdf,
    GeneratedReport,
    Image,
    Data,
    Other,
}

impl AttachmentRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourcePdf => "source_pdf",
            Self::GeneratedReport => "generated_report",
            Self::Image => "image",
            Self::Data => "data",
            Self::Other => "other",
        }
    }
}

/// Collection/Mission policy knobs. Defaults express the open-wiki default:
/// Bears may write, no review gate, all kinds allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CabinetPolicy {
    /// Whether Bear actors may write under this policy.
    #[serde(default = "default_true")]
    pub bears_may_write: bool,
    /// Whether Bear writes require review before publishing (Phase 2).
    #[serde(default)]
    pub review_required: bool,
    /// Restrict item kinds; `None` allows all kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_kinds: Option<Vec<ItemKind>>,
}

fn default_true() -> bool {
    true
}

impl Default for CabinetPolicy {
    fn default() -> Self {
        Self {
            bears_may_write: true,
            review_required: false,
            allowed_kinds: None,
        }
    }
}

fn require_non_empty(
    record: &'static str,
    field: &'static str,
    value: &str,
) -> Result<(), ContractViolation> {
    if value.trim().is_empty() {
        Err(ContractViolation::EmptyField { record, field })
    } else {
        Ok(())
    }
}

/// Validate a Den artifact ref string (`artifact_` + 32 lowercase hex),
/// matching the artifact-service convention. Cabinet never mints these.
pub fn validate_artifact_ref(value: &str) -> Result<(), ContractViolation> {
    parse_prefixed("artifact", "artifact_", value)
}

/// The durable knowledge object — a wiki document or typed knowledge record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CabinetItemRecord")]
pub struct CabinetItem {
    pub cabinet_ref: CabinetItemRef,
    pub kind: ItemKind,
    pub title: String,
    /// Latest published version. Required after the first write; `None` only
    /// for an item record mid-creation before version 1 is committed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<CabinetVersionRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_ref: Option<CabinetCollectionRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_ref: Option<MissionRef>,
    pub created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub lifecycle: Lifecycle,
}

#[derive(Deserialize)]
struct CabinetItemRecord {
    cabinet_ref: CabinetItemRef,
    kind: ItemKind,
    title: String,
    #[serde(default)]
    current_version: Option<CabinetVersionRef>,
    #[serde(default)]
    collection_ref: Option<CabinetCollectionRef>,
    #[serde(default)]
    mission_ref: Option<MissionRef>,
    created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    lifecycle: Lifecycle,
}

impl TryFrom<CabinetItemRecord> for CabinetItem {
    type Error = ContractViolation;

    fn try_from(record: CabinetItemRecord) -> Result<Self, Self::Error> {
        let item = Self {
            cabinet_ref: record.cabinet_ref,
            kind: record.kind,
            title: record.title,
            current_version: record.current_version,
            collection_ref: record.collection_ref,
            mission_ref: record.mission_ref,
            created_by: record.created_by,
            created_at: record.created_at,
            lifecycle: record.lifecycle,
        };
        item.validate()?;
        Ok(item)
    }
}

impl CabinetItem {
    pub fn validate(&self) -> Result<(), ContractViolation> {
        require_non_empty("cabinet_item", "title", &self.title)
    }
}

/// An immutable snapshot of item content: the citation unit and the revision
/// history. Fields are private so a finalized version cannot be mutated;
/// construction and deserialization both enforce the version rules, and the
/// content hash is computed (or verified) rather than trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ItemVersionRecord")]
pub struct ItemVersion {
    version_ref: CabinetVersionRef,
    cabinet_ref: CabinetItemRef,
    revision: u32,
    content: String,
    content_sha256: String,
    authored_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    authored_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_version: Option<CabinetVersionRef>,
    review: ReviewState,
}

#[derive(Deserialize)]
struct ItemVersionRecord {
    version_ref: CabinetVersionRef,
    cabinet_ref: CabinetItemRef,
    revision: u32,
    content: String,
    content_sha256: String,
    authored_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    authored_at: OffsetDateTime,
    #[serde(default)]
    base_version: Option<CabinetVersionRef>,
    review: ReviewState,
}

fn sha256_hex(content: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(content.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

impl TryFrom<ItemVersionRecord> for ItemVersion {
    type Error = ContractViolation;

    fn try_from(record: ItemVersionRecord) -> Result<Self, Self::Error> {
        let computed = sha256_hex(&record.content);
        if record.content_sha256 != computed {
            return Err(ContractViolation::ContentHashMismatch {
                declared: record.content_sha256,
                computed,
            });
        }
        Self::with_review_state(
            record.version_ref,
            record.cabinet_ref,
            record.revision,
            record.content,
            record.authored_by,
            record.authored_at,
            record.base_version,
            record.review,
        )
    }
}

impl ItemVersion {
    /// Construct revision 1 of a new item under Phase 1 direct-edit.
    pub fn first(
        version_ref: CabinetVersionRef,
        cabinet_ref: CabinetItemRef,
        content: String,
        authored_by: ActorScope,
        authored_at: OffsetDateTime,
    ) -> Result<Self, ContractViolation> {
        Self::with_review_state(
            version_ref,
            cabinet_ref,
            1,
            content,
            authored_by,
            authored_at,
            None,
            ReviewState::None,
        )
    }

    /// Construct a follow-up revision under Phase 1 direct-edit. The base
    /// version is required: it is the concurrency evidence a stale-base
    /// conflict check runs against.
    pub fn direct_edit(
        version_ref: CabinetVersionRef,
        cabinet_ref: CabinetItemRef,
        revision: u32,
        content: String,
        authored_by: ActorScope,
        authored_at: OffsetDateTime,
        base_version: CabinetVersionRef,
    ) -> Result<Self, ContractViolation> {
        Self::with_review_state(
            version_ref,
            cabinet_ref,
            revision,
            content,
            authored_by,
            authored_at,
            Some(base_version),
            ReviewState::None,
        )
    }

    /// Full constructor including review state, for Phase 2 review flows and
    /// for rehydrating stored records. Enforces the structural version rules;
    /// use [`Self::ensure_phase1_direct_edit`] to additionally gate Phase 1.
    #[allow(clippy::too_many_arguments)]
    pub fn with_review_state(
        version_ref: CabinetVersionRef,
        cabinet_ref: CabinetItemRef,
        revision: u32,
        content: String,
        authored_by: ActorScope,
        authored_at: OffsetDateTime,
        base_version: Option<CabinetVersionRef>,
        review: ReviewState,
    ) -> Result<Self, ContractViolation> {
        if revision == 0 {
            return Err(ContractViolation::RevisionOutOfRange { revision });
        }
        if revision == 1 && base_version.is_some() {
            return Err(ContractViolation::UnexpectedBaseVersion);
        }
        if revision > 1 && base_version.is_none() {
            return Err(ContractViolation::MissingBaseVersion { revision });
        }
        let content_sha256 = sha256_hex(&content);
        Ok(Self {
            version_ref,
            cabinet_ref,
            revision,
            content,
            content_sha256,
            authored_by,
            authored_at,
            base_version,
            review,
        })
    }

    /// Phase 1 gate: direct-edit publishes every version with
    /// [`ReviewState::None`]; any other state is rejected until Phase 2.
    pub fn ensure_phase1_direct_edit(&self) -> Result<(), ContractViolation> {
        if self.review == ReviewState::None {
            Ok(())
        } else {
            Err(ContractViolation::ReviewStateNotAvailable {
                state: self.review.as_str(),
            })
        }
    }

    #[must_use]
    pub fn version_ref(&self) -> &CabinetVersionRef {
        &self.version_ref
    }

    #[must_use]
    pub fn cabinet_ref(&self) -> &CabinetItemRef {
        &self.cabinet_ref
    }

    #[must_use]
    pub fn revision(&self) -> u32 {
        self.revision
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    #[must_use]
    pub fn authored_by(&self) -> &ActorScope {
        &self.authored_by
    }

    #[must_use]
    pub fn authored_at(&self) -> OffsetDateTime {
        self.authored_at
    }

    #[must_use]
    pub fn base_version(&self) -> Option<&CabinetVersionRef> {
        self.base_version.as_ref()
    }

    #[must_use]
    pub fn review(&self) -> ReviewState {
        self.review
    }
}

/// An organizational grouping within the Cabinet and the policy attachment
/// point below Mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CollectionRecord")]
pub struct Collection {
    pub collection_ref: CabinetCollectionRef,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_ref: Option<MissionRef>,
    pub policy: CabinetPolicy,
    pub created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct CollectionRecord {
    collection_ref: CabinetCollectionRef,
    name: String,
    #[serde(default)]
    mission_ref: Option<MissionRef>,
    #[serde(default)]
    policy: CabinetPolicy,
    created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl TryFrom<CollectionRecord> for Collection {
    type Error = ContractViolation;

    fn try_from(record: CollectionRecord) -> Result<Self, Self::Error> {
        require_non_empty("cabinet_collection", "name", &record.name)?;
        Ok(Self {
            collection_ref: record.collection_ref,
            name: record.name,
            mission_ref: record.mission_ref,
            policy: record.policy,
            created_by: record.created_by,
            created_at: record.created_at,
        })
    }
}

/// What Cabinet requires of a Mission: identity, membership, and policy.
/// Mission lifecycle and non-Cabinet Mission behavior live elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionScope {
    pub mission_ref: MissionRef,
    pub name: String,
    pub user_members: Vec<UserId>,
    pub bear_members: Vec<BearId>,
    pub policy: CabinetPolicy,
}

impl MissionScope {
    #[must_use]
    pub fn is_user_member(&self, user_id: UserId) -> bool {
        self.user_members.contains(&user_id)
    }

    #[must_use]
    pub fn is_bear_member(&self, bear_id: BearId) -> bool {
        self.bear_members.contains(&bear_id)
    }
}

/// Provenance from a Cabinet item to material outside Cabinet. A source link
/// is provenance, not content: Cabinet never owns the bytes behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SourceLinkRecord")]
pub struct SourceLink {
    pub source_ref: CabinetSourceRef,
    pub cabinet_ref: CabinetItemRef,
    pub source_kind: SourceKind,
    pub locator: String,
    pub role: SourceRole,
    pub created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct SourceLinkRecord {
    source_ref: CabinetSourceRef,
    cabinet_ref: CabinetItemRef,
    source_kind: SourceKind,
    locator: String,
    role: SourceRole,
    created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// Validate that a locator matches its declared source kind.
pub fn validate_source_locator(kind: SourceKind, locator: &str) -> Result<(), ContractViolation> {
    require_non_empty("cabinet_source", "locator", locator)?;
    let mismatch = || ContractViolation::SourceLocatorMismatch {
        kind: kind.as_str(),
        locator: locator.to_string(),
    };
    match kind {
        SourceKind::Url => {
            if locator.starts_with("http://") || locator.starts_with("https://") {
                Ok(())
            } else {
                Err(mismatch())
            }
        }
        SourceKind::Offline => {
            // Synthetic schemes per ADR-0008 (`book://…`, `offline://…`, …);
            // anything non-web with an explicit scheme qualifies.
            let has_scheme = locator.split_once("://").is_some_and(|(scheme, rest)| {
                !scheme.is_empty() && !rest.is_empty() && scheme != "http" && scheme != "https"
            });
            if has_scheme {
                Ok(())
            } else {
                Err(mismatch())
            }
        }
        SourceKind::Artifact => validate_artifact_ref(locator).map_err(|_| mismatch()),
        SourceKind::Conversation | SourceKind::ExternalRecord => Ok(()),
    }
}

impl TryFrom<SourceLinkRecord> for SourceLink {
    type Error = ContractViolation;

    fn try_from(record: SourceLinkRecord) -> Result<Self, Self::Error> {
        validate_source_locator(record.source_kind, &record.locator)?;
        Ok(Self {
            source_ref: record.source_ref,
            cabinet_ref: record.cabinet_ref,
            source_kind: record.source_kind,
            locator: record.locator,
            role: record.role,
            created_by: record.created_by,
            created_at: record.created_at,
        })
    }
}

/// Binding from a Cabinet item to a finalized Den artifact ref (Phase 3).
/// Cabinet owns the link's item/ACL policy; the artifact registry owns payload
/// identity, lifecycle, and byte-read authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AttachmentLinkRecord")]
pub struct AttachmentLink {
    pub attachment_ref: CabinetAttachmentRef,
    pub cabinet_ref: CabinetItemRef,
    pub artifact_ref: String,
    pub role: AttachmentRole,
    pub created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct AttachmentLinkRecord {
    attachment_ref: CabinetAttachmentRef,
    cabinet_ref: CabinetItemRef,
    artifact_ref: String,
    role: AttachmentRole,
    created_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl TryFrom<AttachmentLinkRecord> for AttachmentLink {
    type Error = ContractViolation;

    fn try_from(record: AttachmentLinkRecord) -> Result<Self, Self::Error> {
        validate_artifact_ref(&record.artifact_ref)?;
        Ok(Self {
            attachment_ref: record.attachment_ref,
            cabinet_ref: record.cabinet_ref,
            artifact_ref: record.artifact_ref,
            role: record.role,
            created_by: record.created_by,
            created_at: record.created_at,
        })
    }
}

/// The record accompanying a review-state transition out of `pending`
/// (Phase 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub review_ref: CabinetReviewRef,
    pub cabinet_ref: CabinetItemRef,
    pub version_ref: CabinetVersionRef,
    pub reviewer: ActorScope,
    pub decision: ReviewDecision,
    pub rationale: String,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}
