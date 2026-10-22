//! Memory–entity relation layer (ADR-0042 §7): two descriptor-routed physical tables with a
//! union read view. The relation descriptor's `class` selects the table — writers never
//! choose — so a descriptive write can never land in the access surface and vice versa.
//!
//! `memory_access_rules` is the only table the recall gate consults; it is append-only and
//! enforces that access-bearing targets are at least `resolved` (ADR-0042 §11 trust gate).

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use den_core::DenError;

use super::clock::now_rfc3339;
use super::descriptors::{self, RelationClass};
use super::entity;
use super::records::BearMemoryStore;

#[derive(Debug, Clone, Serialize)]
pub struct RelationRow {
    pub link_id: String,
    pub sequence_no: i64,
    pub src_memory_id: String,
    pub entity_id: String,
    pub relation: String,
    pub class: RelationClass,
    pub qualifiers_json: Value,
    pub author_profile: String,
    pub state: String,
    pub created_at: String,
}

/// Append a record→entity relation. The relation id/alias picks the descriptor, which fixes
/// the class and routes the write; qualifiers are validated against the descriptor; access-
/// bearing relations require a resolved target.
#[allow(clippy::too_many_arguments)]
pub async fn append_relation(
    store: &BearMemoryStore,
    src_memory_id: &str,
    entity_id: &str,
    relation_id_or_alias: &str,
    qualifiers: &Value,
    author_profile: &str,
    author_agent_id: Option<&str>,
    confidence: Option<&str>,
) -> Result<RelationRow, DenError> {
    crate::personal_facts::ensure_generic_mutation_allowed(store, src_memory_id).await?;
    let descriptor = descriptors::relation(relation_id_or_alias).ok_or_else(|| {
        DenError::ValidationError(format!("unknown relation: {relation_id_or_alias}"))
    })?;

    // Target entity must exist; follow the merge forward pointer to the live survivor.
    let target = entity::resolve_live_entity(store, entity_id)
        .await?
        .ok_or_else(|| DenError::NotFound(format!("entity {entity_id}")))?;

    // Entity-type applicability.
    if !descriptor.applies_to_entities.contains(&"*")
        && !descriptor
            .applies_to_entities
            .contains(&target.entity_type.as_str())
    {
        return Err(DenError::ValidationError(format!(
            "relation {} does not apply to entity type {}",
            descriptor.alias, target.entity_type
        )));
    }

    // Qualifier whitelist.
    if let Some(obj) = qualifiers.as_object() {
        for key in obj.keys() {
            if !descriptor.allowed_qualifiers.contains(&key.as_str()) {
                return Err(DenError::ValidationError(format!(
                    "relation {} disallows qualifier {key}",
                    descriptor.alias
                )));
            }
        }
    } else if !qualifiers.is_null() {
        return Err(DenError::ValidationError(
            "qualifiers must be a JSON object".to_string(),
        ));
    }

    // Access-bearing trust gate: never gate on a mergeable/provisional identity.
    if descriptor.class == RelationClass::AccessBearing && !target.resolution.can_bear_access() {
        return Err(DenError::ValidationError(format!(
            "access-bearing relation {} requires a resolved target (entity {} is {})",
            descriptor.alias,
            target.entity_id,
            target.resolution.as_str()
        )));
    }

    let table = match descriptor.class {
        RelationClass::Descriptive => "memory_relations",
        RelationClass::AccessBearing => "memory_access_rules",
    };

    let link_id = Uuid::new_v4().to_string();
    let sequence_no = store.next_sequence().await?;
    let created_at = now_rfc3339()?;
    let qualifiers_text = if qualifiers.is_null() {
        "{}".to_string()
    } else {
        qualifiers.to_string()
    };

    // sqlx-dynamic: the descriptor class selects one of two fixed physical tables.
    let sql = format!(
        r"
        INSERT INTO {table} (
            link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
            qualifiers_json, author_profile, author_agent_id, confidence, state,
            supersedes_link_id, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', NULL, ?)
        "
    );
    sqlx::query(&sql)
        .bind(&link_id)
        .bind(store.bear_id().to_string())
        .bind(sequence_no)
        .bind(src_memory_id)
        .bind(&target.entity_id)
        .bind(descriptor.id)
        .bind(&qualifiers_text)
        .bind(author_profile)
        .bind(author_agent_id)
        .bind(confidence)
        .bind(&created_at)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("append relation failed: {e}")))?;

    Ok(RelationRow {
        link_id,
        sequence_no,
        src_memory_id: src_memory_id.to_string(),
        entity_id: target.entity_id,
        relation: descriptor.id.to_string(),
        class: descriptor.class,
        qualifiers_json: qualifiers.clone(),
        author_profile: author_profile.to_string(),
        state: "active".to_string(),
        created_at,
    })
}

