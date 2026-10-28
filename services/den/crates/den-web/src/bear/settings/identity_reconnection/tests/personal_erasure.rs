use super::*;
use den_memory::{
    personal_candidates::{
        capture_enabled, capture_setting, get_candidate, list_candidates, set_capture_enabled,
        stage_owner_preferred_name, CandidateState, PreferredName, VerifiedOwnerSource,
        VerifiedPersonalSource,
    },
    personal_erasure::{
        erase_preferred_name_memory, mark_erasure_cleanup_complete, pending_erasure_cleanups,
    },
    personal_facts::{current_preferred_name, publish_preferred_name, PersonalFactState},
    resolve_authenticated_user_entity,
    test_support::canonical_record_contents_match,
};

#[sqlx::test(migrations = "../../migrations")]
async fn v6_export_import_preserves_erased_bodies_and_source_ledger_without_destination_cleanup_authority(
    pool: PgPool,
) {
    let fixture = fixture(pool).await;
    let source = fixture.store().await;
    let source_owner = resolve_authenticated_user_entity(&source, fixture.actor)
        .await
        .unwrap();
    set_capture_enabled(&source, &source_owner, true)
        .await
        .unwrap();
    let proof = VerifiedOwnerSource::for_authorized_owner_request(&source_owner, Uuid::new_v4());
    let candidate = stage_owner_preferred_name(
        &source,
        &proof,
        PreferredName::new("V6-erased-owner-name-sentinel").unwrap(),
    )
    .await
    .unwrap();
    let publication = publish_preferred_name(
        &source,
        &VerifiedPersonalSource::from(&proof),
        candidate.id(),
        None,
    )
    .await
    .unwrap();
    let head = current_preferred_name(&source, &source_owner)
        .await
        .unwrap()
        .unwrap();
    let latest = list_candidates(&source, &source_owner, 1).await.unwrap()[0].id();
    let capture = capture_setting(&source, &source_owner).await.unwrap();
    assert_eq!(head.fact_id, publication.fact_id);
    assert_eq!(head.state, PersonalFactState::Active);
    assert_eq!(latest, candidate.id());
    assert!(capture.enabled);
    let request_id = Uuid::new_v4();
    // Stop at the core commit: simulate a crash before PG cleanup/ACK. In
    // particular, never call the service inspector that automatically retries it.
    let erasure = erase_preferred_name_memory(
        &source,
        &source_owner,
        request_id,
        Some(head.fact_id),
        Some(latest),
        capture.revision,
    )
    .await
    .unwrap();
    assert_eq!(erasure.affected_fact_ids, vec![publication.fact_id]);
    assert_eq!(
        pending_erasure_cleanups(&source, &source_owner, 100)
            .await
            .unwrap(),
        vec![erasure.clone()]
    );
    let fact_id = publication.fact_id.to_string();
    let expected_bodies = [
        (fact_id.as_str(), ""),
        (
            fixture.private_record.as_str(),
            "private-memory-canary-never-render",
        ),
    ];
    assert!(canonical_record_contents_match(&source, &expected_bodies)
        .await
        .unwrap());

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/bear/identityreview/export.bear")
                .header(header::COOKIE, &fixture.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    // No more source DB reads: validate the actual exported bytes and imported
    // destination, not a hand-copied database or a rewritten fixture table.
    drop(source);
    let (manifest, memory) = crate::bear::settings::bundle_io::read_bear_bundle(&bytes).unwrap();
    assert_eq!(manifest.version, 6);
    let slug = crate::bear::settings::import_creation::create(
        &fixture.state,
        fixture.actor,
        manifest,
        memory,
    )
    .await
    .unwrap();
    let imported = bears_db::bear_for_user_by_slug(&fixture.pool, fixture.actor.get(), &slug)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(imported.id, fixture.bear_id.as_uuid());
    bears_db::grant_membership(
        &fixture.pool,
        fixture.destination.get(),
        imported.id,
        Some(BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    let destination = fixture
        .state
        .memory_stores()
        .unwrap()
        .store_for_bear(imported.id)
        .await
        .unwrap();
    let identities = core::list_identities(&destination).await.unwrap();
    let parked = identities
        .iter()
        .find(|identity| identity.entity_id == source_owner.entity_id())
        .unwrap();
    assert!(parked.active_user_id.is_none());
    assert!(!parked.origins.is_empty());
    let review = core::review_reconnection(
        &destination,
        fixture.actor,
        source_owner.entity_id(),
        fixture.destination,
    )
    .await
    .unwrap();
    assert!(review.displaced_entity_id().is_none());
    core::commit_review(
        &destination,
        &review,
        core::ReplacementConsent::NoReplacement,
    )
    .await
    .unwrap();
    let owner = resolve_authenticated_user_entity(&destination, fixture.destination)
        .await
        .unwrap();
    assert_eq!(owner.bear_id(), BearId::new(imported.id));
    assert_eq!(owner.user_id(), fixture.destination);
    assert_eq!(owner.entity_id(), source_owner.entity_id());
    assert!(!capture_enabled(&destination, &owner).await.unwrap());

    let candidates = list_candidates(&destination, &owner, 100).await.unwrap();
    assert_eq!(candidates.len(), 1);
    let erased = get_candidate(&destination, &owner, candidate.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidates[0].id(), candidate.id());
    for row in [&candidates[0], &erased] {
        assert_eq!(row.state(), CandidateState::Erased);
        assert!(row.preferred_name().is_none());
        assert_eq!(row.person_entity_id(), candidate.person_entity_id());
        assert_eq!(row.source(), candidate.source());
        assert_eq!(row.source_bear_id(), fixture.bear_id);
        assert_eq!(row.source_user_id(), fixture.actor);
        assert_eq!(
            row.source_binding_generation(),
            candidate.source_binding_generation()
        );
    }
    let current = current_preferred_name(&destination, &owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.fact_id, publication.fact_id);
    assert_eq!(current.candidate_id, candidate.id());
    assert_eq!(current.state, PersonalFactState::Retracted);
    assert!(current.preferred_name.is_none());
    assert!(
        canonical_record_contents_match(&destination, &expected_bodies)
            .await
            .unwrap()
    );
    assert!(den_memory::fetch_record_by_id(&destination, &fact_id)
        .await
        .unwrap()
        .is_none());
    assert!(
        den_memory::fetch_record_by_id(&destination, &fixture.private_record)
            .await
            .unwrap()
            .is_none()
    );
    assert!(den_memory::list_generic_indexable_heads(&destination)
        .await
        .unwrap()
        .iter()
        .all(|row| row.memory_id != fact_id));

    assert!(pending_erasure_cleanups(&destination, &owner, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        mark_erasure_cleanup_complete(&destination, &owner, erasure.erasure_id).await,
        Err(den_core::DenError::Authorization(_)),
    ));
    // Fresh destination CAS values would permit a new no-op erasure if the
    // source ledger were lost or its owning Bear were not rebased. The reserved
    // request ID must instead find the imported row and reject its source host.
    let fresh_capture = capture_setting(&destination, &owner).await.unwrap();
    assert!(matches!(
        erase_preferred_name_memory(
            &destination,
            &owner,
            request_id,
            Some(current.fact_id),
            Some(erased.id()),
            fresh_capture.revision
        )
        .await,
        Err(den_core::DenError::Authorization(_)),
    ));
    assert!(pending_erasure_cleanups(&destination, &owner, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(
        canonical_record_contents_match(&destination, &expected_bodies)
            .await
            .unwrap()
    );
}
