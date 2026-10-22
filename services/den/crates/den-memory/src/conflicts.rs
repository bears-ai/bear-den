//! Read-time contradiction detection (ADR-0041 §8): a bounded predicate over records
//! **already retrieved** for a turn — never a corpus scan.
//!
//! Two records conflict iff they share a `logical_path` or a primary `subject` entity
//! (ADR-0042), their validity windows (`COALESCE(valid_from, created_at)` .. `invalid_at`,
//! open-ended when `invalid_at` is `NULL`) overlap, both are live heads, and neither
//! supersedes the other. Supersession is checked transitively: ancestors are gathered by a
//! depth-capped recursive walk seeded **only** by the retrieved ids, so a chain that passes
//! through records outside the retrieved set is still recognized without scanning the corpus.
//!
//! Detected conflicts feed curation as `memory_conflict` observations (idempotent per
//! unordered record pair); resolution stays the standard consolidation path — detection
//! creates work items, it never resolves.

use std::collections::HashMap;

use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::DenError;

use super::descriptors;
use super::observations::{create_memory_observation, SqliteMemoryObservation};
use super::records::{lifecycle_status, parse_rfc3339, BearMemoryStore};

/// Free-form observation kind for read-time memory conflicts (ADR-0041 §8).
pub const MEMORY_CONFLICT_OBSERVATION_KIND: &str = "memory_conflict";

/// Cap on the supersession-chain walk when gathering ancestors (defensive; real chains
/// are short).
const SUPERSESSION_CHAIN_DEPTH: i64 = 32;

/// Why two retrieved records were judged conflicting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictReason {
    /// Divergent live heads on the same `logical_path`.
    SharedLogicalPath(String),
    /// Live records `subject`-linked to the same entity (ADR-0042).
    SharedSubjectEntity(String),
}

impl ConflictReason {
    /// Stable machine tag (`shared_path` / `shared_subject`) for diagnostics and
    /// observation metadata.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SharedLogicalPath(_) => "shared_path",
            Self::SharedSubjectEntity(_) => "shared_subject",
        }
    }

    /// The shared dimension's value: the logical path or the entity id.
    pub fn detail(&self) -> &str {
        match self {
            Self::SharedLogicalPath(path) => path,
            Self::SharedSubjectEntity(entity_id) => entity_id,
        }
    }

    /// Human-readable phrase for observation summaries.
    pub fn describe(&self) -> String {
        match self {
            Self::SharedLogicalPath(path) => format!("shared logical path {path}"),
            Self::SharedSubjectEntity(entity_id) => format!("shared subject entity {entity_id}"),
        }
    }
}

/// A detected conflict between two live records. Ids are stored sorted so the pair is
/// canonical regardless of detection order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflict {
    /// The lexically smaller memory id of the pair.
    pub memory_id_a: String,
    /// The lexically larger memory id of the pair.
    pub memory_id_b: String,
    /// The shared dimension that makes the pair contradictory.
    pub reason: ConflictReason,
}

impl MemoryConflict {
    fn new(x: &str, y: &str, reason: ConflictReason) -> Self {
        let (memory_id_a, memory_id_b) = if x <= y { (x, y) } else { (y, x) };
        Self {
            memory_id_a: memory_id_a.to_string(),
            memory_id_b: memory_id_b.to_string(),
            reason,
        }
    }

    /// Canonical unordered-pair key (sorted ids), the idempotency key for
    /// `memory_conflict` observations.
    pub fn pair_key(&self) -> String {
        format!("{}::{}", self.memory_id_a, self.memory_id_b)
    }

    /// The counterpart of `memory_id` in this pair, if `memory_id` is part of it.
    pub fn other(&self, memory_id: &str) -> Option<&str> {
        if memory_id == self.memory_id_a {
            Some(&self.memory_id_b)
        } else if memory_id == self.memory_id_b {
            Some(&self.memory_id_a)
        } else {
            None
        }
    }
}

/// The per-record inputs the conflict predicate needs, resolved from canonical SQLite.
#[derive(Debug, Clone)]
pub struct ConflictCandidate {
    pub memory_id: String,
    pub logical_path: Option<String>,
    /// Effective event time: `COALESCE(valid_from, created_at)`. `None` when unparseable
    /// (such a record can never satisfy the overlap check).
    pub effective_from: Option<OffsetDateTime>,
    /// Validity-window end; `None` = still valid (open-ended).
    pub invalid_at: Option<OffsetDateTime>,
    pub lifecycle_status: String,
    /// Transitive supersession ancestors (records this one supersedes, directly or via
    /// chain), depth-capped.
    pub supersession_ancestors: Vec<String>,
    /// Entities linked via an active primary `subject` relation (ADR-0042).
    pub subject_entity_ids: Vec<String>,
}