/// Active relations on a generic-readable source, via the `memory_links` view.
pub async fn list_relations_for_source(
    store: &BearMemoryStore,
    src_memory_id: &str,
    limit: i64,
) -> Result<Vec<RelationRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query_as!(
        RelationSqlRow,
        r#"
        SELECT link_id AS "link_id!", sequence_no AS "sequence_no!",
               src_memory_id AS "src_memory_id!", entity_id AS "entity_id!",
               relation AS "relation!", class AS "class!: String", qualifiers_json AS "qualifiers_json!",
               author_profile AS "author_profile!", state AS "state!", created_at AS "created_at!"
        FROM memory_links
        WHERE bear_id = ? AND src_memory_id = ? AND state = 'active'
          AND EXISTS (
              SELECT 1 FROM generic_memory_records source
              WHERE source.bear_id = memory_links.bear_id
                AND source.memory_id = memory_links.src_memory_id
          )
        ORDER BY sequence_no DESC
        LIMIT ?
        "#,
        bear_id,
        src_memory_id,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list relations for source failed: {e}")))?;
    Ok(rows.into_iter().map(RelationSqlRow::into_row).collect())
}

/// Active relations from generic-readable sources pointing at an entity.
pub async fn list_relations_for_entity(
    store: &BearMemoryStore,
    entity_id: &str,
    limit: i64,
) -> Result<Vec<RelationRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query_as!(
        RelationSqlRow,
        r#"
        SELECT link_id AS "link_id!", sequence_no AS "sequence_no!",
               src_memory_id AS "src_memory_id!", entity_id AS "entity_id!",
               relation AS "relation!", class AS "class!: String", qualifiers_json AS "qualifiers_json!",
               author_profile AS "author_profile!", state AS "state!", created_at AS "created_at!"
        FROM memory_links
        WHERE bear_id = ? AND entity_id = ? AND state = 'active'
          AND EXISTS (
              SELECT 1 FROM generic_memory_records source
              WHERE source.bear_id = memory_links.bear_id
                AND source.memory_id = memory_links.src_memory_id
          )
        ORDER BY sequence_no DESC
        LIMIT ?
        "#,
        bear_id,
        entity_id,
        limit,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list relations for entity failed: {e}")))?;
    Ok(rows.into_iter().map(RelationSqlRow::into_row).collect())
}

/// Map of `src_memory_id` → its distinct **descriptive** entity ids, for every record in the
/// Bear that carries one. Descriptive relations (`memory_relations`) are the recall-participating
/// links (ADR-0042 §7 `recall_effect = boost`); the access-bearing gate (`memory_access_rules`)
/// is intentionally excluded. The recall indexer uses this to denormalize resolved entities into
/// the derived Qdrant passage payload (a single bulk query per reconcile, not per record).
pub async fn descriptive_entity_ids_by_source(
    store: &BearMemoryStore,
) -> Result<HashMap<String, Vec<String>>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT src_memory_id AS "src_memory_id!", entity_id AS "entity_id!"
        FROM memory_relations
        WHERE bear_id = ? AND state = 'active'
          AND EXISTS (
              SELECT 1 FROM generic_memory_records source
              WHERE source.bear_id = memory_relations.bear_id
                AND source.memory_id = memory_relations.src_memory_id
          )
        ORDER BY src_memory_id, entity_id
        "#,
        bear_id,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("descriptive entity ids by source failed: {e}")))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        map.entry(row.src_memory_id)
            .or_default()
            .push(row.entity_id);
    }
    Ok(map)
}

/// Distinct **descriptive** entity ids linked to any of `memory_ids` (one hop record→entity over
/// `memory_relations`). The access-bearing gate is excluded by construction. Empty in ⇒ empty out.
pub async fn descriptive_entity_ids_for_records(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<Vec<String>, DenError> {
    if memory_ids.is_empty() {
        return Ok(Vec::new());
    }
    // sqlx-dynamic: the number of bound record IDs varies; source eligibility stays in SQL.
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT DISTINCT entity_id FROM memory_relations \
         WHERE bear_id = ? AND state = 'active' AND src_memory_id IN ({placeholders}) \
         AND EXISTS (SELECT 1 FROM generic_memory_records source \
                     WHERE source.bear_id = memory_relations.bear_id \
                       AND source.memory_id = memory_relations.src_memory_id)"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql).bind(store.bear_id().to_string());
    for id in memory_ids {
        query = query.bind(id);
    }
    query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("entity ids for records failed: {e}")))
}

/// Distinct source record ids descriptively linked to any of `entity_ids` (one hop entity→record
/// over `memory_relations`). Empty in ⇒ empty out.
pub async fn record_ids_for_entities(
    store: &BearMemoryStore,
    entity_ids: &[String],
) -> Result<Vec<String>, DenError> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    // sqlx-dynamic: the number of bound entity IDs varies; source eligibility stays in SQL.
    let placeholders = vec!["?"; entity_ids.len()].join(",");
    let sql = format!(
        "SELECT DISTINCT src_memory_id FROM memory_relations \
         WHERE bear_id = ? AND state = 'active' AND entity_id IN ({placeholders}) \
         AND EXISTS (SELECT 1 FROM generic_memory_records source \
                     WHERE source.bear_id = memory_relations.bear_id \
                       AND source.memory_id = memory_relations.src_memory_id)"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql).bind(store.bear_id().to_string());
    for id in entity_ids {
        query = query.bind(id);
    }
    query
        .fetch_all(store.pool())
        .await
        .map_err(|e| DenError::System(format!("record ids for entities failed: {e}")))
}

