use den_core::ids::{BearId, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentityOrigin {
    pub source_host: String,
    pub source_bear_id: String,
    pub source_user_id: UserId,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub was_exporter: bool,
    pub recorded_at: String,
}

/// Selection metadata only: does not disclose memory contents or grant recall access.
#[derive(Clone, Debug, Serialize)]
pub struct UserIdentitySummary {
    pub entity_id: String,
    pub display_name: Option<String>,
    pub active_user_id: Option<UserId>,
    pub binding_version: i64,
    pub origins: Vec<UserIdentityOrigin>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum AuthorityState {
    Active,
    Quarantined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct IdentitySnapshot {
    pub entity_id: String,
    pub user_id: UserId,
    pub state: AuthorityState,
    pub generation: i64,
    pub display_name: Option<String>,
    pub origins: Vec<UserIdentityOrigin>,
}

/// A server-generated, short-lived snapshot. Serialize only into trusted session storage,
/// never accept this value (including its actor and expected versions) from an HTTP client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityReconnectionReview {
    pub(super) id: Uuid,
    pub(super) bear_id: BearId,
    pub(super) actor: UserId,
    pub(super) destination: Option<UserId>,
    pub(super) selected: IdentitySnapshot,
    pub(super) displaced: Option<IdentitySnapshot>,
    pub(super) expires_at: i64,
}

impl IdentityReconnectionReview {
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn bear_id(&self) -> BearId {
        self.bear_id
    }

    pub fn actor_user_id(&self) -> UserId {
        self.actor
    }

    pub fn entity_id(&self) -> &str {
        &self.selected.entity_id
    }

    pub fn destination_user_id(&self) -> Option<UserId> {
        self.destination
    }

    pub fn displaced_entity_id(&self) -> Option<&str> {
        self.displaced
            .as_ref()
            .map(|identity| identity.entity_id.as_str())
    }

    pub fn expires_at_unix(&self) -> i64 {
        self.expires_at
    }
}

/// Replacement is never inferred, even if the displaced identity seems unused.
#[derive(Clone, Copy, Debug)]
pub enum ReplacementConsent {
    NoReplacement,
    /// Park the old attachment with all entity, memory, candidate and provenance references intact.
    /// It can be selected for a later reconnection; no identities or audiences are merged.
    PreserveDisplacedIdentity,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentityReceiptHistory {
    pub receipts: Vec<IdentityReconnectionReceipt>,
    /// Malformed entries in the bounded history window; these do not block identity management.
    pub unreadable_entries: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentityReconnectionReceipt {
    pub receipt_id: Uuid,
    pub recorded_bear_id: String,
    pub actor_user_id: UserId,
    pub previous_user_id: UserId,
    pub prior_generation: i64,
    pub resulting_generation: i64,
    pub entity_id: String,
    pub destination_user_id: Option<UserId>,
    pub displaced_entity_id: Option<String>,
    pub recorded_at: String,
}
