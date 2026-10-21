use super::*;

pub(super) const ACTOR: UserId = UserId::new(900);
pub(super) const ALICE: UserId = UserId::new(11);
pub(super) const BOB: UserId = UserId::new(22);
pub(super) const DESTINATION: UserId = UserId::new(101);

pub(super) async fn person(store: &BearMemoryStore, user: UserId) -> String {
    resolve_authenticated_user_entity(store, user)
        .await
        .unwrap()
        .entity_id()
        .to_owned()
}

pub(super) async fn parked_person(store: &BearMemoryStore, user: UserId) -> String {
    let entity_id = person(store, user).await;
    let review = review_revocation(store, ACTOR, &entity_id).await.unwrap();
    commit_review(store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    entity_id
}

pub(super) async fn reconnect(
    store: &BearMemoryStore,
    entity_id: &str,
    user: UserId,
) -> IdentityReconnectionReceipt {
    let review = review_reconnection(store, ACTOR, entity_id, user)
        .await
        .unwrap();
    commit_review(store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap()
}

pub(super) async fn context(store: &BearMemoryStore, user: UserId) -> AccessContext {
    AccessContext::for_authenticated_user(
        &resolve_authenticated_user_entity(store, user)
            .await
            .unwrap(),
    )
}

pub(super) async fn assert_visible(
    store: &BearMemoryStore,
    memory_id: &str,
    access: &AccessContext,
    expected: bool,
) {
    assert_eq!(
        record_visible(store, memory_id, access).await.unwrap(),
        expected,
        "unexpected visibility for {memory_id} with {access:?}"
    );
}

pub(super) fn assert_validation<T: std::fmt::Debug>(result: Result<T, DenError>) {
    assert!(
        matches!(result, Err(DenError::ValidationError(_))),
        "{result:?}"
    );
}

pub(super) struct IdentityContext {
    pub active_memory_id: String,
    pub active_path: String,
    pub memory_ids: Vec<String>,
}

/// Seed real audience/descriptive links, lifecycle history, and pending review provenance.
/// None of these references may be re-homed by attachment changes.
pub(super) async fn seed_context(
    store: &BearMemoryStore,
    entity_id: &str,
    label: &str,
) -> IdentityContext {
    // Model imported portable metadata and a historical alias without using the guarded
    // mutation API to grant authority. Neither field is authentication evidence.
    let bear_id = store.bear_id().to_string();
    let metadata =
        json!({"source": "original-host", "label": label, "review": {"entity_id": entity_id}})
            .to_string();
    sqlx::query!(
        "UPDATE entities SET display_name = ?, metadata_json = ? WHERE bear_id = ? AND entity_id = ?",
        label, metadata, bear_id, entity_id,
    ).execute(store.pool()).await.unwrap();
    let alias_id = Uuid::new_v4().to_string();
    sqlx::query!(
        "INSERT INTO entity_handles (handle_id, bear_id, entity_id, handle_type, handle_value, source, trust, state, created_at) VALUES (?, ?, ?, 'alias', ?, 'original provenance', 'asserted', 'active', '2026-01-01T00:00:00Z')",
        alias_id, bear_id, entity_id, label,
    ).execute(store.pool()).await.unwrap();
    let logical = LogicalMemoryPath::shared_core(&format!("identity-{label}"));
    let mut memory_ids = Vec::new();
    let mut previous = None;
    for status in [
        "active",
        "stale",
        "archived",
        "archive-candidate",
        "superseded",
        "active",
    ] {
        let row = store
            .append_record_with_options(
                &logical,
                "entry",
                "pair",
                Some("original-author"),
                &format!("{label} private {status} {}", memory_ids.len()),
                &json!({
                    "provenance": {"entity_id": entity_id, "source_host": "source.example"},
                    "candidate": {"person_entity_id": entity_id, "review_required": true}
                }),
                "normal",
                "high",
                previous.as_deref(),
            )
            .await
            .unwrap();
        for relation in ["audience", "subject"] {
            append_relation(
                store,
                &row.memory_id,
                entity_id,
                relation,
                &json!({}),
                "curate",
                Some("original-reviewer"),
                Some("high"),
            )
            .await
            .unwrap();
        }
        if status != "active" {
            crate::mark_memory_record_lifecycle(
                store,
                &row.memory_id,
                status,
                Some("preserve original lifecycle"),
            )
            .await
            .unwrap();
        }
        previous = (status == "superseded").then(|| row.memory_id.clone());
        memory_ids.push(row.memory_id);
    }
    let proposal = crate::create_memory_proposal(
        store,
        "review_identity_context",
        "sensitive",
        true,
        &json!({"entity_id": entity_id, "source_memory_id": memory_ids[0], "label": label}),
    )
    .await
    .unwrap();
    let observation_id = Uuid::new_v4().to_string();
    crate::create_memory_observation(
        store,
        &observation_id,
        "pending identity context",
        "high",
        &logical.to_logical_path(),
        &json!({"entity_id": entity_id, "memory_id": memory_ids[0]}),
    )
    .await
    .unwrap();
    crate::mark_observation_review_queued(store, &observation_id, &proposal.proposal_id)
        .await
        .unwrap();
    let promotion_id = Uuid::new_v4().to_string();
    let sequence_no = store.next_sequence().await.unwrap();
    let source_memory_id = &memory_ids[0];
    let target_memory_id = &memory_ids[5];
    sqlx::query!(
        "INSERT INTO memory_promotions (promotion_id, bear_id, sequence_no, source_memory_id, target_memory_id, review_agent_id, action, created_at, notes) VALUES (?, ?, ?, ?, ?, 'original-reviewer', 'review', '2026-01-01T00:00:00Z', 'original provenance')",
        promotion_id, bear_id, sequence_no, source_memory_id, target_memory_id,
    )
    .execute(store.pool()).await.unwrap();
    IdentityContext {
        active_memory_id: memory_ids[5].clone(),
        active_path: logical.to_logical_path(),
        memory_ids,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HistoricalHandle {
    handle_id: String,
    bear_id: String,
    entity_id: String,
    handle_type: String,
    handle_value: String,
    source: Option<String>,
    trust: String,
    state: String,
    created_at: String,
}

pub(super) async fn historical_handles(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Vec<HistoricalHandle> {
    let bear_id = store.bear_id().to_string();
    sqlx::query_as!(
        HistoricalHandle,
        r#"SELECT handle_id AS "handle_id!: String", bear_id, entity_id, handle_type, handle_value, source, trust, state, created_at FROM entity_handles WHERE bear_id = ? AND entity_id = ? ORDER BY handle_id"#,
        bear_id, entity_id,
    ).fetch_all(store.pool()).await.unwrap()
}

pub(super) async fn assert_historical_handles_detached(
    store: &BearMemoryStore,
    entity_id: &str,
    before: &[HistoricalHandle],
) {
    let after = historical_handles(store, entity_id).await;
    for handle in before {
        let mut expected = handle.clone();
        expected.state = "detached".into();
        assert_eq!(after.iter().find(|row| row.handle_id == handle.handle_id), Some(&expected),
            "historical handles retain their ids, owner and provenance, without retaining authority");
    }
}

// Ordered SQL snapshots include every column, including fields not projected by public
// read APIs. This detects silent merges, lifecycle rewrites, and partial rollbacks.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ContextSnapshot {
    records: String,
    links: String,
    proposals: String,
    observations: String,
    promotions: String,
    origins: String,
}

pub(super) async fn context_snapshot(store: &BearMemoryStore) -> ContextSnapshot {
    let records = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(memory_id, bear_id, sequence_no, scope_type,
            scope_profile, scope_source_kind, scope_source_id, scope_hat_id, kind, author_profile,
            author_agent_id, created_at, content_text, metadata_json, supersedes_memory_id,
            visibility, logical_path, work_surface_ref, valid_from, invalid_at, salience)) AS "rows!: String"
           FROM (SELECT * FROM memory_records ORDER BY memory_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    let links = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, qualifiers_json, author_profile, author_agent_id, confidence,
            state, supersedes_link_id, created_at, class)) AS "rows!: String"
           FROM (SELECT * FROM memory_links ORDER BY class, link_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
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
    let promotions = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(promotion_id, bear_id, sequence_no, source_memory_id,
            target_memory_id, review_agent_id, action, created_at, notes)) AS "rows!: String"
           FROM (SELECT * FROM memory_promotions ORDER BY promotion_id)"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let origins = sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(origin_id, bear_id, entity_id, source_host,
            source_bear_id, source_user_id, username, display_name, was_exporter, recorded_at)) AS "rows!: String"
           FROM (SELECT * FROM user_identity_origins ORDER BY origin_id)"#,
    ).fetch_one(store.pool()).await.unwrap();
    ContextSnapshot {
        records,
        links,
        proposals,
        observations,
        promotions,
        origins,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AuthoritySnapshot {
    bindings: String,
    generations: String,
    entities: String,
    handles: String,
    receipts: String,
    sequence: i64,
}

pub(super) async fn authority_snapshot(store: &BearMemoryStore) -> AuthoritySnapshot {
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
    let sequence = sqlx::query_scalar!(
        r#"SELECT next_sequence AS "sequence!: i64" FROM bear_sequence WHERE id = 1"#,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    AuthoritySnapshot {
        bindings,
        generations,
        entities,
        handles,
        receipts,
        sequence,
    }
}

pub(super) async fn summary(store: &BearMemoryStore, entity_id: &str) -> UserIdentitySummary {
    list_identities(store)
        .await
        .unwrap()
        .into_iter()
        .find(|identity| identity.entity_id == entity_id)
        .unwrap()
}

pub(super) async fn receipt_count(store: &BearMemoryStore) -> i64 {
    sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM user_identity_receipts"#)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

pub(super) async fn assert_receipt(
    store: &BearMemoryStore,
    review: &IdentityReconnectionReview,
    receipt: &IdentityReconnectionReceipt,
) {
    let receipt_id = receipt.receipt_id.to_string();
    let row = sqlx::query!(
        r#"SELECT bear_id, recorded_bear_id, actor_user_id AS "actor!: i32", entity_id,
            destination_user_id AS "destination?: i32", previous_user_id AS "previous!: i32",
            displaced_entity_id, prior_generation AS "prior!: i64",
            resulting_generation AS "resulting!: i64", recorded_at
           FROM user_identity_receipts WHERE receipt_id = ?"#,
        receipt_id,
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(receipt.receipt_id, review.id());
    assert_eq!(receipt.entity_id, review.entity_id());
    assert_eq!(receipt.destination_user_id, review.destination_user_id());
    assert_eq!(
        receipt.displaced_entity_id.as_deref(),
        review.displaced_entity_id()
    );
    assert_eq!(row.bear_id, store.bear_id().to_string());
    assert_eq!(row.recorded_bear_id, row.bear_id);
    assert_eq!(row.actor, ACTOR.get());
    assert_eq!(row.entity_id, receipt.entity_id);
    assert_eq!(
        row.destination.map(UserId::new),
        receipt.destination_user_id
    );
    assert_eq!(row.previous, review.selected.user_id.get());
    assert_eq!(row.displaced_entity_id, receipt.displaced_entity_id);
    assert_eq!(row.prior, review.selected.generation);
    assert_eq!(row.resulting, row.prior + 1);
    assert_eq!(row.recorded_at, receipt.recorded_at);
    assert_eq!(receipt.previous_user_id.get(), row.previous);
    assert_eq!(receipt.actor_user_id.get(), row.actor);
    assert_eq!(receipt.recorded_bear_id, row.recorded_bear_id);
    assert_eq!(receipt.prior_generation, row.prior);
    assert_eq!(receipt.resulting_generation, row.resulting);
    let recovered = list_receipts(store, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|historical| historical.receipt_id == receipt.receipt_id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
}

pub(super) async fn assert_identity_guarded(store: &BearMemoryStore, entity_id: &str) {
    let generic = create_entity(
        store,
        "person",
        Some("unrelated generic person"),
        ResolutionState::Confirmed,
        EntityTrust::Asserted,
        None,
        &json!({}),
    )
    .await
    .unwrap();
    let before = authority_snapshot(store).await;
    assert_validation(
        set_resolution(
            store,
            entity_id,
            ResolutionState::Confirmed,
            Some(EntityTrust::Asserted),
        )
        .await,
    );
    assert_validation(set_canonical_ref(store, entity_id, Some("den:user:999")).await);
    assert_validation(
        attach_handle(
            store,
            entity_id,
            "den_user",
            "999",
            None,
            EntityTrust::Asserted,
        )
        .await,
    );
    assert_validation(merge_entities(store, entity_id, &generic.entity_id).await);
    assert_validation(merge_entities(store, &generic.entity_id, entity_id).await);
    let bear_id = store.bear_id().to_string();
    let handles = sqlx::query_scalar!(
        r#"SELECT handle_id AS "handle_id!: String" FROM entity_handles WHERE bear_id = ? AND entity_id = ? ORDER BY handle_id"#,
        bear_id, entity_id,
    ).fetch_all(store.pool()).await.unwrap();
    assert!(!handles.is_empty());
    for handle_id in &handles {
        assert_validation(detach_handle(store, handle_id).await);
        assert_validation(
            split_entity(
                store,
                "person",
                Some("not a new identity"),
                std::slice::from_ref(handle_id),
                ResolutionState::Confirmed,
                EntityTrust::Asserted,
            )
            .await,
        );
    }
    assert_eq!(authority_snapshot(store).await, before);
}
