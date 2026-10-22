//! Owner-approved, atomic personal fact publication. Not a model tool or automatic Curate lane.
//!
//! Canonical bodies live in `memory_records`; every version is permanently classified and
//! created together with its subject, audience, and immutable publication decision. Generic
//! readers must use `generic_memory_records`, never infer access from Bear-admin membership.

mod read;
mod retract;
mod types;
mod write;

pub use read::current_preferred_name;
pub use retract::retract_preferred_name;
pub use types::{PersonalFactHead, PersonalFactId, PersonalFactState, PublicationReceipt};
pub use write::publish_preferred_name;

pub(crate) use read::{ensure_generic_mutation_allowed, publication_access};
pub(crate) use retract::retract_for_capture_disable;

pub(crate) const GENERIC_MEMORY_RECORDS_VIEW_SQL: &str = r"
CREATE VIEW IF NOT EXISTS generic_memory_records AS
    SELECT r.* FROM memory_records r
    WHERE NOT EXISTS (
        SELECT 1 FROM memory_access_rules a
        WHERE a.bear_id = r.bear_id AND a.src_memory_id = r.memory_id
    ) AND NOT EXISTS (
        SELECT 1 FROM personal_memory_publications p
        WHERE p.bear_id = r.bear_id AND p.memory_id = r.memory_id
    )
";

pub(crate) async fn recreate_generic_view(
    pool: &sqlx::SqlitePool,
) -> Result<(), den_core::DenError> {
    sqlx::query!(
        r"
        CREATE VIEW IF NOT EXISTS generic_memory_records AS
            SELECT r.* FROM memory_records r
            WHERE NOT EXISTS (
                SELECT 1 FROM memory_access_rules a
                WHERE a.bear_id = r.bear_id AND a.src_memory_id = r.memory_id
            ) AND NOT EXISTS (
                SELECT 1 FROM personal_memory_publications p
                WHERE p.bear_id = r.bear_id AND p.memory_id = r.memory_id
            )
        ",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
