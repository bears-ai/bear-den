use std::fmt;

use uuid::Uuid;

use crate::personal_candidates::{PersonalCandidateId, PreferredName};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalFactId(pub Uuid);

impl fmt::Display for PersonalFactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonalFactState {
    Active,
    Retracted,
    /// Retained head whose canonical lifecycle or mandatory relations no longer permit reuse.
    Suppressed,
}

/// The head version remains addressable after retraction, preventing stale/ABA corrections.
/// Only an eligible active head carries a value. No generic serialization or model projection.
#[derive(Clone, Debug)]
pub struct PersonalFactHead {
    pub fact_id: PersonalFactId,
    pub candidate_id: PersonalCandidateId,
    pub state: PersonalFactState,
    pub preferred_name: Option<PreferredName>,
}

/// Immutable decision receipt, not a statement that the version remains active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub fact_id: PersonalFactId,
    pub candidate_id: PersonalCandidateId,
    pub previous_fact_id: Option<PersonalFactId>,
}
