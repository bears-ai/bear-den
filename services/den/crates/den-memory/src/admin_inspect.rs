//! Read-only admin inspection helpers for per-Bear SQLite memory.

use std::path::PathBuf;

use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use den_core::{config::Config, DenError};

use crate::{
    list_memory_proposals, memory_sequence_high_water,
    records::{freshness_trend, lifecycle_status},
    MemoryRecordRow, MemoryScopeType, MemoryStoreManager,
};

#[derive(Debug, Clone, Serialize)]
pub struct BearMemoryAdminStats {
    pub db_path: String,
    pub db_exists: bool,
    pub db_size_bytes: Option<u64>,
    pub record_count: i64,
    pub shared_count: i64,
    pub profile_local_count: i64,
    pub sequence_high_water: i64,
    pub pending_proposals: i64,
    pub pending_observations: i64,
    pub distinct_paths: i64,
}

/// A labeled count bucket for grouped memory stats (by kind, by role, etc.).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryCountBucket {
    pub label: String,
    pub count: i64,
}

/// An extended memory record for the single-entry admin page — includes the fields the
/// lightweight [`MemoryRecordRow`] omits (`author`, `visibility`, `supersedes_memory_id`).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryRecordDetail {
    pub memory_id: String,
    pub sequence_no: i64,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub kind: String,
    pub author_profile: String,
    pub author_agent_id: Option<String>,
    pub visibility: String,
    pub supersedes_memory_id: Option<String>,
    pub logical_path: Option<String>,
    pub work_surface_ref: Option<String>,
    pub content_text: String,
    pub metadata_json: serde_json::Value,
    pub created_at: String,
}

pub fn bear_sqlite_db_path(config: &Config, bear_id: Uuid) -> PathBuf {
    PathBuf::from(&config.bear_sqlite_data_dir).join(format!("{bear_id}.sqlite"))
}

pub async fn bear_memory_admin_stats(
    manager: &MemoryStoreManager,
    config: &Config,
    bear_id: Uuid,
) -> Result<BearMemoryAdminStats, DenError> {
    let db_path = bear_sqlite_db_path(config, bear_id);
    let db_path_display = db_path.display().to_string();
    let db_exists = db_path.exists();
    let db_size_bytes = db_exists
        .then(|| std::fs::metadata(&db_path).ok().map(|m| m.len()))
        .flatten();

    let store = manager.store_for_bear(bear_id).await?;
    let pool = store.pool();

    let record_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM memory_records WHERE bear_id = ?")
            .bind(bear_id.to_string())
            .fetch_one(pool)
            .await
            .map_err(|e| DenError::System(format!("memory record count failed: {e}")))?;

    let shared_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'shared'",
    )
    .bind(bear_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory shared count failed: {e}")))?;

    let profile_local_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'profile_local'",
    )
    .bind(bear_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory profile_local count failed: {e}")))?;

    let distinct_paths: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT logical_path) FROM memory_records WHERE bear_id = ? AND logical_path IS NOT NULL",
    )
    .bind(bear_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory path count failed: {e}")))?;

    let pending_proposals = list_memory_proposals(&store, Some("pending"), 500)
        .await?
        .len() as i64;

    let pending_observations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_observations WHERE bear_id = ? AND status = 'pending_review'",
    )
    .bind(bear_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| DenError::System(format!("memory observation count failed: {e}")))?;

    let sequence_high_water = memory_sequence_high_water(&store).await?;

    Ok(BearMemoryAdminStats {
        db_path: db_path_display,
        db_exists,
        db_size_bytes,
        record_count,
        shared_count,
        profile_local_count,
        sequence_high_water,
        pending_proposals,
        pending_observations,
        distinct_paths,
    })
}