/// A record is a live head when its lifecycle is not `superseded`/`archived` (`active`,
/// `stale`, and `archive-candidate` are still current assertions).
fn is_live_head(candidate: &ConflictCandidate) -> bool {
    !matches!(
        candidate.lifecycle_status.as_str(),
        "superseded" | "archived"
    )
}

/// Half-open validity windows `[effective_from, invalid_at)` overlap; a missing end is
/// open-ended. Records without a parseable effective time never overlap.
fn windows_overlap(a: &ConflictCandidate, b: &ConflictCandidate) -> bool {
    let (Some(a_from), Some(b_from)) = (a.effective_from, b.effective_from) else {
        return false;
    };
    a.invalid_at.is_none_or(|a_end| b_from < a_end)
        && b.invalid_at.is_none_or(|b_end| a_from < b_end)
}

/// The shared dimension of a pair, if any: same non-empty `logical_path` first, else the
/// first shared primary `subject` entity.
fn shared_dimension(a: &ConflictCandidate, b: &ConflictCandidate) -> Option<ConflictReason> {
    if let (Some(pa), Some(pb)) = (a.logical_path.as_deref(), b.logical_path.as_deref()) {
        if !pa.is_empty() && pa == pb {
            return Some(ConflictReason::SharedLogicalPath(pa.to_string()));
        }
    }
    a.subject_entity_ids
        .iter()
        .find(|entity_id| b.subject_entity_ids.contains(entity_id))
        .map(|entity_id| ConflictReason::SharedSubjectEntity(entity_id.clone()))
}

fn in_supersession_chain(a: &ConflictCandidate, b: &ConflictCandidate) -> bool {
    a.supersession_ancestors.contains(&b.memory_id)
        || b.supersession_ancestors.contains(&a.memory_id)
}

/// The pure conflict predicate (ADR-0041 §8) over an already-retrieved candidate set:
/// every unordered pair that shares a `logical_path` or primary `subject` entity, has
/// overlapping validity windows, is live on both sides, and is not related by
/// supersession. O(n²) over the (small, turn-bounded) retrieved set.
pub fn detect_conflicts(candidates: &[ConflictCandidate]) -> Vec<MemoryConflict> {
    let mut conflicts = Vec::new();
    for (index, a) in candidates.iter().enumerate() {
        if !is_live_head(a) {
            continue;
        }
        for b in &candidates[index + 1..] {
            if b.memory_id == a.memory_id || !is_live_head(b) {
                continue;
            }
            let Some(reason) = shared_dimension(a, b) else {
                continue;
            };
            if !windows_overlap(a, b) || in_supersession_chain(a, b) {
                continue;
            }
            conflicts.push(MemoryConflict::new(&a.memory_id, &b.memory_id, reason));
        }
    }
    conflicts
}

/// Resolve [`ConflictCandidate`]s for `memory_ids` from canonical SQLite: record fields,
/// depth-capped supersession ancestors, and active primary `subject` relations. Three
/// bounded queries, each seeded only by the retrieved ids.
pub async fn conflict_candidates(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<Vec<ConflictCandidate>, DenError> {
    if memory_ids.is_empty() {
        return Ok(Vec::new());
    }
    let bear_id = store.bear_id().to_string();
    let placeholders = vec!["?"; memory_ids.len()].join(",");

    // sqlx-dynamic: a bounded caller-supplied ID list determines only the bound placeholders.
    let sql = format!(
        "SELECT memory_id, logical_path, COALESCE(valid_from, created_at), invalid_at, \
                metadata_json, supersedes_memory_id \
         FROM generic_memory_records WHERE bear_id = ? AND memory_id IN ({placeholders})"
    );
    let mut query = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            Option<String>,
            String,
            Option<String>,
        ),
    >(&sql)
    .bind(&bear_id);
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("conflict candidate lookup failed: {e}")))?;

    let ancestors = supersession_ancestors(store, memory_ids).await?;
    let subjects = primary_subject_entities(store, memory_ids).await?;

    Ok(rows
        .into_iter()
        .map(
            |(memory_id, logical_path, effective_from, invalid_at, metadata_json, supersedes)| {
                let metadata: Value = serde_json::from_str(&metadata_json)
                    .unwrap_or_else(|_| Value::Object(Default::default()));
                let lifecycle =
                    lifecycle_status(&metadata, supersedes.as_deref(), invalid_at.as_deref());
                ConflictCandidate {
                    logical_path,
                    effective_from: parse_rfc3339(&effective_from),
                    invalid_at: invalid_at.as_deref().and_then(parse_rfc3339),
                    lifecycle_status: lifecycle,
                    supersession_ancestors: ancestors.get(&memory_id).cloned().unwrap_or_default(),
                    subject_entity_ids: subjects.get(&memory_id).cloned().unwrap_or_default(),
                    memory_id,
                }
            },
        )
        .collect())
}

