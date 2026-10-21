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

/// All relations (both classes) anchored on a source record, via the `memory_links` view.
pub async fn list_relations_for_source(
    store: &BearMemoryStore,
    src_memory_id: &str,
    limit: i64,
) -> Result<Vec<RelationRow>, DenError> {
    let rows = sqlx::query_as::<_, RelationSqlRow>(
        r"
        SELECT link_id, sequence_no, src_memory_id, entity_id, relation, class,
               qualifiers_json, author_profile, state, created_at
        FROM memory_links
        WHERE bear_id = ? AND src_memory_id = ? AND state = 'active'
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(src_memory_id)
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list relations for source failed: {e}")))?;
    Ok(rows.into_iter().map(RelationSqlRow::into_row).collect())
}

/// All relations (both classes) pointing at an entity, via the `memory_links` view.
pub async fn list_relations_for_entity(
    store: &BearMemoryStore,
    entity_id: &str,
    limit: i64,
) -> Result<Vec<RelationRow>, DenError> {
    let rows = sqlx::query_as::<_, RelationSqlRow>(
        r"
        SELECT link_id, sequence_no, src_memory_id, entity_id, relation, class,
               qualifiers_json, author_profile, state, created_at
        FROM memory_links
        WHERE bear_id = ? AND entity_id = ? AND state = 'active'
        ORDER BY sequence_no DESC
        LIMIT ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(entity_id)
    .bind(limit)
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
    let rows = sqlx::query_as::<_, (String, String)>(
        r"
        SELECT DISTINCT src_memory_id, entity_id
        FROM memory_relations
        WHERE bear_id = ? AND state = 'active'
        ORDER BY src_memory_id, entity_id
        ",
    )
    .bind(store.bear_id().to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("descriptive entity ids by source failed: {e}")))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (src, entity) in rows {
        map.entry(src).or_default().push(entity);
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
    let placeholders = vec!["?"; memory_ids.len()].join(",");
    let sql = format!(
        "SELECT DISTINCT entity_id FROM memory_relations \
         WHERE bear_id = ? AND state = 'active' AND src_memory_id IN ({placeholders})"
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
    let placeholders = vec!["?"; entity_ids.len()].join(",");
    let sql = format!(
        "SELECT DISTINCT src_memory_id FROM memory_relations \
         WHERE bear_id = ? AND state = 'active' AND entity_id IN ({placeholders})"
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
/// expansion. Access/role gating is applied by the caller when rendering the reached records.
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
        SELECT EXISTS (
            SELECT 1 FROM memory_access_rules
            WHERE bear_id = ? AND src_memory_id = ?
        ) AS "has_rules!: bool"
        "#,
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
mod tests {
    use super::*;
    use crate::resolver::{resolve, Assertion, Resolution, Signal};
    use crate::test_support::new_test_store;
    use serde_json::json;

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
    async fn descriptive_and_access_route_to_distinct_tables() {
        let store = new_test_store().await;
        let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;

        let descriptive = append_relation(
            &store,
            "mem-1",
            &person,
            "subject",
            &json!({ "is_primary": true }),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(descriptive.class, RelationClass::Descriptive);

        let access = append_relation(
            &store,
            "mem-1",
            &person,
            "audience",
            &json!({}),
            "curate",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(access.class, RelationClass::AccessBearing);

        // The gate reader only sees the access-bearing row.
        let gate = list_access_rules_for_source(&store, "mem-1").await.unwrap();
        assert_eq!(gate.len(), 1);
        assert_eq!(gate[0].relation, "den.memory.relation.audience");

        // The union view returns both, correctly tagged.
        let all = list_relations_for_source(&store, "mem-1", 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|r| r.class == RelationClass::Descriptive));
        assert!(all.iter().any(|r| r.class == RelationClass::AccessBearing));
    }

    #[tokio::test]
    async fn access_bearing_rejects_unresolved_target() {
        let store = new_test_store().await;
        // A weak-only person stays provisional and cannot gate visibility.
        let provisional = match resolve(
            &store,
            "person",
            Some("Maybe-Ryan"),
            &[Signal::new("mention", "Ryan")],
            Assertion::Inferred,
        )
        .await
        .unwrap()
        {
            Resolution::Created(e) => e.entity_id,
            other => panic!("expected Created provisional, got {other:?}"),
        };
        let err = append_relation(
            &store,
            "mem-2",
            &provisional,
            "audience",
            &json!({}),
            "curate",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DenError::ValidationError(_)), "got {err:?}");

        // But a descriptive relation against the same provisional entity is fine.
        append_relation(
            &store,
            "mem-2",
            &provisional,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .expect("descriptive relation on provisional entity allowed");
    }

    #[tokio::test]
    async fn disallowed_qualifier_and_unknown_relation_rejected() {
        let store = new_test_store().await;
        let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;

        let bad_qual = append_relation(
            &store,
            "mem-3",
            &person,
            "subject",
            &json!({ "nonsense": 1 }),
            "pair",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(bad_qual, DenError::ValidationError(_)));

        let bad_rel = append_relation(
            &store,
            "mem-3",
            &person,
            "definitely_not_a_relation",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(bad_rel, DenError::ValidationError(_)));
    }

    #[tokio::test]
    async fn descriptive_entity_ids_by_source_excludes_access_bearing() {
        let store = new_test_store().await;
        let person = resolved_person(&store, "Ryan", "ryan@acme.com").await;
        let other = resolved_person(&store, "Dana", "dana@acme.com").await;

        // Descriptive relations participate in recall; access-bearing (gate) must be excluded.
        append_relation(
            &store,
            "mem-1",
            &person,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "mem-1",
            &other,
            "participant",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "mem-1",
            &person,
            "audience",
            &json!({}),
            "curate",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "mem-2",
            &other,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();

        let map = descriptive_entity_ids_by_source(&store).await.unwrap();

        let mut mem1 = map.get("mem-1").cloned().unwrap_or_default();
        mem1.sort();
        let mut expected = vec![person.clone(), other.clone()];
        expected.sort();
        assert_eq!(
            mem1, expected,
            "descriptive entities only, gate row excluded"
        );
        assert_eq!(map.get("mem-2").map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn bounded_graph_expand_reaches_indirect_records_depth_capped() {
        let store = new_test_store().await;
        let e1 = resolved_person(&store, "Alice", "alice@acme.com").await;
        let e2 = resolved_person(&store, "Bob", "bob@acme.com").await;

        // A—e1—B (1 hop apart), B—e2—C (so C is 2 hops from A, never sharing an entity with A).
        append_relation(
            &store,
            "rec-A",
            &e1,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "rec-B",
            &e1,
            "participant",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "rec-B",
            &e2,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        append_relation(
            &store,
            "rec-C",
            &e2,
            "participant",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();

        // Depth 2 from A reaches B (hop 1) and C (hop 2); the seed A is excluded.
        let reached = bounded_graph_expand(&store, &["rec-A".into()], 2, 10)
            .await
            .unwrap();
        let by_id: std::collections::HashMap<&str, u32> = reached
            .iter()
            .map(|r| (r.memory_id.as_str(), r.hop))
            .collect();
        assert_eq!(
            by_id.get("rec-B"),
            Some(&1),
            "B is one hop via the shared entity: {reached:?}"
        );
        assert_eq!(
            by_id.get("rec-C"),
            Some(&2),
            "C is two hops, never directly co-located: {reached:?}"
        );
        assert!(
            !by_id.contains_key("rec-A"),
            "seed is excluded from results"
        );

        // Depth 1 stops at B; C is beyond the cap.
        let shallow = bounded_graph_expand(&store, &["rec-A".into()], 1, 10)
            .await
            .unwrap();
        let ids: Vec<&str> = shallow.iter().map(|r| r.memory_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["rec-B"],
            "depth 1 reaches only the 1-hop neighbor: {shallow:?}"
        );
    }

    #[tokio::test]
    async fn relation_follows_merge_forward_pointer() {
        let store = new_test_store().await;
        let survivor = resolved_person(&store, "Ryan", "ryan@acme.com").await;
        let loser = resolved_person(&store, "Ryan2", "ryan@personal.com").await;
        crate::entity::merge_entities(&store, &survivor, &loser)
            .await
            .unwrap();
        // Writing against the (dead) loser id re-homes to the survivor.
        let rel = append_relation(
            &store,
            "mem-4",
            &loser,
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(rel.entity_id, survivor);
    }
}
