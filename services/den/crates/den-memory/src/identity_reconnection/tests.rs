//! Regression coverage for the trusted restore boundary, not caller authentication.

use den_core::{ids::UserId, DenError};
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{
    append_relation, attach_handle, create_entity, detach_handle, get_entity, list_handles,
    merge_entities, quarantine_imported_authenticated_users, record_visible,
    resolve_authenticated_user_entity, set_canonical_ref, set_resolution, split_entity,
    test_support::new_test_store, AccessContext, BearMemoryStore, EntityTrust, LogicalMemoryPath,
    ResolutionState,
};

mod atomicity;
mod generations;
mod origins;
mod provenance;
mod receipts;
mod replacement;
mod restoration;
mod reviews;
mod support;

use support::*;
