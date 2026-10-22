use std::fmt;

use den_core::{
    ids::{BearId, UserId},
    TurnExecutionOrigin,
};
use den_memory::{
    personal_facts::{PersonalFactId, PersonalFactState},
    AuthenticatedUserEntity,
};
use uuid::Uuid;

/// Server-owned policy only; this does not register or enable a runtime consumer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PersonalContextMode {
    #[default]
    DefaultDisabled,
    OwnerApprovedPreferredName,
}

/// Construct only from a trusted server principal and its canonical conversation.
/// Locators and the execution origin do not themselves establish ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalContextSource {
    pub bear_id: BearId,
    pub user_id: UserId,
    pub conversation_id: Uuid,
    pub origin: TurnExecutionOrigin,
}

/// A value-free snapshot, not an access grant or a lease on future rendering.
/// Every render rechecks source rights, the retained binding and the exact head state.
#[derive(Clone)]
pub struct PersonalContextReference {
    pub(super) source: PersonalContextSource,
    pub(super) binding: AuthenticatedUserEntity,
    pub(super) expected: Option<(PersonalFactId, PersonalFactState)>,
}

impl PersonalContextReference {
    /// A version locator for private dependency handling, never generic diagnostics
    /// or evidence that the fact remains active or authorized.
    pub fn fact_id(&self) -> Option<PersonalFactId> {
        self.expected.map(|(fact_id, _)| fact_id)
    }
}

impl fmt::Debug for PersonalContextReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PersonalContextReference([redacted])")
    }
}

/// Ephemeral protected text; callers must not persist or emit it as diagnostics.
/// Rendering establishes no authority for a later provider request.
#[derive(Clone)]
pub struct RenderedPersonalContext(String);

impl RenderedPersonalContext {
    pub(super) fn new(text: String) -> Self {
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }
}

impl fmt::Debug for RenderedPersonalContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RenderedPersonalContext([redacted])")
    }
}
