use den_core::DenError;
use serde::Deserialize;

use super::{
    head_record_for_logical_path, head_record_for_logical_path_with_access, BearMemoryStore,
    MemoryRecordRow,
};
use crate::{
    access::{record_visible, AccessContext},
    logical_path::entity_anchor_path,
};

/// Explicit anchors whose subject-linked salience evidence and bodies are generic-visible.
pub async fn list_entity_anchor_head_records(
    store: &BearMemoryStore,
    limit: i64,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    entity_anchor_heads(store, limit, None).await
}

/// Authorized projection of explicit anchors. Both salience evidence and the selected
/// canonical head must pass the supplied context; descriptive subject links confer no access.
pub async fn list_entity_anchor_head_records_with_access(
    store: &BearMemoryStore,
    limit: i64,
    access: &AccessContext,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    if access.is_empty() {
        return list_entity_anchor_head_records(store, limit).await;
    }
    entity_anchor_heads(store, limit, Some(access)).await
}

struct EntityAnchorCandidate {
    entity_id: String,
    entity_type: String,
    display_name: Option<String>,
    resolution: String,
    canonical_ref: Option<String>,
    high_count: i64,
    normal_count: i64,
    evidence_json: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EvidenceSalience {
    Low,
    Normal,
    High,
    Critical,
}

async fn entity_anchor_heads(
    store: &BearMemoryStore,
    limit: i64,
    access: Option<&AccessContext>,
) -> Result<Vec<MemoryRecordRow>, DenError> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let bear_id = store.bear_id().to_string();
    let candidate_limit = limit.saturating_mul(4).clamp(1, 200);
    let candidates = match access {
        None => sqlx::query_as!(
            EntityAnchorCandidate,
            r#"
            SELECT e.entity_id AS "entity_id!", e.type AS "entity_type!", e.display_name,
                   e.resolution AS "resolution!", e.canonical_ref,
                   SUM(CASE WHEN m.salience IN ('high', 'critical') THEN 1 ELSE 0 END) AS "high_count!: i64",
                   SUM(CASE WHEN m.salience = 'normal' THEN 1 ELSE 0 END) AS "normal_count!: i64",
                   NULL AS "evidence_json?: String"
            FROM entities e
            JOIN memory_relations r ON r.bear_id = e.bear_id AND r.entity_id = e.entity_id
              AND r.state = 'active' AND r.relation = 'den.memory.relation.subject'
            JOIN generic_memory_records m ON m.bear_id = e.bear_id
              AND m.memory_id = r.src_memory_id AND m.visibility = 'normal'
            WHERE e.bear_id = ? AND e.resolution IN ('resolved', 'confirmed')
              AND e.superseded_by_entity_id IS NULL
            GROUP BY e.entity_id, e.type, e.display_name, e.resolution, e.canonical_ref
            ORDER BY MAX(m.sequence_no) DESC LIMIT ?
            "#,
            bear_id,
            candidate_limit,
        )
        .fetch_all(store.pool())
        .await?,
        Some(_) => sqlx::query_as!(
            EntityAnchorCandidate,
            r#"
            SELECT e.entity_id AS "entity_id!", e.type AS "entity_type!", e.display_name,
                   e.resolution AS "resolution!", e.canonical_ref,
                   0 AS "high_count!: i64", 0 AS "normal_count!: i64",
                   json_group_array(json_array(m.memory_id, m.salience)) AS "evidence_json?: String"
            FROM entities e
            JOIN memory_relations r ON r.bear_id = e.bear_id AND r.entity_id = e.entity_id
              AND r.state = 'active' AND r.relation = 'den.memory.relation.subject'
            JOIN memory_records m ON m.bear_id = e.bear_id
              AND m.memory_id = r.src_memory_id AND m.visibility = 'normal'
            WHERE e.bear_id = ? AND e.resolution IN ('resolved', 'confirmed')
              AND e.superseded_by_entity_id IS NULL
            GROUP BY e.entity_id, e.type, e.display_name, e.resolution, e.canonical_ref
            ORDER BY MAX(m.sequence_no) DESC LIMIT ?
            "#,
            bear_id,
            candidate_limit,
        )
        .fetch_all(store.pool())
        .await?,
    };

    let mut out = Vec::new();
    for candidate in candidates {
        let mut high_count = candidate.high_count;
        let mut normal_count = candidate.normal_count;
        if let (Some(access), Some(evidence)) = (access, candidate.evidence_json.as_deref()) {
            let evidence: Vec<(String, EvidenceSalience)> = serde_json::from_str(evidence)?;
            for (memory_id, salience) in evidence {
                if !record_visible(store, &memory_id, access).await? {
                    continue;
                }
                match salience {
                    EvidenceSalience::High | EvidenceSalience::Critical => high_count += 1,
                    EvidenceSalience::Normal => normal_count += 1,
                    EvidenceSalience::Low => {}
                }
            }
        }
        if high_count == 0 && (candidate.resolution != "confirmed" || normal_count < 2) {
            continue;
        }
        let anchor_ref = candidate
            .display_name
            .as_deref()
            .or(candidate.canonical_ref.as_deref())
            .unwrap_or(&candidate.entity_id);
        for kind in ["profile", "overview", "index"] {
            let Some(path) = entity_anchor_path(&candidate.entity_type, anchor_ref, kind) else {
                continue;
            };
            let record = match access {
                Some(access) => {
                    head_record_for_logical_path_with_access(store, &path, access).await?
                }
                None => head_record_for_logical_path(store, &path).await?,
            };
            if let Some(record) = record {
                out.push(record);
                break;
            }
        }
        if out.len() >= limit as usize {
            break;
        }
    }
    Ok(out)
}