pub async fn list_all_logical_paths(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<String>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let rows = sqlx::query_scalar::<_, String>(
        r"
        SELECT DISTINCT logical_path
        FROM memory_records
        WHERE bear_id = ? AND logical_path IS NOT NULL
        ORDER BY logical_path ASC
        ",
    )
    .bind(bear_id.to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list memory paths failed: {e}")))?;
    Ok(rows)
}

pub async fn get_memory_record_by_id(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    memory_id: &str,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let row = sqlx::query(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND memory_id = ?
        ",
    )
    .bind(bear_id.to_string())
    .bind(memory_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("get memory record failed: {e}")))?;

    let Some(row) = row else {
        return Ok(None);
    };

    let metadata_raw: String = row
        .try_get("metadata_json")
        .map_err(|e| DenError::System(format!("decode metadata_json: {e}")))?;
    let metadata_json =
        serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({}));

    let supersedes_memory_id: Option<String> = row.try_get("supersedes_memory_id").ok();
    let invalid_at: Option<String> = row.try_get("invalid_at").ok();
    let lifecycle_status = lifecycle_status(
        &metadata_json,
        supersedes_memory_id.as_deref(),
        invalid_at.as_deref(),
    );
    let freshness_trend = freshness_trend(&lifecycle_status, invalid_at.as_deref());

    Ok(Some(MemoryRecordRow {
        memory_id: row
            .try_get("memory_id")
            .map_err(|e| DenError::System(e.to_string()))?,
        sequence_no: row
            .try_get("sequence_no")
            .map_err(|e| DenError::System(e.to_string()))?,
        scope_type: MemoryScopeType::parse(
            &row.try_get::<String, _>("scope_type")
                .map_err(|e| DenError::System(e.to_string()))?,
        )
        .unwrap_or(MemoryScopeType::ProfileLocal),
        scope_profile: row.try_get("scope_profile").ok(),
        kind: row
            .try_get("kind")
            .map_err(|e| DenError::System(e.to_string()))?,
        content_text: row
            .try_get("content_text")
            .map_err(|e| DenError::System(e.to_string()))?,
        logical_path: row.try_get("logical_path").ok(),
        work_surface_ref: row.try_get("work_surface_ref").ok(),
        metadata_json,
        created_at: row
            .try_get("created_at")
            .map_err(|e| DenError::System(e.to_string()))?,
        salience: row
            .try_get("salience")
            .unwrap_or_else(|_| "normal".to_string()),
        supersedes_memory_id,
        invalid_at,
        lifecycle_status,
        freshness_trend,
    }))
}

/// Record counts grouped by `kind`, highest first (dashboard "what kinds of memory").
pub async fn count_records_by_kind(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<MemoryCountBucket>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let rows = sqlx::query(
        r"
        SELECT kind AS label, COUNT(*) AS count
        FROM memory_records
        WHERE bear_id = ?
        GROUP BY kind
        ORDER BY count DESC, label ASC
        ",
    )
    .bind(bear_id.to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("count by kind failed: {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(MemoryCountBucket {
                label: row.try_get("label").ok()?,
                count: row.try_get("count").ok()?,
            })
        })
        .collect())
}

/// Record counts grouped by canonical scope, highest first. A missing profile
/// does not imply core: source-local and hat records also have no profile.
pub async fn count_records_by_profile(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<MemoryCountBucket>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let rows = sqlx::query(
        r"
        SELECT CASE scope_type
                   WHEN 'shared' THEN 'core'
                   WHEN 'hat' THEN 'hat'
                   WHEN 'source_local' THEN 'session'
                   ELSE COALESCE(scope_profile, 'legacy profile')
               END AS label, COUNT(*) AS count
        FROM memory_records
        WHERE bear_id = ?
        GROUP BY label
        ORDER BY count DESC, label ASC
        ",
    )
    .bind(bear_id.to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("count by profile failed: {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(MemoryCountBucket {
                label: row.try_get("label").ok()?,
                count: row.try_get("count").ok()?,
            })
        })
        .collect())
}

/// Distinct current entries: logical paths with at least one `normal`-visibility record.
/// (Supersession is latest-at-path today, so this approximates the live head count.)
pub async fn head_entry_count(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<i64, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    sqlx::query_scalar(
        r"
        SELECT COUNT(DISTINCT logical_path)
        FROM memory_records
        WHERE bear_id = ? AND logical_path IS NOT NULL AND visibility = 'normal'
        ",
    )
    .bind(bear_id.to_string())
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("head entry count failed: {e}")))
}

