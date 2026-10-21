use super::*;

async fn assert_no_generic_candidate(
    store: &BearMemoryStore,
    id: PersonalCandidateId,
    value: &str,
    source: Note,
) {
    let id = id.to_string();
    assert!(crate::get_memory_proposal(store, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::get_memory_observation(store, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::fetch_record_by_id(store, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::fetch_records_min(store, &[id], "curate")
        .await
        .unwrap()
        .is_empty());
    assert!(crate::list_memory_proposals(store, None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(crate::list_memory_proposals(store, Some("pending"), 100)
        .await
        .unwrap()
        .is_empty());
    // This is the generic curate queue: candidate ids must never be proposal ids.
    assert!(crate::list_reviewable_memory_proposals(store)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(crate::count_memory_proposals(store, None).await.unwrap(), 0);
    let grant =
        crate::scoped::MemoryReadGrant::new(MemorySource::Conversation(source.conversation), None);
    let access = crate::AccessContext::empty();
    assert!(crate::scoped::search(store, grant, &access, value, 100)
        .await
        .unwrap()
        .is_empty());
    let paths = crate::scoped::browse(store, grant, &access).await.unwrap();
    assert_eq!(
        paths.len(),
        1,
        "only the existing canonical source note is browsable"
    );
    let records = crate::scoped::read_path(store, grant, &access, &paths[0], 100)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].memory_id, source.memory.to_string());
    assert!(!records[0].content_text.contains(value));
    assert!(!records[0].metadata_json.to_string().contains(value));
}

#[tokio::test]
async fn all_personal_states_remain_outside_generic_records_proposals_observations_and_queues() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let before = generic_snapshot(&store).await;
    let value = "CandidateOnlySentinel-A7E9";
    let candidate = stage(&store, &alice, source, value).await;
    assert_eq!(
        generic_snapshot(&store).await,
        before,
        "staging must not copy a source, publish a fact, or enqueue generic curate work"
    );
    assert_no_generic_candidate(&store, candidate.id(), value, source).await;
    quarantine_candidate(&store, candidate.id()).await;
    assert_no_generic_candidate(&store, candidate.id(), value, source).await;
    assert!(withdraw_candidate(&store, &alice, candidate.id())
        .await
        .unwrap());
    assert_no_generic_candidate(&store, candidate.id(), value, source).await;
    set_capture_enabled(&store, &alice, false).await.unwrap();
    set_capture_enabled(&store, &alice, true).await.unwrap();
    assert_eq!(generic_snapshot(&store).await, before);
    assert_eq!(
        current_candidate(&store, &alice, candidate.id())
            .await
            .preferred_name()
            .as_str(),
        value
    );
}

#[tokio::test]
async fn dedicated_table_enforces_preferred_name_explicit_user_request_and_durable_scope() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let candidate = stage(&store, &alice, source, "Alice").await;
    let id = candidate.id().to_string();
    let before = snapshot(&store).await;
    for (fact, evidence, retention) in [
        ("biography", "explicit_user_request", "durable"),
        ("preferred_name", "inferred", "durable"),
        ("preferred_name", "explicit_user_request", "ephemeral"),
    ] {
        let error = sqlx::query!(
            "UPDATE personal_memory_candidates SET fact_type = ?, evidence_kind = ?, retention = ? WHERE candidate_id = ?",
            fact, evidence, retention, id,
        ).execute(store.pool()).await.unwrap_err();
        assert!(error.as_database_error().unwrap().is_check_violation());
        assert_eq!(snapshot(&store).await, before);
    }
}

#[tokio::test]
async fn ordinary_admin_inspection_and_search_cannot_load_candidate_bodies() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let value = "AdminMustNotFindCandidate-76DF";
    let candidate = stage(&store, &alice, source, value).await;
    let id = candidate.id().to_string();
    // Admin APIs require a manager. Snapshot the new_test_store database into its
    // isolated directory; no network service, test-only Config constructor or env writes.
    let directory = std::env::temp_dir().join(format!("den-personal-admin-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}.sqlite", store.bear_id()));
    let path_text = path.to_str().unwrap();
    sqlx::query!("VACUUM INTO ?", path_text)
        .execute(store.pool())
        .await
        .unwrap();
    let mut config = den_core::config::Config::try_load().unwrap();
    config.bear_sqlite_data_dir = directory.to_str().unwrap().to_owned();
    let manager = crate::MemoryStoreManager::new(&config);
    let bear = store.bear_id();
    assert!(crate::get_memory_record_by_id(&manager, bear, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::get_memory_record_detail(&manager, bear, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::search_memory_records(&manager, bear, value, 100)
        .await
        .unwrap()
        .is_empty());
    let recent = crate::list_recent_memory_records(&manager, bear, 50)
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].memory_id, source.memory.to_string());
    assert!(!serde_json::to_string(&recent).unwrap().contains(value));
    let paths = crate::list_path_summaries(&manager, bear).await.unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].head_memory_id, source.memory.to_string());
    let stats = crate::bear_memory_admin_stats(&manager, &config, bear)
        .await
        .unwrap();
    assert_eq!(stats.record_count, 1);
    assert_eq!(stats.pending_proposals, 0);
    assert_eq!(stats.pending_observations, 0);
    // The protected owner API on that SAME copied store still has the minimized value.
    let inspection_store = manager.store_for_bear(bear).await.unwrap();
    let owner = person(&inspection_store, ALICE).await;
    assert_eq!(
        current_candidate(&inspection_store, &owner, candidate.id())
            .await
            .preferred_name()
            .as_str(),
        value
    );
    inspection_store.pool().close().await;
    drop(manager);
    std::fs::remove_dir_all(directory).unwrap();
}
