use super::*;
use den_core::ids::HatId;

mod surfaces;

const NAME: &str = "PersonalNameSentinel-8D24";
const METADATA: &str =
    r#"{"claim_fingerprint":"PrivateFingerprint-4E19","provenance":"PrivateProvenance-61BA"}"#;

#[tokio::test]
async fn generic_surfaces_ignore_paths_scopes_and_lost_acl_while_owner_heads_fail_closed() {
    for scope in 0..4 {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let source = note(&store).await;
        let fact = publish(&store, &alice, source, NAME, None).await;
        let hat = HatId::from(Uuid::new_v4());
        let path = match scope {
            0 => LogicalMemoryPath::shared_core("personal-sentinel"),
            1 => LogicalMemoryPath::profile_local("pair", "personal-sentinel"),
            2 => LogicalMemoryPath::source_local(
                MemorySource::Conversation(source.conversation),
                "personal-sentinel",
            ),
            _ => LogicalMemoryPath::hat(hat, "personal-sentinel"),
        };
        // A real public neighbor makes graph assertions exercise both directions.
        crate::append_relation(
            &store,
            &source.memory.to_string(),
            alice.entity_id(),
            "subject",
            &json!({}),
            "pair",
            None,
            None,
        )
        .await
        .unwrap();
        fixtures::alter_record(
            &store,
            &fact.fact_id.to_string(),
            &path,
            "normal",
            METADATA,
            None,
        )
        .await;
        assert_head(&store, &alice, fact, PersonalFactState::Active, Some(NAME)).await;
        surfaces::assert_generic_hidden(&store, &alice, source, fact, &path, hat).await;
        assert!(crate::head_record_for_logical_path_with_access(
            &store,
            &path.to_logical_path(),
            &AccessContext::for_authenticated_user(&alice)
        )
        .await
        .unwrap()
        .is_some());

        // Canonical visibility/lifecycle failures suppress the retained head, not its marker.
        for (visibility, metadata, invalid_at) in [
            ("private", METADATA, None),
            ("normal", r#"{"lifecycle":{"status":"archived"}}"#, None),
            ("normal", "{invalid", None),
            ("normal", METADATA, Some("2026-01-01T00:00:00Z")),
        ] {
            fixtures::alter_record(
                &store,
                &fact.fact_id.to_string(),
                &path,
                visibility,
                metadata,
                invalid_at,
            )
            .await;
            assert_head(&store, &alice, fact, PersonalFactState::Suppressed, None).await;
            assert!(!crate::record_visible(
                &store,
                &fact.fact_id.to_string(),
                &AccessContext::for_authenticated_user(&alice)
            )
            .await
            .unwrap());
        }
        fixtures::alter_record(
            &store,
            &fact.fact_id.to_string(),
            &path,
            "normal",
            METADATA,
            None,
        )
        .await;
        fixtures::lose_all_acl(&store, &fact.fact_id.to_string()).await;
        for access in [
            AccessContext::empty(),
            AccessContext::empty().with_audience([alice.entity_id()]),
            AccessContext::for_authenticated_user(&alice),
        ] {
            assert!(
                !crate::record_visible(&store, &fact.fact_id.to_string(), &access)
                    .await
                    .unwrap()
            );
        }
        assert_head(&store, &alice, fact, PersonalFactState::Suppressed, None).await;
        surfaces::assert_generic_hidden(&store, &alice, source, fact, &path, hat).await;
        let next = stage(&store, &alice, source, "Correct suppressed head").await;
        denied(publish_preferred_name(&store, &source.proof(&alice), next.id(), None).await);
        assert!(retract_preferred_name(&store, &alice, fact.fact_id)
            .await
            .unwrap());
        // Even repaired lifecycle/path data and a lost ACL cannot undo the retraction decision.
        fixtures::alter_record(
            &store,
            &fact.fact_id.to_string(),
            &path,
            "normal",
            METADATA,
            None,
        )
        .await;
        assert_head(&store, &alice, fact, PersonalFactState::Retracted, None).await;
        surfaces::assert_generic_hidden(&store, &alice, source, fact, &path, hat).await;
    }
}

#[tokio::test]
async fn generic_writes_lifecycle_supersession_and_relation_append_cannot_mutate_classified_facts()
{
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, NAME, None).await;
    let path = LogicalMemoryPath::shared_core("personal-sentinel");
    fixtures::alter_record(
        &store,
        &fact.fact_id.to_string(),
        &path,
        "normal",
        METADATA,
        None,
    )
    .await;
    for retracted in [false, true] {
        if retracted {
            retract_preferred_name(&store, &alice, fact.fact_id)
                .await
                .unwrap();
            fixtures::lose_all_acl(&store, &fact.fact_id.to_string()).await;
        }
        let before = all_state(&store).await;
        for status in ["active", "archived", "superseded"] {
            denied(
                crate::mark_memory_record_lifecycle(
                    &store,
                    &fact.fact_id.to_string(),
                    status,
                    Some("generic attempt"),
                )
                .await,
            );
        }
        denied(
            store
                .append_record_with_options(
                    &path,
                    "note",
                    "curate",
                    None,
                    "Generic replacement",
                    &json!({}),
                    "normal",
                    "normal",
                    Some(&fact.fact_id.to_string()),
                )
                .await,
        );
        for relation in ["subject", "audience", "confined_to"] {
            denied(
                crate::append_relation(
                    &store,
                    &fact.fact_id.to_string(),
                    alice.entity_id(),
                    relation,
                    &json!({}),
                    "curate",
                    None,
                    None,
                )
                .await,
            );
        }
        assert!(crate::create_verified_hat_proposal(
            &store,
            "normal",
            false,
            &json!({}),
            crate::VerifiedHatProposalSource {
                memory_id: fact.fact_id.0,
                hat_id: HatId::from(Uuid::new_v4())
            }
        )
        .await
        .is_err());
        assert_eq!(all_state(&store).await, before);
        // Generic path writers cannot discover a classified head to supersede; a new
        // unclassified record at the locator must leave the protected version untouched.
        let generic = crate::append_memory_record(
            &store,
            &LogicalMemoryPath::shared_core("unrelated"),
            "note",
            "curate",
            None,
            "Public write",
            &json!({}),
        )
        .await
        .unwrap();
        assert!(crate::fetch_record_by_id(&store, &generic.memory_id)
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            row(&generic_snapshot(&store).await.0, "records", fact.fact_id),
            row(&before.1 .0, "records", fact.fact_id)
        );
    }
}