/// One row per logical path: the current head record plus how many versions exist there.
/// Powers the browse/library view without an N+1 per-path query.
#[derive(Debug, Clone, Serialize)]
pub struct PathSummary {
    pub logical_path: String,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub kind: String,
    pub head_memory_id: String,
    pub head_created_at: String,
    pub version_count: i64,
}

pub async fn list_path_summaries(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<PathSummary>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let rows = sqlx::query(
        r"
        SELECT m.logical_path, m.scope_type, m.scope_profile, m.kind,
               m.memory_id AS head_memory_id, m.created_at AS head_created_at,
               agg.version_count
        FROM memory_records m
        JOIN (
            SELECT logical_path, MAX(sequence_no) AS max_seq, COUNT(*) AS version_count
            FROM memory_records
            WHERE bear_id = ? AND logical_path IS NOT NULL
            GROUP BY logical_path
        ) agg ON agg.logical_path = m.logical_path AND agg.max_seq = m.sequence_no
        WHERE m.bear_id = ? AND m.logical_path IS NOT NULL
        ORDER BY m.logical_path ASC
        ",
    )
    .bind(bear_id.to_string())
    .bind(bear_id.to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list path summaries failed: {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(PathSummary {
                logical_path: row.try_get("logical_path").ok()?,
                scope_type: row.try_get("scope_type").ok()?,
                scope_profile: row.try_get("scope_profile").ok(),
                kind: row.try_get("kind").ok()?,
                head_memory_id: row.try_get("head_memory_id").ok()?,
                head_created_at: row.try_get("head_created_at").ok()?,
                version_count: row.try_get("version_count").ok()?,
            })
        })
        .collect())
}

/// Extended single record (all columns) for the entry admin page.
pub async fn get_memory_record_detail(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    memory_id: &str,
) -> Result<Option<MemoryRecordDetail>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let row = sqlx::query(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, author_profile,
               author_agent_id, visibility, supersedes_memory_id, logical_path,
               work_surface_ref, content_text, metadata_json, created_at
        FROM memory_records
        WHERE bear_id = ? AND memory_id = ?
        ",
    )
    .bind(bear_id.to_string())
    .bind(memory_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("get memory record detail failed: {e}")))?;

    let Some(row) = row else {
        return Ok(None);
    };
    let metadata_raw: String = row
        .try_get("metadata_json")
        .map_err(|e| DenError::System(format!("decode metadata_json: {e}")))?;
    let metadata_json =
        serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({}));
    Ok(Some(MemoryRecordDetail {
        memory_id: row
            .try_get("memory_id")
            .map_err(|e| DenError::System(e.to_string()))?,
        sequence_no: row
            .try_get("sequence_no")
            .map_err(|e| DenError::System(e.to_string()))?,
        scope_type: row
            .try_get("scope_type")
            .map_err(|e| DenError::System(e.to_string()))?,
        scope_profile: row.try_get("scope_profile").ok(),
        kind: row
            .try_get("kind")
            .map_err(|e| DenError::System(e.to_string()))?,
        author_profile: row
            .try_get("author_profile")
            .map_err(|e| DenError::System(e.to_string()))?,
        author_agent_id: row.try_get("author_agent_id").ok(),
        visibility: row
            .try_get("visibility")
            .map_err(|e| DenError::System(e.to_string()))?,
        supersedes_memory_id: row.try_get("supersedes_memory_id").ok(),
        logical_path: row.try_get("logical_path").ok(),
        work_surface_ref: row.try_get("work_surface_ref").ok(),
        content_text: row
            .try_get("content_text")
            .map_err(|e| DenError::System(e.to_string()))?,
        metadata_json,
        created_at: row
            .try_get("created_at")
            .map_err(|e| DenError::System(e.to_string()))?,
    }))
}

