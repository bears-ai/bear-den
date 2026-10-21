use super::*;
use den_memory::{
    personal_candidates::{
        capture_enabled, get_candidate, list_candidates, set_capture_enabled, stage_preferred_name,
        withdraw_candidate, CandidateState, PersonalCandidate, PreferredName,
        VerifiedPersonalSource,
    },
    resolve_authenticated_user_entity, AuthenticatedUserEntity, MemorySource,
};

fn assert_preserved(
    actual: &PersonalCandidate,
    original: &PersonalCandidate,
    state: CandidateState,
) {
    assert_eq!(actual.id(), original.id());
    assert_eq!(actual.person_entity_id(), original.person_entity_id());
    assert_eq!(actual.preferred_name(), original.preferred_name());
    assert_eq!(actual.fact_type(), original.fact_type());
    assert_eq!(actual.evidence(), original.evidence());
    assert_eq!(actual.retention(), original.retention());
    assert_eq!(actual.source_bear_id(), original.source_bear_id());
    assert_eq!(
        actual.source_conversation_id(),
        original.source_conversation_id()
    );
    assert_eq!(actual.source_memory_id(), original.source_memory_id());
    assert_eq!(actual.source_user_id(), original.source_user_id());
    assert_eq!(
        actual.source_binding_generation(),
        original.source_binding_generation()
    );
    assert_eq!(actual.created_at(), original.created_at());
    assert_eq!(actual.withdrawn_at(), original.withdrawn_at());
    assert_eq!(actual.state(), state);
}

async fn stage(
    store: &BearMemoryStore,
    source: &VerifiedPersonalSource,
    name: &str,
) -> PersonalCandidate {
    stage_preferred_name(store, source, PreferredName::new(name).unwrap())
        .await
        .unwrap()
}