/// Detect conflicts among `memory_ids` (bounded: candidate resolution + the pure
/// predicate). The intended read-path entry point.
pub async fn memory_conflicts_among(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<Vec<MemoryConflict>, DenError> {
    if memory_ids.len() < 2 {
        return Ok(Vec::new());
    }
    Ok(detect_conflicts(
        &conflict_candidates(store, memory_ids).await?,
    ))
}

/// Transitive supersession ancestors for each seed id, via a depth-capped recursive walk
/// over `supersedes_memory_id`. Seeded only by `memory_ids`; chains may pass through
/// records outside the retrieved set.
async fn supersession_ancestors(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<HashMap<String, Vec<String>>, DenError> {
    let bear_id = store.bear_id().to_string();
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    // sqlx-dynamic: seed IDs are parameterized through a bounded variable-length IN list.
    let sql = format!(
        "WITH RECURSIVE chain(start_id, ancestor_id, depth) AS ( \
             SELECT memory_id, supersedes_memory_id, 1 \
               FROM generic_memory_records \
              WHERE bear_id = ? AND supersedes_memory_id IS NOT NULL \
                AND memory_id IN ({placeholders}) \
             UNION ALL \
             SELECT chain.start_id, r.supersedes_memory_id, chain.depth + 1 \
               FROM chain \
               JOIN generic_memory_records r ON r.bear_id = ? AND r.memory_id = chain.ancestor_id \
              WHERE r.supersedes_memory_id IS NOT NULL AND chain.depth < ? \
         ) \
         SELECT DISTINCT start_id, ancestor_id FROM chain"
    );
    let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(&bear_id);
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .bind(&bear_id)
        .bind(SUPERSESSION_CHAIN_DEPTH)
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("supersession chain lookup failed: {e}")))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (start_id, ancestor_id) in rows {
        map.entry(start_id).or_default().push(ancestor_id);
    }
    Ok(map)
}

/// Active primary `subject` entity ids per record (ADR-0042). A `subject` relation counts
/// as primary unless its `is_primary` qualifier is explicitly `false` (the qualifier is
/// optional and a sole subject is primary by default).
async fn primary_subject_entities(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<HashMap<String, Vec<String>>, DenError> {
    let subject_relation =
        descriptors::relation("subject").map_or("den.memory.relation.subject", |d| d.id);
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT src_memory_id, entity_id, qualifiers_json \
         FROM memory_relations \
         WHERE bear_id = ? AND state = 'active' AND relation = ? \
           AND src_memory_id IN ({placeholders})"
    );
    let mut query = sqlx::query_as::<_, (String, String, String)>(&sql)
        .bind(store.bear_id().to_string())
        .bind(subject_relation);
    for id in memory_ids {
        query = query.bind(id);
    }
    let rows = query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("subject relation lookup failed: {e}")))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (src_memory_id, entity_id, qualifiers_json) in rows {
        let is_primary = serde_json::from_str::<Value>(&qualifiers_json)
            .ok()
            .and_then(|q| q.get("is_primary").and_then(Value::as_bool))
            .unwrap_or(true);
        if !is_primary {
            continue;
        }
        let entities = map.entry(src_memory_id).or_default();
        if !entities.contains(&entity_id) {
            entities.push(entity_id);
        }
    }
    Ok(map)
}

