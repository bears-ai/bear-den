//! Append-only page discussion, separate from Cabinet's publication history.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{ActorScope, CabinetDiscussionRef, CabinetItemRef};

/// Maximum trimmed discussion content length, in Unicode scalar values (not bytes).
pub const MAX_DISCUSSION_CHARS: usize = 16_000;

/// An immutable discussion entry with the explicit, historical actor provenance.
/// Content is plain text; rendering surfaces must escape it, not trust it as HTML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionEntry {
    pub discussion_ref: CabinetDiscussionRef,
    pub cabinet_ref: CabinetItemRef,
    pub content: String,
    pub authored_by: ActorScope,
    #[serde(with = "time::serde::rfc3339")]
    pub authored_at: OffsetDateTime,
}