#[tokio::test]
async fn admin_read_search_paths_counts_and_details_never_project_personal_versions() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let fact = publish(&store, &alice, source, NAME, None).await;
    let logical = LogicalMemoryPath::shared_core("personal-sentinel");
    fixtures::alter_record(
        &store,
        &fact.fact_id.to_string(),
        &logical,
        "normal",
        METADATA,
        None,
    )
    .await;
    for phase in 0..3 {
        if phase == 1 {
            retract_preferred_name(&store, &alice, fact.fact_id)
                .await
                .unwrap();
        }
        if phase == 2 {
            fixtures::lose_all_acl(&store, &fact.fact_id.to_string()).await;
        }
        // Reuse the existing admin-fixture pattern and EXACT cached VACUUM query.
        let directory =
            std::env::temp_dir().join(format!("den-personal-fact-admin-{}", Uuid::new_v4()));
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
        let id = fact.fact_id.to_string();
        assert!(crate::get_memory_record_by_id(&manager, bear, &id)
            .await
            .unwrap()
            .is_none());
        assert!(crate::get_memory_record_detail(&manager, bear, &id)
            .await
            .unwrap()
            .is_none());
        for query in [NAME, "PrivateFingerprint-4E19", "PrivateProvenance-61BA"] {
            assert!(crate::search_memory_records(&manager, bear, query, 100)
                .await
                .unwrap()
                .is_empty());
        }
        let recent = crate::list_recent_memory_records(&manager, bear, 1)
            .await
            .unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].memory_id, source.memory.to_string());
        let paths = crate::list_path_summaries(&manager, bear).await.unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].head_memory_id, source.memory.to_string());
        assert!(!crate::list_all_logical_paths(&manager, bear)
            .await
            .unwrap()
            .contains(&logical.to_logical_path()));
        let stats = crate::bear_memory_admin_stats(&manager, &config, bear)
            .await
            .unwrap();
        // Physical record count is intentionally used by import-integrity checks.
        assert_eq!(stats.record_count, 2);
        assert_eq!(stats.shared_count, 0);
        assert_eq!(stats.distinct_paths, 1);
        let buckets = crate::count_records_by_kind(&manager, bear).await.unwrap();
        assert!(!buckets
            .iter()
            .any(|bucket| bucket.label == "personal_preferred_name"));
        let inspection = manager.store_for_bear(bear).await.unwrap();
        let owner = person(&inspection, ALICE).await;
        assert_head(
            &inspection,
            &owner,
            fact,
            if phase == 0 {
                PersonalFactState::Active
            } else {
                PersonalFactState::Retracted
            },
            if phase == 0 { Some(NAME) } else { None },
        )
        .await;
        inspection.pool().close().await;
        drop(manager);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