/// Emit a `memory_conflict` observation for a detected pair, **idempotent per unordered
/// pair**: the canonical [`MemoryConflict::pair_key`] is stored in the observation's
/// source metadata and checked (among non-resolved observations) before insert. Returns
/// `None` when the pair is already queued. Callers on the read path must treat errors as
/// best-effort (log and continue) — a failed write never fails recall.
pub async fn record_conflict_observation(
    store: &BearMemoryStore,
    conflict: &MemoryConflict,
) -> Result<Option<SqliteMemoryObservation>, DenError> {
    let pair_key = conflict.pair_key();
    let existing: i64 = sqlx::query_scalar(
        r"
        SELECT COUNT(*) FROM memory_observations
        WHERE bear_id = ? AND reviewed_at IS NULL
          AND status IN ('pending_review', 'review_queued')
          AND json_extract(source_json, '$.kind') = ?
          AND json_extract(source_json, '$.pair_key') = ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(MEMORY_CONFLICT_OBSERVATION_KIND)
    .bind(&pair_key)
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("memory_conflict idempotency check failed: {e}")))?;
    if existing > 0 {
        return Ok(None);
    }

    let observation_id = Uuid::new_v4().to_string();
    let summary = format!(
        "Conflicting live memory records {} and {} ({})",
        conflict.memory_id_a,
        conflict.memory_id_b,
        conflict.reason.describe()
    );
    let source = json!({
        "kind": MEMORY_CONFLICT_OBSERVATION_KIND,
        "pair_key": pair_key,
        "memory_ids": [conflict.memory_id_a, conflict.memory_id_b],
        "reason": conflict.reason.kind(),
        "reason_detail": conflict.reason.detail(),
    });
    create_memory_observation(
        store,
        &observation_id,
        &summary,
        "normal",
        &format!("watch/observations/{observation_id}.md"),
        &source,
    )
    .await
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical_path::LogicalMemoryPath;
    use crate::records::append_memory_record;
    use crate::relations::append_relation;
    use crate::resolver::{resolve, Assertion, Resolution, Signal};
    use crate::test_support::new_test_store;

    fn candidate(memory_id: &str, path: Option<&str>) -> ConflictCandidate {
        ConflictCandidate {
            memory_id: memory_id.to_string(),
            logical_path: path.map(str::to_string),
            effective_from: Some(OffsetDateTime::from_unix_timestamp(1_000).unwrap()),
            invalid_at: None,
            lifecycle_status: "active".to_string(),
            supersession_ancestors: Vec::new(),
            subject_entity_ids: Vec::new(),
        }
    }

    #[test]
    fn disjoint_validity_windows_do_not_conflict() {
        // a was valid [1000, 2000); b became valid at 3000 — a clean handoff, no overlap.
        let mut a = candidate("m-a", Some("core/x.md"));
        a.invalid_at = Some(OffsetDateTime::from_unix_timestamp(2_000).unwrap());
        let mut b = candidate("m-b", Some("core/x.md"));
        b.effective_from = Some(OffsetDateTime::from_unix_timestamp(3_000).unwrap());
        assert!(detect_conflicts(&[a.clone(), b.clone()]).is_empty());

        // Overlapping windows on the same path do conflict.
        b.effective_from = Some(OffsetDateTime::from_unix_timestamp(1_500).unwrap());
        let conflicts = detect_conflicts(&[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].reason,
            ConflictReason::SharedLogicalPath("core/x.md".to_string())
        );
    }

    #[test]
    fn supersession_chain_between_live_candidates_suppresses_conflict() {
        // c transitively supersedes a (via a record outside the retrieved set); even if both
        // look live, the chain wins.
        let a = candidate("m-a", Some("core/x.md"));
        let mut c = candidate("m-c", Some("core/x.md"));
        c.supersession_ancestors = vec!["m-b".to_string(), "m-a".to_string()];
        assert!(detect_conflicts(&[a, c]).is_empty());
    }

    #[test]
    fn conflict_pair_is_canonical_and_keyed() {
        let conflict = MemoryConflict::new(
            "zeta",
            "alpha",
            ConflictReason::SharedLogicalPath("p".to_string()),
        );
        assert_eq!(conflict.memory_id_a, "alpha");
        assert_eq!(conflict.memory_id_b, "zeta");
        assert_eq!(conflict.pair_key(), "alpha::zeta");
        assert_eq!(conflict.other("alpha"), Some("zeta"));
        assert_eq!(conflict.other("zeta"), Some("alpha"));
        assert_eq!(conflict.other("nope"), None);
    }

    async fn shared_note(store: &BearMemoryStore, name: &str, content: &str) -> String {
        let logical = LogicalMemoryPath::shared_core(name);
        append_memory_record(store, &logical, "note", "curate", None, content, &json!({}))
            .await
            .expect("append record")
            .memory_id
    }

    #[tokio::test]
    async fn divergent_heads_on_one_path_conflict() {
        let store = new_test_store().await;
        let first = shared_note(&store, "workflow", "deploy on Fridays").await;
        let second = shared_note(&store, "workflow", "never deploy on Fridays").await;
        let other = shared_note(&store, "elsewhere", "unrelated").await;

        let conflicts = memory_conflicts_among(&store, &[first.clone(), second.clone(), other])
            .await
            .expect("detect");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(
            conflicts[0],
            MemoryConflict::new(
                &first,
                &second,
                ConflictReason::SharedLogicalPath("core/workflow.md".to_string())
            )
        );
    }

    #[tokio::test]
    async fn superseded_chain_does_not_conflict() {
        let store = new_test_store().await;
        let logical = LogicalMemoryPath::shared_core("workflow");
        let first =
            append_memory_record(&store, &logical, "note", "curate", None, "v1", &json!({}))
                .await
                .expect("append v1");
        let second = store
            .append_record_with_options(
                &logical,
                "note",
                "curate",
                None,
                "v2",
                &json!({}),
                "normal",
                "normal",
                Some(&first.memory_id),
            )
            .await
            .expect("append v2");
        let third = store
            .append_record_with_options(
                &logical,
                "note",
                "curate",
                None,
                "v3",
                &json!({}),
                "normal",
                "normal",
                Some(&second.memory_id),
            )
            .await
            .expect("append v3");

        // Neither the direct predecessor nor the transitive one conflicts with the head.
        let ids = vec![
            first.memory_id.clone(),
            second.memory_id.clone(),
            third.memory_id.clone(),
        ];
        let conflicts = memory_conflicts_among(&store, &ids).await.expect("detect");
        assert!(conflicts.is_empty(), "{conflicts:?}");

        // The chain is also recognized when the middle record was not retrieved.
        let sparse = vec![first.memory_id.clone(), third.memory_id.clone()];
        let conflicts = memory_conflicts_among(&store, &sparse)
            .await
            .expect("detect sparse");
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    async fn resolved_person(store: &BearMemoryStore, name: &str, email: &str) -> String {
        match resolve(
            store,
            "person",
            Some(name),
            &[Signal::new("email", email)],
            Assertion::Inferred,
        )
        .await
        .unwrap()
        {
            Resolution::Resolved(e) => e.entity_id,
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shared_subject_entity_conflicts_and_non_primary_is_excluded() {
        let store = new_test_store().await;
        let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;
        let first = shared_note(&store, "ryan-role", "Ryan leads platform").await;
        let second = shared_note(&store, "ryan-move", "Ryan left the company").await;
        let third = shared_note(&store, "ryan-mention", "mentions Ryan in passing").await;

        for id in [&first, &second] {
            append_relation(
                &store,
                id,
                &person,
                "subject",
                &json!({}),
                "pair",
                None,
                None,
            )
            .await
            .expect("subject relation");
        }
        // Explicitly non-primary subject links never participate.
        append_relation(
            &store,
            &third,
            &person,
            "subject",
            &json!({ "is_primary": false }),
            "pair",
            None,
            None,
        )
        .await
        .expect("non-primary subject relation");

        let ids = vec![first.clone(), second.clone(), third];
        let conflicts = memory_conflicts_among(&store, &ids).await.expect("detect");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(
            conflicts[0],
            MemoryConflict::new(&first, &second, ConflictReason::SharedSubjectEntity(person))
        );
    }

    #[tokio::test]
    async fn conflict_observation_is_idempotent_per_pair() {
        let store = new_test_store().await;
        let conflict = MemoryConflict::new(
            "mem-b",
            "mem-a",
            ConflictReason::SharedLogicalPath("core/x.md".to_string()),
        );

        let first = record_conflict_observation(&store, &conflict)
            .await
            .expect("first write");
        let observation = first.expect("observation created");
        assert_eq!(
            observation.source_json["kind"],
            MEMORY_CONFLICT_OBSERVATION_KIND
        );
        assert_eq!(observation.source_json["pair_key"], "mem-a::mem-b");
        assert_eq!(observation.source_json["reason"], "shared_path");
        assert!(observation.summary.contains("mem-a"));
        assert!(observation.summary.contains("mem-b"));

        // Repeat detection of the same unordered pair inserts nothing.
        let repeat = record_conflict_observation(&store, &conflict)
            .await
            .expect("repeat write");
        assert!(repeat.is_none());

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM memory_observations WHERE bear_id = ?")
                .bind(store.bear_id().to_string())
                .fetch_one(store.pool())
                .await
                .expect("count observations");
        assert_eq!(count, 1);

        // A different pair still records.
        let other = MemoryConflict::new(
            "mem-c",
            "mem-a",
            ConflictReason::SharedSubjectEntity("entity-1".to_string()),
        );
        assert!(record_conflict_observation(&store, &other)
            .await
            .expect("other pair")
            .is_some());
    }
}
