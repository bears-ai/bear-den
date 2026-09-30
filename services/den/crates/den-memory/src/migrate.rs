use sqlx::SqlitePool;

use den_core::DenError;

/// Upgrade per-Bear SQLite files created before profile and source/hat scope changes.
pub async fn migrate_bear_sqlite_schema(pool: &SqlitePool) -> Result<(), DenError> {
    let columns =
        sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info('memory_records')")
            .fetch_all(pool)
            .await
            .map_err(|e| DenError::System(format!("bear sqlite pragma failed: {e}")))?;
    if columns.iter().any(|c| c == "scope_role") && !columns.iter().any(|c| c == "scope_profile") {
        sqlx::query("ALTER TABLE memory_records RENAME COLUMN scope_role TO scope_profile")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("rename scope_role failed: {e}")))?;
    }
    if columns.iter().any(|c| c == "author_role") && !columns.iter().any(|c| c == "author_profile")
    {
        sqlx::query("ALTER TABLE memory_records RENAME COLUMN author_role TO author_profile")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("rename author_role failed: {e}")))?;
    }

    // Additive bi-temporal event-time columns (ADR-0041 / DERIVED_RECALL Phase 3.5).
    add_adr0041_record_columns_if_missing(pool, &columns).await?;
    add_verified_proposal_hat_column_if_missing(pool).await?;
    ensure_memory_harvest_marks(pool).await?;

    // Retire the legacy record→record `memory_links` base table; relations now live in
    // `memory_relations`/`memory_access_rules` with `memory_links` as a read view (ADR-0042 §7).
    retire_legacy_memory_links_table(pool).await?;

    let table_sql: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'memory_records'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| DenError::System(format!("bear sqlite table info failed: {e}")))?;

    let needs_scope_vocab_rebuild = table_sql
        .as_deref()
        .map(|sql| sql.contains("'role_local'") || !sql.contains("'source_local'"))
        .unwrap_or(false);

    if needs_scope_vocab_rebuild {
        // The rebuild path already produces a table without `entity_ref`.
        rebuild_memory_records_scope_vocab(pool).await?;
        normalize_legacy_import_hashed_kinds(pool).await?;
        return Ok(());
    }

    sqlx::query(
        "UPDATE memory_records SET scope_type = 'profile_local' WHERE scope_type = 'role_local'",
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("migrate scope_type values failed: {e}")))?;

    // Retire the vestigial `entity_ref` column (never populated); relations carry aboutness now.
    drop_entity_ref_column_if_present(pool, &columns).await?;

    // Early legacy imports used the hashed filename stem (`mem_...`) as `kind` for
    // generic role-local entries. Normalize those to `note`; directory-specific
    // imports such as `summaries/`, `logs/`, and `decisions/` keep their richer kind.
    normalize_legacy_import_hashed_kinds(pool).await?;

    Ok(())
}

async fn add_verified_proposal_hat_column_if_missing(pool: &SqlitePool) -> Result<(), DenError> {
    let columns =
        sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info('memory_proposals')")
            .fetch_all(pool)
            .await
            .map_err(|err| DenError::System(format!("inspect memory proposal columns: {err}")))?;
    if !columns.iter().any(|column| column == "target_hat_id") {
        sqlx::query("ALTER TABLE memory_proposals ADD COLUMN target_hat_id TEXT NULL")
            .execute(pool)
            .await
            .map_err(|err| DenError::System(format!("add verified proposal hat column: {err}")))?;
    }
    Ok(())
}

async fn normalize_legacy_import_hashed_kinds(pool: &SqlitePool) -> Result<(), DenError> {
    sqlx::query(
        r"
        UPDATE memory_records
        SET kind = 'note'
        WHERE memory_id LIKE 'legacy-memory-import:%'
          AND kind GLOB 'mem_[0-9a-fA-F]*'
        ",
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("normalize legacy import hashed kinds failed: {e}")))?;
    Ok(())
}

/// Add the bi-temporal event-time columns (ADR-0041 / DERIVED_RECALL Phase 3.5) to pre-existing
/// per-Bear SQLite files. Additive and idempotent; `valid_from` defaults to `created_at` on write,
/// `invalid_at` is forward-looking (set on supersession). Recall reads COALESCE(valid_from,created_at).
async fn add_adr0041_record_columns_if_missing(
    pool: &SqlitePool,
    record_columns: &[String],
) -> Result<(), DenError> {
    if !record_columns.iter().any(|c| c == "valid_from") {
        sqlx::query("ALTER TABLE memory_records ADD COLUMN valid_from TEXT NULL")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("add valid_from column failed: {e}")))?;
    }
    if !record_columns.iter().any(|c| c == "invalid_at") {
        sqlx::query("ALTER TABLE memory_records ADD COLUMN invalid_at TEXT NULL")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("add invalid_at column failed: {e}")))?;
    }
    if !record_columns.iter().any(|c| c == "salience") {
        sqlx::query(
            "ALTER TABLE memory_records ADD COLUMN salience TEXT NOT NULL DEFAULT 'normal'",
        )
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("add salience column failed: {e}")))?;
    }
    Ok(())
}

async fn ensure_memory_harvest_marks(pool: &SqlitePool) -> Result<(), DenError> {
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS memory_harvest_marks (
            mark_id TEXT PRIMARY KEY,
            bear_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            source_kind TEXT NOT NULL,
            source_ref TEXT NOT NULL,
            source_hash TEXT NULL,
            harvested_at TEXT NOT NULL,
            run_id TEXT NULL,
            proposal_ids_json TEXT NOT NULL DEFAULT '[]'
        )
        ",
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("create memory_harvest_marks failed: {e}")))?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_harvest_marks_source ON memory_harvest_marks (bear_id, source_kind, source_ref)",
    )
    .execute(pool)
    .await
    .map_err(|e| DenError::System(format!("index memory_harvest_marks failed: {e}")))?;
    Ok(())
}