async fn assert_hidden(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    originals: &[PersonalCandidate],
) {
    assert!(list_candidates(store, binding, 100)
        .await
        .unwrap()
        .is_empty());
    for original in originals {
        assert!(get_candidate(store, binding, original.id())
            .await
            .unwrap()
            .is_none());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn real_bundle_import_quarantines_candidates_without_rewriting_person_or_provenance(
    pool: PgPool,
) {
    let fixture = fixture(pool).await;
    let source = fixture.store().await;
    let alice = resolve_authenticated_user_entity(&source, fixture.actor)
        .await
        .unwrap();
    let conversation = Uuid::new_v4();
    let note = source
        .append_record(
            &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
            "note",
            "pair",
            None,
            "Please remember my preferred name; leave this source body unchanged.",
            &json!({"source_canary": "unchanged"}),
            "normal",
        )
        .await
        .unwrap();
    let mut bodies = Vec::new();
    for id in [&note.memory_id, &fixture.private_record] {
        let record = den_memory::fetch_record_by_id(&source, id)
            .await
            .unwrap()
            .unwrap();
        bodies.push((id.clone(), serde_json::to_value(record).unwrap()));
    }
    // This memory fixture supplies trusted proof; Postgres source authority is tested elsewhere.
    let proof = VerifiedPersonalSource::for_authorized_conversation(
        &alice,
        conversation,
        Uuid::parse_str(&note.memory_id).unwrap(),
    );
    set_capture_enabled(&source, &alice, true).await.unwrap();
    let historical = stage(&source, &proof, "Explicit historical name").await;
    let withdrawn = stage(&source, &proof, "Explicit withdrawn name").await;
    assert!(withdraw_candidate(&source, &alice, withdrawn.id())
        .await
        .unwrap());
    let withdrawn = get_candidate(&source, &alice, withdrawn.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(withdrawn.state(), CandidateState::Withdrawn);
    assert!(withdrawn.withdrawn_at().is_some());

    let review = core::review_revocation(&source, fixture.actor, alice.entity_id())
        .await
        .unwrap();
    let revoked = core::commit_review(&source, &review, core::ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_eq!(
        historical.source_binding_generation(),
        revoked.prior_generation
    );
    let review =
        core::review_reconnection(&source, fixture.actor, alice.entity_id(), fixture.actor)
            .await
            .unwrap();
    let reconnected =
        core::commit_review(&source, &review, core::ReplacementConsent::NoReplacement)
            .await
            .unwrap();
    let current = resolve_authenticated_user_entity(&source, fixture.actor)
        .await
        .unwrap();
    assert_eq!(current.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&source, &current).await.unwrap());
    let quarantined = get_candidate(&source, &current, historical.id())
        .await
        .unwrap()
        .unwrap();
    assert_preserved(&quarantined, &historical, CandidateState::Quarantined);
    set_capture_enabled(&source, &current, true).await.unwrap();
    let proof = VerifiedPersonalSource::for_authorized_conversation(
        &current,
        conversation,
        Uuid::parse_str(&note.memory_id).unwrap(),
    );
    let pending = stage(&source, &proof, "Explicit new pending name").await;
    assert_ne!(pending.id(), historical.id());
    assert_eq!(pending.state(), CandidateState::Pending);
    assert_eq!(
        pending.source_binding_generation(),
        reconnected.resulting_generation
    );
    let originals = [pending, withdrawn, quarantined];
    for row in &originals {
        assert_eq!(row.person_entity_id(), alice.entity_id());
        assert_eq!(row.source_bear_id(), fixture.bear_id);
        assert_eq!(row.source_user_id(), fixture.actor);
        assert_eq!(row.source_conversation_id(), conversation);
        assert_eq!(row.source_memory_id().to_string(), note.memory_id);
    }

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
    let (manifest, memory) = crate::bear::settings::bundle_io::read_bear_bundle(&bytes).unwrap();
    let identities = core::list_identities(&source).await.unwrap();
    let original_identity = identities
        .iter()
        .find(|row| row.entity_id == alice.entity_id())
        .unwrap();
    assert!(!original_identity.origins.is_empty());
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
    let destination = fixture
        .state
        .memory_stores()
        .unwrap()
        .store_for_bear(imported.id)
        .await
        .unwrap();
    let imported_identities = core::list_identities(&destination).await.unwrap();
    let imported_identity = imported_identities
        .iter()
        .find(|row| row.entity_id == alice.entity_id())
        .unwrap();
    assert!(imported_identity.active_user_id.is_none());
    assert_eq!(imported_identity.origins, original_identity.origins);

    // Even the same authenticated account number must get a fresh destination person.
    let fresh = resolve_authenticated_user_entity(&destination, fixture.actor)
        .await
        .unwrap();
    assert_ne!(fresh.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&destination, &fresh).await.unwrap());
    assert_hidden(&destination, &fresh, &originals).await;
    let review = core::review_reconnection(
        &destination,
        fixture.actor,
        alice.entity_id(),
        fixture.actor,
    )
    .await
    .unwrap();
    assert_eq!(review.displaced_entity_id(), Some(fresh.entity_id()));
    let receipt = core::commit_review(
        &destination,
        &review,
        core::ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    assert_eq!(
        receipt.displaced_entity_id.as_deref(),
        Some(fresh.entity_id())
    );
    let owner = resolve_authenticated_user_entity(&destination, fixture.actor)
        .await
        .unwrap();
    assert_eq!(owner.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&destination, &owner).await.unwrap());
    let identities = core::list_identities(&destination).await.unwrap();
    let parked = identities
        .iter()
        .find(|row| row.entity_id == fresh.entity_id())
        .unwrap();
    assert!(parked.active_user_id.is_none());
    assert_eq!(
        identities
            .iter()
            .find(|row| row.entity_id == alice.entity_id())
            .unwrap()
            .origins,
        original_identity.origins
    );
    let rows = list_candidates(&destination, &owner, 100).await.unwrap();
    assert_eq!(rows.len(), originals.len());
    for original in &originals {
        let state = match original.state() {
            CandidateState::Pending | CandidateState::Quarantined => CandidateState::Quarantined,
            CandidateState::Withdrawn => CandidateState::Withdrawn,
        };
        let listed = rows.iter().find(|row| row.id() == original.id()).unwrap();
        assert_preserved(listed, original, state);
        let fetched = get_candidate(&destination, &owner, original.id())
            .await
            .unwrap()
            .unwrap();
        assert_preserved(&fetched, original, state);
    }
    for user in [UserId::new(4343), fixture.member] {
        let unrelated = resolve_authenticated_user_entity(&destination, user)
            .await
            .unwrap();
        assert_hidden(&destination, &unrelated, &originals).await;
    }

    assert!(capture_enabled(&source, &current).await.unwrap());
    let source_rows = list_candidates(&source, &current, 100).await.unwrap();
    assert_eq!(source_rows.len(), originals.len());
    for original in &originals {
        let row = source_rows
            .iter()
            .find(|row| row.id() == original.id())
            .unwrap();
        assert_preserved(row, original, original.state());
    }
    for store in [&source, &destination] {
        for (id, body) in &bodies {
            let record = den_memory::fetch_record_by_id(store, id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(serde_json::to_value(record).unwrap(), *body);
        }
    }
}