/// A record reached by bounded-graph expansion, with its hop distance from the seed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphReach {
    pub memory_id: String,
    pub hop: u32,
}

/// Bounded, read-only, retrieval-time expansion over the **bipartite** record↔entity descriptive
/// relation graph (ADR-0042 §4 anti-RDF guardrails; DERIVED_RECALL Phase 3.5). Starting from
/// `seed_memory_ids`, alternately hops record→entity→record up to `max_depth` (default 2),
/// returning the newly reached memory ids (excluding the seeds) with their hop distance, closest
/// hops first. No stored transitive edges, no inference, no entity↔entity edges — pure query
/// expansion. Only generic-readable sources participate; callers still enforce role scope.
pub async fn bounded_graph_expand(
    store: &BearMemoryStore,
    seed_memory_ids: &[String],
    max_depth: u32,
    limit: usize,
) -> Result<Vec<GraphReach>, DenError> {
    use std::collections::HashSet;
    if seed_memory_ids.is_empty() || max_depth == 0 || limit == 0 {
        return Ok(Vec::new());
    }
    let mut seen: HashSet<String> = seed_memory_ids.iter().cloned().collect();
    let mut frontier: Vec<String> = seed_memory_ids.to_vec();
    let mut out: Vec<GraphReach> = Vec::new();
    for hop in 1..=max_depth {
        let entities = descriptive_entity_ids_for_records(store, &frontier).await?;
        if entities.is_empty() {
            break;
        }
        let candidates = record_ids_for_entities(store, &entities).await?;
        let mut next: Vec<String> = Vec::new();
        for memory_id in candidates {
            if seen.insert(memory_id.clone()) {
                out.push(GraphReach {
                    memory_id: memory_id.clone(),
                    hop,
                });
                next.push(memory_id);
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(out)
}

/// The recall gate input (ADR-0042 §7): the access-bearing relations on a record. This is the
/// only reader of `memory_access_rules`; the recall assembler must consult it before returning.
pub async fn list_access_rules_for_source(
    store: &BearMemoryStore,
    src_memory_id: &str,
) -> Result<Vec<RelationRow>, DenError> {
    let rows = sqlx::query_as::<_, AccessRuleSqlRow>(
        r"
        SELECT link_id, sequence_no, src_memory_id, entity_id, relation,
               qualifiers_json, author_profile, state, created_at
        FROM memory_access_rules
        WHERE bear_id = ? AND src_memory_id = ? AND state = 'active'
        ORDER BY sequence_no DESC
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(src_memory_id)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list access rules failed: {e}")))?;
    Ok(rows.into_iter().map(AccessRuleSqlRow::into_row).collect())
}

/// Whether a record has ever carried an access rule, including withdrawn rules.
/// Removing the final active rule must not silently make a protected record public.
pub(crate) async fn has_access_rule_history_for_source(
    store: &BearMemoryStore,
    src_memory_id: &str,
) -> Result<bool, DenError> {
    let bear_id = store.bear_id().to_string();
    sqlx::query_scalar!(
        r#"
        SELECT (EXISTS (
            SELECT 1 FROM memory_access_rules
            WHERE bear_id = ? AND src_memory_id = ?
        ) OR EXISTS (
            SELECT 1 FROM personal_memory_publications WHERE bear_id = ? AND memory_id = ?
        )) AS "has_rules!: bool"
        "#,
        bear_id,
        src_memory_id,
        bear_id,
        src_memory_id,
    )
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("check access rule history failed: {e}")))
}

#[derive(sqlx::FromRow)]
struct RelationSqlRow {
    link_id: String,
    sequence_no: i64,
    src_memory_id: String,
    entity_id: String,
    relation: String,
    class: String,
    qualifiers_json: String,
    author_profile: String,
    state: String,
    created_at: String,
}

impl RelationSqlRow {
    fn into_row(self) -> RelationRow {
        RelationRow {
            link_id: self.link_id,
            sequence_no: self.sequence_no,
            src_memory_id: self.src_memory_id,
            entity_id: self.entity_id,
            relation: self.relation,
            class: if self.class == "access_bearing" {
                RelationClass::AccessBearing
            } else {
                RelationClass::Descriptive
            },
            qualifiers_json: serde_json::from_str(&self.qualifiers_json)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            author_profile: self.author_profile,
            state: self.state,
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AccessRuleSqlRow {
    link_id: String,
    sequence_no: i64,
    src_memory_id: String,
    entity_id: String,
    relation: String,
    qualifiers_json: String,
    author_profile: String,
    state: String,
    created_at: String,
}

impl AccessRuleSqlRow {
    fn into_row(self) -> RelationRow {
        RelationRow {
            link_id: self.link_id,
            sequence_no: self.sequence_no,
            src_memory_id: self.src_memory_id,
            entity_id: self.entity_id,
            relation: self.relation,
            class: RelationClass::AccessBearing,
            qualifiers_json: serde_json::from_str(&self.qualifiers_json)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            author_profile: self.author_profile,
            state: self.state,
            created_at: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests;
