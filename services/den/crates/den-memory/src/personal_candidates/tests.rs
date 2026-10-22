//! Memory-boundary regressions. The service constructor is trusted; canonical Postgres
//! conversation ownership/locking is intentionally not inferred from SQLite note text.

use den_core::{ids::UserId, DenError};
use uuid::Uuid;

use super::*;
use crate::{
    identity_reconnection::{
        commit_review, review_reconnection, review_revocation, ReplacementConsent,
    },
    quarantine_imported_authenticated_users, resolve_authenticated_user_entity,
    test_support::new_test_store,
    AuthenticatedUserEntity, BearMemoryStore, LogicalMemoryPath, MemorySource,
};

mod atomicity;
mod authority;
mod consent;
mod deduplication;
mod isolation;

mod reconnection;
pub(crate) mod snapshots;
mod sources;
mod support;
mod values;

use snapshots::*;
use support::*;
