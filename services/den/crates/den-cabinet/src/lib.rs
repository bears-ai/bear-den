//! `den-cabinet`: provider-neutral contract types for Cabinet, Den's shared
//! knowledge layer.
//!
//! This crate is the Phase 0 deliverable of
//! `docs/roadmap/CABINET_IMPLEMENTATION_PLAN.md`; the normative contract is
//! `docs/architecture/cabinet-contract.md`. It defines the typed refs, minimum
//! records, operation inputs, and authorization vocabulary that every Cabinet
//! provider and client must honor. No storage, transport, or policy engine
//! lives here — the Phase 1 facade builds on these types.

pub mod discussion;
pub mod error;
pub mod ops;
pub mod records;
pub mod refs;
pub mod scope;

pub use discussion::{DiscussionEntry, MAX_DISCUSSION_CHARS};
pub use error::{CabinetError, ContractViolation};
pub use ops::{
    ArchiveItemRequest, CreateItemRequest, HistoryRequest, ItemSummary, LinkAttachmentRequest,
    LinkSourceRequest, NewSourceLink, OrganizeRequest, ReadRequest, RestoreItemRequest,
    ReviewRequest, SearchFilters, SearchRequest, UnlinkAttachmentRequest, UnlinkSourceRequest,
    UpdateItemRequest, VersionSummary,
};
pub use records::{
    validate_artifact_ref, validate_source_locator, AttachmentLink, AttachmentRole, CabinetItem,
    CabinetPolicy, Collection, ItemKind, ItemVersion, Lifecycle, MissionScope, ReviewDecision,
    ReviewRecord, ReviewState, SourceKind, SourceLink, SourceRole,
};
pub use refs::{
    CabinetAttachmentRef, CabinetCollectionRef, CabinetDiscussionRef, CabinetItemRef,
    CabinetReviewRef, CabinetSourceRef, CabinetVersionRef, MissionRef,
};
pub use scope::{Actor, ActorScope, Authority, AuthorizationOutcome, DenialReason};
