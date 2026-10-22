//! Owner-approval and privacy boundary regressions; conversation authorization belongs to
//! the service. Fixtures here never treat source text, raw UUIDs, or Bear membership as grants.

use den_core::{ids::UserId, DenError};
use serde_json::{json, Value};
use uuid::Uuid;

use super::*;
use crate::{
    personal_candidates::tests::snapshots::{
        generic_snapshot, snapshot, GenericSnapshot, Snapshot,
    },
    personal_candidates::{
        capture_enabled, get_candidate, list_candidates, set_capture_enabled, stage_preferred_name,
        withdraw_candidate, CandidateState, PersonalCandidate, PersonalCandidateId, PreferredName,
        VerifiedPersonalSource,
    },
    resolve_authenticated_user_entity,
    test_support::new_test_store,
    AccessContext, AuthenticatedUserEntity, BearMemoryStore, LogicalMemoryPath, MemorySource,
};

mod atomicity;
mod authority;
mod fixtures;
mod isolation;
mod lifecycle;
mod publication;
mod support;

use support::*;
