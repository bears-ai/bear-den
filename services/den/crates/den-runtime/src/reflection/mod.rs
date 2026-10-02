//! Reflection/curation worker subsystem: the memory-curate conductor loop,
//! compaction archive-harvest pass, and conversation-lane persistence.

pub mod archive_harvest;
pub mod briefing_source;
pub use briefing_source::ReflectionRunId;
pub mod conductor;
pub mod conversations;
mod curate_retry;