/// Drop the legacy `memory_links` base table (record→record provenance, already captured in
/// `memory_promotions`) and (re)create the descriptive ∪ access-bearing read view.
async fn retire_legacy_memory_links_table(pool: &SqlitePool) -> Result<(), DenError> {
    let object_type: Option<String> =
        sqlx::query_scalar("SELECT type FROM sqlite_master WHERE name = 'memory_links'")
            .fetch_optional(pool)
            .await
            .map_err(|e| DenError::System(format!("memory_links object lookup failed: {e}")))?;

    if object_type.as_deref() == Some("table") {
        sqlx::query("DROP TABLE memory_links")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("drop legacy memory_links failed: {e}")))?;
        sqlx::query(MEMORY_LINKS_VIEW_SQL)
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("create memory_links view failed: {e}")))?;
    }
    Ok(())
}

/// `memory_links` read view (must mirror `schema.sql`).
const MEMORY_LINKS_VIEW_SQL: &str = r"
CREATE VIEW IF NOT EXISTS memory_links AS
    SELECT link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
           qualifiers_json, author_profile, author_agent_id, confidence, state,
           supersedes_link_id, created_at, 'descriptive' AS class
    FROM memory_relations
    UNION ALL
    SELECT link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
           qualifiers_json, author_profile, author_agent_id, confidence, state,
           supersedes_link_id, created_at, 'access_bearing' AS class
    FROM memory_access_rules
";

async fn drop_entity_ref_column_if_present(
    pool: &SqlitePool,
    record_columns: &[String],
) -> Result<(), DenError> {
    if record_columns.iter().any(|c| c == "entity_ref") {
        sqlx::query("ALTER TABLE memory_records DROP COLUMN entity_ref")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("drop entity_ref column failed: {e}")))?;
    }
    Ok(())
}

async fn rebuild_memory_records_scope_vocab(pool: &SqlitePool) -> Result<(), DenError> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("bear sqlite migration begin failed: {e}")))?;

    let migration = async {
        sqlx::query(
            r"
            CREATE TABLE memory_records_new (
                memory_id TEXT PRIMARY KEY,
                bear_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                scope_type TEXT NOT NULL CHECK (scope_type IN ('profile_local', 'source_local', 'hat', 'shared')),
                scope_profile TEXT NULL,
                scope_source_kind TEXT NULL,
                scope_source_id TEXT NULL,
                scope_hat_id TEXT NULL,
                kind TEXT NOT NULL,
                author_profile TEXT NOT NULL,
                author_agent_id TEXT NULL,
                created_at TEXT NOT NULL,
                content_text TEXT NOT NULL,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                supersedes_memory_id TEXT NULL,
                visibility TEXT NOT NULL DEFAULT 'normal',
                logical_path TEXT NULL,
                work_surface_ref TEXT NULL,
                valid_from TEXT NULL,
                invalid_at TEXT NULL,
                salience TEXT NOT NULL DEFAULT 'normal',
                CHECK (
                    (scope_type = 'source_local' AND scope_source_kind IN ('conversation', 'work_run', 'intake')
                     AND scope_source_id IS NOT NULL AND scope_hat_id IS NULL AND scope_profile IS NULL)
                    OR (scope_type = 'hat' AND scope_hat_id IS NOT NULL AND scope_source_kind IS NULL
                        AND scope_source_id IS NULL AND scope_profile IS NULL)
                    OR (scope_type IN ('profile_local', 'shared') AND scope_source_kind IS NULL
                        AND scope_source_id IS NULL AND scope_hat_id IS NULL)
                )
            )
            ",
        )
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("create memory_records_new failed: {e}")))?;

        sqlx::query(
            r"
            INSERT INTO memory_records_new (
                memory_id, bear_id, sequence_no, scope_type, scope_profile, kind,
                author_profile, author_agent_id, created_at, content_text, metadata_json,
                supersedes_memory_id, visibility, logical_path, work_surface_ref,
                valid_from, invalid_at, salience
            )
            SELECT
                memory_id,
                bear_id,
                sequence_no,
                CASE scope_type WHEN 'role_local' THEN 'profile_local' ELSE scope_type END,
                scope_profile,
                kind,
                author_profile,
                author_agent_id,
                created_at,
                content_text,
                metadata_json,
                supersedes_memory_id,
                visibility,
                logical_path,
                work_surface_ref,
                valid_from,
                invalid_at,
                COALESCE(salience, 'normal')
            FROM memory_records
            ",
        )
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("copy memory_records scope vocab failed: {e}")))?;

        sqlx::query("DROP TABLE memory_records")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("drop legacy memory_records failed: {e}")))?;

        sqlx::query("ALTER TABLE memory_records_new RENAME TO memory_records")
            .execute(pool)
            .await
            .map_err(|e| DenError::System(format!("rename memory_records_new failed: {e}")))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_records_bear_sequence ON memory_records (bear_id, sequence_no)",
        )
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("recreate memory_records index failed: {e}")))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_records_logical_path ON memory_records (bear_id, logical_path)",
        )
        .execute(pool)
        .await
        .map_err(|e| DenError::System(format!("recreate memory_records logical_path index failed: {e}")))?;

        Ok::<(), DenError>(())
    }
    .await;

    match migration {
        Ok(()) => {
            sqlx::query("COMMIT").execute(pool).await.map_err(|e| {
                DenError::System(format!("bear sqlite migration commit failed: {e}"))
            })?;
        }
        Err(err) => {
            let _ = sqlx::query("ROLLBACK").execute(pool).await;
            return Err(err);
        }
    }

    Ok(())
}