pub async fn list_recent_memory_records(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let rows = sqlx::query(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ?
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(bear_id.to_string())
    .bind(limit.clamp(1, 50))
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list recent memory records failed: {e}")))?;

    rows.into_iter()
        .map(decode_memory_record_row)
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|e| DenError::System(format!("decode memory records: {e}")))
}

/// Keyword search across **all** of a Bear's memory (every role/scope), newest first.
///
/// Case-insensitive `content_text LIKE` — the always-available fallback when semantic
/// recall (Qdrant) is unconfigured. `query` is matched literally (LIKE metacharacters escaped).
pub async fn search_memory_records(
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    let store = manager.store_for_bear(bear_id).await?;
    let pattern = format!("%{}%", escape_like(query));
    let rows = sqlx::query(
        r"
        SELECT memory_id, sequence_no, scope_type, scope_profile, kind, content_text,
               logical_path, work_surface_ref, metadata_json, created_at, salience,
               supersedes_memory_id, invalid_at
        FROM memory_records
        WHERE bear_id = ? AND content_text LIKE ? ESCAPE '\'
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(bear_id.to_string())
    .bind(pattern)
    .bind(limit.clamp(1, 100))
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("search memory records failed: {e}")))?;

    rows.into_iter()
        .map(decode_memory_record_row)
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|e| DenError::System(format!("decode memory records: {e}")))
}

