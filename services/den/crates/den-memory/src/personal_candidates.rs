//! Protected, server-only personal-memory candidate storage. Not a model tool or publisher.
//!
//! Values live outside generic proposal/observation/record tables so ordinary inspection,
//! synthesis and recall cannot accidentally load them. Capture starts disabled. Every read
//! and mutation requires a current exact authenticated person binding and generation.
//! The service boundary must verify canonical conversation ownership and keep that authority
//! locked through staging. A source UUID, name, admin role, or imported account is not a grant.

mod authority;
mod read;
mod settings;
mod stage;
mod types;

pub use read::{get_candidate, list_candidates, withdraw_candidate};
pub use settings::{capture_enabled, set_capture_enabled};
pub use stage::stage_preferred_name;
pub use types::{
    CandidateEvidence, CandidateFactType, CandidateRetention, CandidateState, PersonalCandidate,
    PersonalCandidateId, PreferredName, VerifiedPersonalSource,
};

pub(crate) use settings::{quarantine_imported, quarantine_person};

#[cfg(test)]
mod tests;
