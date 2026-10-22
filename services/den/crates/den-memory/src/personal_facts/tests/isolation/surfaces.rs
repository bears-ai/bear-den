use super::*;
use sha2::Digest;

pub(super) async fn assert_generic_hidden(
    store: &BearMemoryStore,
    alice: &AuthenticatedUserEntity,
    source: Note,
    fact: PublicationReceipt,
    path: &LogicalMemoryPath,
    hat: HatId,
) {
    let id = fact.fact_id.to_string();
    let ids = [id.clone()];
    let logical = path.to_logical_path();
    assert!(crate::fetch_record_by_id(store, &id)
        .await
        .unwrap()
        .is_none());
    assert!(crate::head_record_for_logical_path(store, &logical)
        .await
        .unwrap()
        .is_none());
    assert!(crate::list_records_for_logical_path(store, &logical, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(
        crate::list_record_history_for_logical_path(store, &logical, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        crate::head_record_as_of(store, &logical, time::OffsetDateTime::now_utc())
            .await
            .unwrap()
            .is_none()
    );
    assert!(crate::effective_time_by_ids(store, &ids)
        .await
        .unwrap()
        .is_empty());
    assert!(crate::conflict_candidates(store, &ids)
        .await
        .unwrap()
        .is_empty());
    for role in ["pair", "work", "curate"] {
        assert!(crate::fetch_records_min(store, &ids, role)
            .await
            .unwrap()
            .is_empty());
        assert!(
            !crate::list_profile_local_head_records(store, role, None, 100)
                .await
                .unwrap()
                .iter()
                .any(|r| r.memory_id == id)
        );
        let browse = crate::tools::sqlite_memory_browse(store, role)
            .await
            .unwrap();
        let read = crate::tools::sqlite_memory_read(store, &logical)
            .await
            .unwrap();
        let search = crate::tools::sqlite_memory_search(store, role, NAME, 100)
            .await
            .unwrap();
        // The search envelope echoes the caller's query; only its hits come from storage.
        for output in [browse, read, search["hits"].clone()] {
            let output = output.to_string();
            for secret in [
                NAME,
                "PrivateFingerprint-4E19",
                "PrivateProvenance-61BA",
                &id,
                &fact.candidate_id.to_string(),
            ] {
                assert!(!output.contains(secret), "generic tool leaked {secret}");
            }
        }
    }
    let empty = AccessContext::empty();
    for grant in [
        crate::scoped::MemoryReadGrant::new(MemorySource::Conversation(source.conversation), None),
        crate::scoped::MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), Some(hat)),
        crate::scoped::MemoryReadGrant::new(
            MemorySource::Conversation(source.conversation),
            Some(HatId::from(Uuid::new_v4())),
        ),
    ] {
        assert!(
            crate::scoped::read_path(store, grant, &empty, &logical, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(crate::scoped::search(store, grant, &empty, NAME, 100)
            .await
            .unwrap()
            .is_empty());
        assert!(!crate::scoped::browse(store, grant, &empty)
            .await
            .unwrap()
            .contains(&logical));
    }
    assert!(crate::list_relations_for_source(store, &id, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !crate::list_relations_for_entity(store, alice.entity_id(), 100)
            .await
            .unwrap()
            .iter()
            .any(|r| r.src_memory_id == id)
    );
    assert!(crate::descriptive_entity_ids_for_records(store, &ids)
        .await
        .unwrap()
        .is_empty());
    assert!(!crate::relations::descriptive_entity_ids_by_source(store)
        .await
        .unwrap()
        .contains_key(&id));
    assert!(
        !crate::record_ids_for_entities(store, &[alice.entity_id().to_string()])
            .await
            .unwrap()
            .contains(&id)
    );
    assert!(crate::bounded_graph_expand(store, &ids, 2, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !crate::bounded_graph_expand(store, &[source.memory.to_string()], 2, 100)
            .await
            .unwrap()
            .iter()
            .any(|r| r.memory_id == id)
    );
    let indexable = crate::list_generic_indexable_heads(store).await.unwrap();
    assert!(
        indexable.is_empty(),
        "neither raw source notes nor protected personal facts are indexable"
    );
    // Legacy review must not turn a classified profile-local record into a new hat entry.
    let legacy = crate::legacy_review::inventory_page(store, None, 100)
        .await
        .unwrap();
    assert!(legacy.inventory.is_empty());
    assert!(legacy.candidates.is_empty());
    assert!(legacy.next_before.is_none());
    let candidate_error = crate::legacy_review::candidate(store, &id)
        .await
        .unwrap_err();
    assert!(matches!(&candidate_error, DenError::NotFound(_)));
    let before_reauthor = all_state(store).await;
    let reauthor_error = crate::legacy_review::reauthor_into_hat(
        store,
        crate::legacy_review::LegacyReauthoring {
            source_memory_id: id.clone(),
            target_hat: hat,
            kind: "finding".into(),
            reviewed_content: "Independently reviewed knowledge for the target hat".into(),
            expected_head: None,
            review_notes: "Reviewed for the wider Work audience without private details".into(),
            reviewer: alice.user_id(),
            work_audience_reviewed: true,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(&reauthor_error, DenError::NotFound(_)));
    assert_eq!(all_state(store).await, before_reauthor);

    let mut review_errors = vec![candidate_error, reauthor_error];
    match path.scope_type {
        crate::MemoryScopeType::Hat => {
            // Filtering protected history into an incomplete review is not safe either.
            let error = crate::hat_review::snapshot_for_hat(store, hat, 1)
                .await
                .unwrap_err();
            assert!(matches!(&error, DenError::Authorization(_)));
            review_errors.push(error);
        }
        _ => {
            let review = crate::hat_review::snapshot_for_hat(store, hat, 1)
                .await
                .unwrap();
            assert_eq!(review.total_records, 0);
            assert!(review.records.is_empty());
            assert_eq!(review.sha256, format!("{:x}", sha2::Sha256::digest(b"[]")));
        }
    }
    for error in review_errors {
        let rendered = format!("{error}\n{error:?}");
        for secret in [
            NAME,
            "PrivateFingerprint-4E19",
            "PrivateProvenance-61BA",
            &id,
            &fact.candidate_id.to_string(),
        ] {
            assert!(
                !rendered.contains(secret),
                "review error leaked protected data"
            );
        }
    }
    // Both same-path freshness and cross-path fingerprint lookup must ignore personal bodies.
    for (target, proposed) in [
        (logical.as_str(), "A public proposal unrelated to the name"),
        ("core/unrelated.md", "PERSONALNAMESENTINEL-8D24"),
    ] {
        let proposal = crate::create_memory_proposal(
            store,
            "write",
            "normal",
            false,
            &json!({
                "target_ref": target, "proposed_content": proposed,
            }),
        )
        .await
        .unwrap();
        assert!(proposal.payload_json.get("freshness").is_none());
        assert!(proposal.payload_json.get("consolidation_review").is_none());
        let serialized = proposal.payload_json.to_string();
        for secret in [
            NAME,
            "PrivateFingerprint-4E19",
            "PrivateProvenance-61BA",
            &id,
            &fact.candidate_id.to_string(),
        ] {
            assert!(!serialized.contains(secret));
        }
    }
    assert!(crate::fetch_record_by_id(store, &source.memory.to_string())
        .await
        .unwrap()
        .is_some());
}
