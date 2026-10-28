//! Protected, server-only personal-memory candidate storage. Not a model tool or publisher.
//!
//! Values live outside generic proposal/observation/record tables so ordinary inspection,
//! synthesis and recall cannot accidentally load them. Capture starts disabled. Every read
//! and mutation requires a current exact authenticated person binding and generation.
//! The service boundary must lock current membership and, for conversation evidence,
//! canonical conversation ownership through staging. A locator, name, admin role, or imported
//! account is not a grant. Direct human settings use a distinct verified owner-request boundary.

mod authority;
mod owner;
mod read;
mod review;
mod settings;
mod source;
mod stage;
mod types;

pub use owner::stage_owner_preferred_name;
pub use read::{get_candidate, list_candidates, withdraw_candidate};
pub use review::review_candidate;
pub use settings::{
    capture_enabled, capture_setting, set_capture_enabled, set_capture_enabled_if_current,
};
pub use stage::stage_preferred_name;
pub use types::{
    CandidateEvidence, CandidateFactType, CandidateRetention, CandidateReviewDecision,
    CandidateReviewId, CandidateReviewReceipt, CandidateSource, CandidateState, CaptureSetting,
    PersonalCandidate, PersonalCandidateId, PersonalCaptureRevision, PreferredName,
    VerifiedOwnerSource, VerifiedPersonalSource,
};

pub(crate) use authority::{ensure_current, write_transaction};
pub(crate) use read::get_inside;
pub(crate) use settings::{enabled, quarantine_imported, quarantine_person};
pub(crate) use source::candidate_source_is_eligible;

#[cfg(test)]
pub(crate) mod tests;