/// Escape SQLite `LIKE` metacharacters (`\`, `%`, `_`) so a user query matches literally.
pub(crate) fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn decode_memory_record_row(row: sqlx::sqlite::SqliteRow) -> Result<MemoryRecordRow, sqlx::Error> {
    let metadata_raw: String = row.try_get("metadata_json")?;
    let metadata_json =
        serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({}));
    let supersedes_memory_id: Option<String> = row.try_get("supersedes_memory_id").ok();
    let invalid_at: Option<String> = row.try_get("invalid_at").ok();
    let lifecycle_status = lifecycle_status(
        &metadata_json,
        supersedes_memory_id.as_deref(),
        invalid_at.as_deref(),
    );
    let freshness_trend = freshness_trend(&lifecycle_status, invalid_at.as_deref());
    Ok(MemoryRecordRow {
        memory_id: row.try_get("memory_id")?,
        sequence_no: row.try_get("sequence_no")?,
        scope_type: MemoryScopeType::parse(&row.try_get::<String, _>("scope_type")?)
            .unwrap_or(MemoryScopeType::ProfileLocal),
        scope_profile: row.try_get("scope_profile").ok(),
        kind: row.try_get("kind")?,
        content_text: row.try_get("content_text")?,
        logical_path: row.try_get("logical_path").ok(),
        work_surface_ref: row.try_get("work_surface_ref").ok(),
        metadata_json,
        created_at: row.try_get("created_at")?,
        salience: row
            .try_get("salience")
            .unwrap_or_else(|_| "normal".to_string()),
        supersedes_memory_id,
        invalid_at,
        lifecycle_status,
        freshness_trend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        append_memory_record, append_relation, create_entity, get_entity,
        list_record_history_for_logical_path, list_relations_for_source, EntityTrust,
        LogicalMemoryPath, MemorySource, ResolutionState,
    };
    use den_core::ids::HatId;
    use serde_json::json;

    fn temp_config() -> Config {
        let mut config = Config::load();
        let dir = std::env::temp_dir().join(format!("den-admin-inspect-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        config.bear_sqlite_data_dir = dir.to_string_lossy().into_owned();
        config
    }

    #[tokio::test]
    async fn admin_helpers_cover_records_history_search_and_entity_links() {
        let config = temp_config();
        let manager = MemoryStoreManager::new(&config);
        let bear_id = Uuid::new_v4();
        let store = manager.store_for_bear(bear_id).await.expect("store");

        // Two versions at one path (history) + a second path of a different kind.
        let path = LogicalMemoryPath::profile_local("pair", "note");
        let v1 = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "first draft about Ryan",
            &json!({}),
        )
        .await
        .expect("append v1");
        let v2 = append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            "second draft about Ryan",
            &json!({}),
        )
        .await
        .expect("append v2");
        let other = LogicalMemoryPath::shared_core("bear-glossary");
        append_memory_record(
            &store,
            &other,
            "glossary",
            "curate",
            None,
            "glossary entry",
            &json!({}),
        )
        .await
        .expect("append other");

        // Detail fetch includes author/visibility.
        let detail = get_memory_record_detail(&manager, bear_id, &v2.memory_id)
            .await
            .expect("detail query")
            .expect("record exists");
        assert_eq!(detail.author_profile, "pair");
        assert_eq!(detail.visibility, "normal");

        // History: both versions at the path, newest first.
        let versions = list_record_history_for_logical_path(&store, &path.to_logical_path(), 50)
            .await
            .expect("versions");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].memory_id, v2.memory_id, "newest first");
        assert_eq!(versions[1].memory_id, v1.memory_id);

        // Grouped counts + head count.
        let by_kind = count_records_by_kind(&manager, bear_id)
            .await
            .expect("by kind");
        assert!(by_kind.iter().any(|b| b.label == "note" && b.count == 2));
        assert!(by_kind
            .iter()
            .any(|b| b.label == "glossary" && b.count == 1));
        let by_profile = count_records_by_profile(&manager, bear_id)
            .await
            .expect("by profile");
        assert!(by_profile.iter().any(|b| b.label == "pair" && b.count == 2));
        assert!(by_profile.iter().any(|b| b.label == "core" && b.count == 1));
        assert_eq!(head_entry_count(&manager, bear_id).await.expect("heads"), 2);

        append_memory_record(
            &store,
            &LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "hat-note"),
            "note",
            "curate",
            None,
            "hat only",
            &json!({}),
        )
        .await
        .expect("append hat record");
        append_memory_record(
            &store,
            &LogicalMemoryPath::source_local(
                MemorySource::Conversation(Uuid::new_v4()),
                "private-note",
            ),
            "note",
            "pair",
            None,
            "session only",
            &json!({}),
        )
        .await
        .expect("append session record");
        let counts = count_records_by_profile(&manager, bear_id)
            .await
            .expect("scope counts");
        assert!(counts.iter().any(|b| b.label == "core" && b.count == 1));
        assert!(counts.iter().any(|b| b.label == "hat" && b.count == 1));
        assert!(counts.iter().any(|b| b.label == "session" && b.count == 1));

        // Path summaries: one head row per path, with the version count.
        let summaries = list_path_summaries(&manager, bear_id)
            .await
            .expect("summaries");
        let note_summary = summaries
            .iter()
            .find(|s| s.logical_path == path.to_logical_path())
            .expect("note path summary");
        assert_eq!(note_summary.version_count, 2);
        assert_eq!(note_summary.head_memory_id, v2.memory_id);

        // Keyword search across all roles.
        let hits = search_memory_records(&manager, bear_id, "Ryan", 50)
            .await
            .expect("search");
        assert_eq!(hits.len(), 2, "both 'Ryan' drafts match");
        assert!(
            search_memory_records(&manager, bear_id, "nonexistent-token", 50)
                .await
                .expect("search empty")
                .is_empty()
        );

        // Entity cross-link: a record references a resolved entity.
        let entity = create_entity(
            &store,
            "person",
            Some("Ryan"),
            ResolutionState::Resolved,
            EntityTrust::Asserted,
            None,
            &json!({}),
        )
        .await
        .expect("create entity");
        append_relation(
            &store,
            &v2.memory_id,
            &entity.entity_id,
            "subject",
            &json!({ "is_primary": true }),
            "pair",
            None,
            None,
        )
        .await
        .expect("append relation");

        let rels = list_relations_for_source(&store, &v2.memory_id, 10)
            .await
            .expect("relations for source");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].entity_id, entity.entity_id);
        let linked = get_entity(&store, &rels[0].entity_id)
            .await
            .expect("get entity")
            .expect("entity exists");
        assert_eq!(linked.display_name.as_deref(), Some("Ryan"));
    }
}
