//! Per-Bear refresh authority. Empty entries are persistent generation tombstones.

use std::{collections::hash_map::Entry, sync::Arc};

use den_core::{DenError, ModelAvailabilityFailureKind};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

use super::{availability::catalog_failure, BifrostCatalogSnapshot, BifrostClient};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct CredentialFingerprint([u8; 32]);

impl CredentialFingerprint {
    pub fn for_key(key: &str) -> Self {
        Self(Sha256::digest(key.trim().as_bytes()).into())
    }
}

#[derive(Clone)]
struct AuthenticatedCatalog {
    snapshot: BifrostCatalogSnapshot,
    credential: CredentialFingerprint,
}

#[derive(Default)]
pub(super) struct BearCatalogState {
    generation: u64,
    exhausted: bool,
    pending: bool,
    catalog: Option<AuthenticatedCatalog>,
    refresh_queue: Arc<Mutex<()>>,
}

#[derive(Clone, Copy)]
pub(super) struct RefreshPermit {
    bear_id: Uuid,
    generation: u64,
    cache_was_idle: bool,
}

pub(super) enum RefreshFailure {
    Failed(DenError),
    /// Only a completed, current refresh can authorize explicit-pin continuity.
    Outage(Option<BifrostCatalogSnapshot>),
    Superseded,
}

impl RefreshFailure {
    pub fn into_den(self) -> DenError {
        match self {
            Self::Failed(error) => error,
            Self::Outage(_) | Self::Superseded => {
                catalog_failure(ModelAvailabilityFailureKind::CatalogUnavailable)
            }
        }
    }
}

fn state_unavailable() -> RefreshFailure {
    RefreshFailure::Failed(catalog_failure(
        ModelAvailabilityFailureKind::CatalogUnavailable,
    ))
}

#[cfg(test)]
#[path = "catalog_state_tests.rs"]
mod tests;

impl BifrostClient {
    /// Queue operations on the existing Bear state, not on a second catalog or
    /// credential authority. The guard spans lookup, HTTP, recheck and publication.
    pub(super) async fn enter_catalog_refresh(
        &self,
        bear_id: Uuid,
    ) -> Result<(OwnedMutexGuard<()>, bool), RefreshFailure> {
        let queue = {
            let mut states = self
                .bear_catalogs
                .write()
                .map_err(|_| state_unavailable())?;
            Arc::clone(&states.entry(bear_id).or_default().refresh_queue)
        };
        match Arc::clone(&queue).try_lock_owned() {
            Ok(guard) => Ok((guard, false)),
            Err(_) => Ok((queue.lock_owned().await, true)),
        }
    }

    /// Reserve authority before the first asynchronous credential lookup. Never
    /// remove a state entry: invalidation must not reset its generation (ABA).
    pub(super) fn begin_catalog_refresh(
        &self,
        bear_id: Uuid,
    ) -> Result<RefreshPermit, RefreshFailure> {
        let mut states = self
            .bear_catalogs
            .write()
            .map_err(|_| state_unavailable())?;
        let state = match states.entry(bear_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(BearCatalogState::default()),
        };
        let cache_was_idle = !state.pending;
        state.pending = true;
        if state.exhausted {
            return Err(state_unavailable());
        }
        let Some(generation) = state.generation.checked_add(1) else {
            state.exhausted = true;
            state.catalog = None;
            return Err(state_unavailable());
        };
        state.generation = generation;
        Ok(RefreshPermit {
            bear_id,
            generation,
            cache_was_idle,
        })
    }

    fn with_current_catalog<T>(
        &self,
        permit: RefreshPermit,
        operation: impl FnOnce(&mut BearCatalogState) -> T,
    ) -> Result<T, RefreshFailure> {
        let mut states = self
            .bear_catalogs
            .write()
            .map_err(|_| state_unavailable())?;
        let state = states
            .get_mut(&permit.bear_id)
            .ok_or(RefreshFailure::Superseded)?;
        if state.exhausted || state.generation != permit.generation {
            return Err(RefreshFailure::Superseded);
        }
        Ok(operation(state))
    }

    pub(super) fn require_current_refresh(
        &self,
        permit: RefreshPermit,
    ) -> Result<(), RefreshFailure> {
        self.with_current_catalog(permit, |_| ())
    }

    pub(super) fn bind_catalog_credential(
        &self,
        permit: RefreshPermit,
        credential: CredentialFingerprint,
    ) -> Result<(), RefreshFailure> {
        self.with_current_catalog(permit, |state| {
            if state
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.credential != credential)
            {
                state.catalog = None;
            }
        })
    }

    pub(super) fn invalidate_catalog(&self, permit: RefreshPermit) -> Result<(), RefreshFailure> {
        self.with_current_catalog(permit, |state| {
            state.catalog = None;
            state.pending = false;
        })
    }

    pub(super) fn publish_catalog(
        &self,
        permit: RefreshPermit,
        credential: CredentialFingerprint,
        snapshot: BifrostCatalogSnapshot,
    ) -> Result<BifrostCatalogSnapshot, RefreshFailure> {
        self.with_current_catalog(permit, |state| {
            state.catalog = Some(AuthenticatedCatalog {
                snapshot: snapshot.clone(),
                credential,
            });
            state.pending = false;
            snapshot
        })
    }

    pub(super) fn finish_catalog_outage(
        &self,
        permit: RefreshPermit,
        credential: CredentialFingerprint,
    ) -> Result<RefreshFailure, RefreshFailure> {
        self.with_current_catalog(permit, |state| {
            state.pending = false;
            let cached = state
                .catalog
                .as_ref()
                .filter(|catalog| catalog.credential == credential)
                .map(|catalog| catalog.snapshot.clone());
            RefreshFailure::Outage(cached)
        })
    }

    pub(super) fn finish_cached_catalog(
        &self,
        permit: RefreshPermit,
        credential: CredentialFingerprint,
    ) -> Result<Option<BifrostCatalogSnapshot>, RefreshFailure> {
        self.with_current_catalog(permit, |state| {
            // A TTL read cannot cancel an in-flight authoritative refresh and
            // then authorize itself using the pre-refresh positive snapshot.
            if !permit.cache_was_idle {
                return None;
            }
            let cached = state
                .catalog
                .as_ref()
                .filter(|catalog| catalog.credential == credential)
                .filter(|catalog| !catalog.snapshot.stale)
                .filter(|catalog| {
                    catalog.snapshot.fetched_at.is_some_and(|fetched_at| {
                        OffsetDateTime::now_utc() - fetched_at < Duration::hours(1)
                    })
                })
                .map(|catalog| catalog.snapshot.clone());
            if cached.is_some() {
                state.pending = false;
            }
            cached
        })
    }

    /// Observation only; execution continuity must use the fenced fresh helper.
    /// In-flight refreshes never expose a prior snapshot as cached authority.
    pub fn cached_bear_catalog_snapshot(&self, bear_id: Uuid) -> Option<BifrostCatalogSnapshot> {
        self.bear_catalogs.read().ok().and_then(|states| {
            states
                .get(&bear_id)
                .filter(|state| !state.pending)
                .and_then(|state| state.catalog.as_ref())
                .map(|catalog| catalog.snapshot.clone())
        })
    }
}
