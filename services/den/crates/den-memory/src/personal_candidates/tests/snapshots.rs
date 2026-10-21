use super::*;

/// Whole-table snapshots detect collateral writes, including other people/Bears and audit
/// fields that a count/state-only assertion would miss. Ordering is explicit in every query.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    candidates: String,
    settings: String,
    bindings: String,
    generations: String,
    entities: String,
    handles: String,
    receipts: String,
    sequence: i64,
}

pub(super) async fn snapshot(store: &BearMemoryStore) -> Snapshot {
    let candidates = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(candidate_id, bear_id, person_entity_id, sequence_no,
            fact_type, value_text, evidence_kind, retention, source_conversation_id, source_memory_id,
            source_bear_id, source_user_id, source_binding_generation, state, created_at,
            withdrawn_at, withdrawn_on_bear_id, withdrawn_by_user_id)) AS "rows!: String"
           FROM (SELECT * FROM personal_memory_candidates ORDER BY candidate_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let settings = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(bear_id, person_entity_id, capture_enabled,
            authority_generation, updated_at, updated_on_bear_id, updated_by_user_id)) AS "rows!: String"
           FROM (SELECT * FROM personal_memory_settings ORDER BY bear_id, person_entity_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let bindings = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(bear_id, user_id, entity_id, state)) AS "rows!: String"
           FROM (SELECT * FROM authenticated_user_entities ORDER BY bear_id, entity_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let generations = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(bear_id, entity_id, generation)) AS "rows!: String"
           FROM (SELECT * FROM user_identity_generations ORDER BY bear_id, entity_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let entities = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(entity_id, bear_id, sequence_no, type, display_name,
            resolution, trust, canonical_ref, superseded_by_entity_id, metadata_json, created_at)) AS "rows!: String"
           FROM (SELECT * FROM entities ORDER BY entity_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let handles = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(handle_id, bear_id, entity_id, handle_type,
            handle_value, source, trust, state, created_at)) AS "rows!: String"
           FROM (SELECT * FROM entity_handles ORDER BY handle_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let receipts = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(receipt_id, bear_id, recorded_bear_id, actor_user_id,
            entity_id, destination_user_id, previous_user_id, displaced_entity_id, prior_generation,
            resulting_generation, recorded_at)) AS "rows!: String"
           FROM (SELECT * FROM user_identity_receipts ORDER BY receipt_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    Snapshot {
        candidates,
        settings,
        bindings,
        generations,
        entities,
        handles,
        receipts,
        sequence: next_sequence(store).await,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct GenericSnapshot {
    records: String,
    proposals: String,
    observations: String,
    relations: String,
    rules: String,
    promotions: String,
    harvest: String,
    reflection: String,
}

pub(super) async fn generic_snapshot(store: &BearMemoryStore) -> GenericSnapshot {
    let records = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(memory_id, bear_id, sequence_no, scope_type, scope_profile,
            scope_source_kind, scope_source_id, scope_hat_id, kind, author_profile, author_agent_id,
            created_at, content_text, metadata_json, supersedes_memory_id, visibility,
            logical_path, work_surface_ref, valid_from, invalid_at, salience)) AS "rows!: String"
           FROM (SELECT * FROM memory_records ORDER BY memory_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let proposals = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(proposal_id, bear_id, sequence_no, source_memory_id,
            target_hat_id, suggested_action, sensitivity, requires_human, status, payload_json,
            created_at, reviewed_at)) AS "rows!: String"
           FROM (SELECT * FROM memory_proposals ORDER BY proposal_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let observations = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(observation_id, bear_id, sequence_no, summary,
            salience, payload_ref, source_json, logical_path, status, proposal_id, created_at,
            reviewed_at)) AS "rows!: String"
           FROM (SELECT * FROM memory_observations ORDER BY observation_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let relations = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, qualifiers_json, author_profile, author_agent_id, confidence,
            state, supersedes_link_id, created_at)) AS "rows!: String"
           FROM (SELECT * FROM memory_relations ORDER BY link_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let rules = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, qualifiers_json, author_profile, author_agent_id, confidence,
            state, supersedes_link_id, created_at)) AS "rows!: String"
           FROM (SELECT * FROM memory_access_rules ORDER BY link_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let promotions = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(promotion_id, bear_id, sequence_no, source_memory_id,
            target_memory_id, review_agent_id, action, created_at, notes)) AS "rows!: String"
           FROM (SELECT * FROM memory_promotions ORDER BY promotion_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let harvest = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(mark_id, bear_id, sequence_no, source_kind, source_ref,
            source_hash, harvested_at, run_id, proposal_ids_json)) AS "rows!: String"
           FROM (SELECT * FROM memory_harvest_marks ORDER BY mark_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let reflection = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(run_id, bear_id, sequence_no, lane, trigger, status,
            input_summary, output_summary, proposal_ids_json, created_at, completed_at)) AS "rows!: String"
           FROM (SELECT * FROM reflection_run_outcomes ORDER BY run_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    GenericSnapshot {
        records,
        proposals,
        observations,
        relations,
        rules,
        promotions,
        harvest,
        reflection,
    }
}
