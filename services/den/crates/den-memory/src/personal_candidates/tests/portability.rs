use super::*;

/// Simulate the portable database's owner-key remap, not a new import implementation.
/// Each table is a literal, parameterized SQLx query; source/audit Bear IDs are NOT owners.
async fn remap_owner(store: &BearMemoryStore, target: Uuid) -> BearMemoryStore {
    let source = store.bear_id().to_string();
    let target_id = target.to_string();
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query!(
        "UPDATE memory_records SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE entities SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE entity_handles SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE authenticated_user_entities SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_generations SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_origins SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE user_identity_receipts SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE personal_memory_settings SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE personal_memory_candidates SET bear_id = ? WHERE bear_id = ?",
        target_id,
        source
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    BearMemoryStore::new(target, store.pool().clone())
}

async fn original_provenance(store: &BearMemoryStore, original_bear: Uuid) -> String {
    let original = original_bear.to_string();
    sqlx::query_scalar!(
        r#"SELECT json_group_array(json_array(candidate_id, person_entity_id, sequence_no,
            fact_type, value_text, evidence_kind, retention, source_conversation_id, source_memory_id,
            source_bear_id, source_user_id, source_binding_generation, created_at,
            withdrawn_at, withdrawn_on_bear_id, withdrawn_by_user_id)) AS "rows!: String"
           FROM (SELECT * FROM personal_memory_candidates WHERE source_bear_id = ? ORDER BY candidate_id)"#,
        original,
    ).fetch_one(store.pool()).await.unwrap()
}

#[tokio::test]
async fn owner_bear_remap_preserves_source_qualified_provenance_but_not_imported_authority_or_consent(
) {
    let source_store = new_test_store().await;
    let original_bear = source_store.bear_id();
    let alice = opted_in(&source_store, ALICE).await;
    let source = note(&source_store).await;
    let pending = stage(&source_store, &alice, source, "Portable preferred name").await;
    let quarantined = stage(&source_store, &alice, source, "Historical quarantine").await;
    quarantine_candidate(&source_store, quarantined.id()).await;
    let withdrawn = stage(&source_store, &alice, source, "Historical withdrawal").await;
    withdraw_candidate(&source_store, &alice, withdrawn.id())
        .await
        .unwrap();
    crate::identity_reconnection::record_export_origins(
        &source_store,
        "https://source.example",
        ALICE,
        &[],
    )
    .await
    .unwrap();
    let provenance = original_provenance(&source_store, original_bear).await;
    let target = remap_owner(&source_store, Uuid::new_v4()).await;
    let imported = quarantine_imported_authenticated_users(&target)
        .await
        .unwrap();
    assert_eq!(imported, vec![alice.entity_id().to_owned()]);
    assert_all_denied(&target, &alice, source, pending.id()).await;
    assert_eq!(
        original_provenance(&target, original_bear).await,
        provenance
    );
    let target_id = target.bear_id().to_string();
    let settings = sqlx::query!(
        r#"SELECT capture_enabled AS "enabled!: i64", updated_on_bear_id,
            updated_by_user_id AS "actor?: i32" FROM personal_memory_settings WHERE bear_id = ?"#,
        target_id,
    )
    .fetch_one(target.pool())
    .await
    .unwrap();
    assert_eq!(settings.enabled, 0);
    assert_eq!(settings.updated_on_bear_id, target_id);
    assert_eq!(settings.actor, None);
    // The same host-local numeric id resolves a NEW person, not the portable original.
    let numeric_collision = person(&target, ALICE).await;
    assert_ne!(numeric_collision.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&target, &numeric_collision).await.unwrap());
    assert!(get_candidate(&target, &numeric_collision, pending.id())
        .await
        .unwrap()
        .is_none());
    assert!(list_candidates(&target, &numeric_collision, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !withdraw_candidate(&target, &numeric_collision, pending.id())
            .await
            .unwrap()
    );
    let destination_user = UserId::new(101);
    let attached = reconnect(
        &target,
        alice.entity_id(),
        destination_user,
        ReplacementConsent::NoReplacement,
    )
    .await;
    assert!(!capture_enabled(&target, &attached).await.unwrap());
    let history = list_candidates(&target, &attached, 100).await.unwrap();
    assert_eq!(history.len(), 3);
    for row in &history {
        assert_eq!(row.source_bear_id().as_uuid(), original_bear);
        assert_eq!(row.source_user_id(), ALICE);
        assert_eq!(row.source_binding_generation(), alice.authority_generation);
        assert_eq!(row.source_conversation_id(), source.conversation);
        assert_eq!(row.source_memory_id(), source.memory);
        assert_eq!(row.person_entity_id(), alice.entity_id());
    }
    assert_eq!(
        current_candidate(&target, &attached, pending.id())
            .await
            .state(),
        CandidateState::Quarantined
    );
    assert_eq!(
        current_candidate(&target, &attached, quarantined.id())
            .await
            .state(),
        CandidateState::Quarantined
    );
    assert_eq!(
        current_candidate(&target, &attached, withdrawn.id())
            .await
            .state(),
        CandidateState::Withdrawn
    );
    let new_source = note(&target).await;
    assert_denied(
        stage_preferred_name(
            &target,
            &new_source.proof(&attached),
            PreferredName::new("Portable preferred name").unwrap(),
        )
        .await,
    );
    set_capture_enabled(&target, &attached, true).await.unwrap();
    let fresh = stage(&target, &attached, new_source, "Portable preferred name").await;
    assert_ne!(fresh.id(), pending.id());
    assert_eq!(fresh.source_bear_id(), attached.bear_id());
    assert_eq!(fresh.source_user_id(), destination_user);
    assert_eq!(
        fresh.source_binding_generation(),
        attached.authority_generation
    );
    assert_eq!(
        original_provenance(&target, original_bear).await,
        provenance
    );
    assert_eq!(
        state(&target, pending.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(
        state(&target, quarantined.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(
        state(&target, withdrawn.id()).await,
        CandidateState::Withdrawn
    );
    let attached_entity_id = attached.entity_id();
    let fresh_settings = sqlx::query!(
        r#"SELECT authority_generation, updated_on_bear_id, updated_by_user_id AS "actor?: i32"
           FROM personal_memory_settings WHERE bear_id = ? AND person_entity_id = ?"#,
        target_id,
        attached_entity_id,
    )
    .fetch_one(target.pool())
    .await
    .unwrap();
    assert_eq!(
        fresh_settings.authority_generation,
        attached.authority_generation
    );
    assert_eq!(fresh_settings.updated_on_bear_id, target_id);
    assert_eq!(fresh_settings.actor, Some(destination_user.get()));
}
