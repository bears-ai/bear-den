use super::*;
use den_memory::{
    personal_candidates::{
        capture_enabled, get_candidate, list_candidates, set_capture_enabled, stage_preferred_name,
        withdraw_candidate, CandidateState, PersonalCandidate, PreferredName,
        VerifiedPersonalSource,
    },
    personal_facts::{
        current_preferred_name, publish_preferred_name, retract_preferred_name, PersonalFactState,
        PublicationReceipt,
    },
    resolve_authenticated_user_entity, AuthenticatedUserEntity, MemorySource,
};

const PUBLISHED_NAME: &str = "BundlePersonalNameSentinel-7C92";

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

async fn assert_head(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    publication: PublicationReceipt,
    state: PersonalFactState,
    name: Option<&str>,
) {
    let head = current_preferred_name(store, binding)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.fact_id, publication.fact_id);
    assert_eq!(head.candidate_id, publication.candidate_id);
    assert_eq!(head.state, state);
    assert_eq!(
        head.preferred_name.as_ref().map(PreferredName::as_str),
        name
    );
}

async fn assert_generic_hidden(
    store: &BearMemoryStore,
    publication: PublicationReceipt,
    private_record: &str,
) {
    let fact_id = publication.fact_id.to_string();
    for id in [fact_id.as_str(), private_record] {
        assert!(den_memory::fetch_record_by_id(store, id)
            .await
            .unwrap()
            .is_none());
    }
    assert!(den_memory::list_memory_proposals(store, None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(den_memory::list_reviewable_memory_proposals(store)
        .await
        .unwrap()
        .is_empty());
    let indexable = den_memory::list_generic_indexable_heads(store)
        .await
        .unwrap();
    for row in indexable {
        assert_ne!(row.memory_id, fact_id);
        assert_ne!(row.memory_id, private_record);
        assert!(!row.content_text.contains(PUBLISHED_NAME));
        assert!(!row.metadata_json.contains(PUBLISHED_NAME));
    }
    let private_path = LogicalMemoryPath::shared_core("personal").to_logical_path();
    let read = den_memory::tools::sqlite_memory_read(store, &private_path)
        .await
        .unwrap();
    assert!(!read.to_string().contains(PUBLISHED_NAME));
    for role in ["pair", "work", "curate"] {
        assert!(
            den_memory::fetch_records_min(store, std::slice::from_ref(&fact_id), role)
                .await
                .unwrap()
                .is_empty()
        );
        let browse = den_memory::tools::sqlite_memory_browse(store, role)
            .await
            .unwrap();
        let search = den_memory::tools::sqlite_memory_search(store, role, PUBLISHED_NAME, 100)
            .await
            .unwrap();
        // The query is caller-supplied; only search hits are retrieved from storage.
        for output in [browse, search["hits"].clone()] {
            let serialized = output.to_string();
            assert!(!serialized.contains(PUBLISHED_NAME));
            assert!(!serialized.contains(&fact_id));
        }
    }
}

async fn assert_hidden(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    originals: &[PersonalCandidate],
) {
    assert!(current_preferred_name(store, binding)
        .await
        .unwrap()
        .is_none());
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

async fn assert_source_unchanged(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    originals: &[PersonalCandidate],
    publication: PublicationReceipt,
) {
    assert!(capture_enabled(store, binding).await.unwrap());
    assert_head(
        store,
        binding,
        publication,
        PersonalFactState::Active,
        Some(PUBLISHED_NAME),
    )
    .await;
    let rows = list_candidates(store, binding, 100).await.unwrap();
    assert_eq!(rows.len(), originals.len());
    for original in originals {
        let row = rows.iter().find(|row| row.id() == original.id()).unwrap();
        assert_preserved(row, original, original.state());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn real_bundle_import_quarantines_candidates_without_rewriting_person_or_provenance(
    pool: PgPool,
) {
    let fixture = fixture(pool).await;
    assert_ne!(fixture.actor, fixture.destination);
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
    let source_body = serde_json::to_value(
        den_memory::fetch_record_by_id(&source, &note.memory_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(
        den_memory::fetch_record_by_id(&source, &fixture.private_record)
            .await
            .unwrap()
            .is_none()
    );
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
    let candidate = stage(&source, &proof, PUBLISHED_NAME).await;
    assert_eq!(candidate.state(), CandidateState::Pending);
    assert!(current_preferred_name(&source, &current)
        .await
        .unwrap()
        .is_none());
    let publication = publish_preferred_name(&source, &proof, candidate.id(), None)
        .await
        .unwrap();
    assert_eq!(publication.candidate_id, candidate.id());
    assert_eq!(publication.previous_fact_id, None);
    let published = get_candidate(&source, &current, publication.candidate_id)
        .await
        .unwrap()
        .unwrap();
    // Published is projected from the immutable decision, not a mutable candidate status.
    assert_preserved(&published, &candidate, CandidateState::Published);
    let originals = [pending, withdrawn, quarantined, published];
    for row in &originals {
        assert_eq!(row.person_entity_id(), alice.entity_id());
        assert_eq!(row.source_bear_id(), fixture.bear_id);
        assert_eq!(row.source_user_id(), fixture.actor);
        assert_eq!(row.source_conversation_id(), conversation);
        assert_eq!(row.source_memory_id().to_string(), note.memory_id);
    }

    assert_source_unchanged(&source, &current, &originals, publication).await;
    assert_generic_hidden(&source, publication, &fixture.private_record).await;
    let bob = resolve_authenticated_user_entity(&source, UserId::new(4343))
        .await
        .unwrap();
    assert_hidden(&source, &bob, &originals).await;

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
    assert_generic_hidden(&destination, publication, &fixture.private_record).await;
    let imported_identities = core::list_identities(&destination).await.unwrap();
    let imported_identity = imported_identities
        .iter()
        .find(|row| row.entity_id == alice.entity_id())
        .unwrap();
    assert!(imported_identity.active_user_id.is_none());
    assert_eq!(imported_identity.origins, original_identity.origins);

    // Even the same authenticated account number must get a fresh destination person.
    let numeric_collision = resolve_authenticated_user_entity(&destination, fixture.actor)
        .await
        .unwrap();
    assert_ne!(numeric_collision.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&destination, &numeric_collision)
        .await
        .unwrap());
    assert_hidden(&destination, &numeric_collision, &originals).await;
    assert!(
        !withdraw_candidate(&destination, &numeric_collision, originals[0].id())
            .await
            .unwrap()
    );
    let fresh = resolve_authenticated_user_entity(&destination, fixture.destination)
        .await
        .unwrap();
    assert_ne!(fresh.entity_id(), alice.entity_id());
    assert_ne!(fresh.entity_id(), numeric_collision.entity_id());
    assert!(!capture_enabled(&destination, &fresh).await.unwrap());
    assert_hidden(&destination, &fresh, &originals).await;
    let review = core::review_reconnection(
        &destination,
        fixture.actor,
        alice.entity_id(),
        fixture.destination,
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
    assert_eq!(receipt.actor_user_id, fixture.actor);
    assert_eq!(receipt.destination_user_id, Some(fixture.destination));
    let owner = resolve_authenticated_user_entity(&destination, fixture.destination)
        .await
        .unwrap();
    assert_eq!(owner.entity_id(), alice.entity_id());
    assert!(!capture_enabled(&destination, &owner).await.unwrap());
    assert_head(
        &destination,
        &owner,
        publication,
        PersonalFactState::Active,
        Some(PUBLISHED_NAME),
    )
    .await;
    assert_generic_hidden(&destination, publication, &fixture.private_record).await;
    let destination_bob = resolve_authenticated_user_entity(&destination, UserId::new(4343))
        .await
        .unwrap();
    assert_hidden(&destination, &destination_bob, &originals).await;
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
            CandidateState::Published => CandidateState::Published,
        };
        let listed = rows.iter().find(|row| row.id() == original.id()).unwrap();
        assert_preserved(listed, original, state);
        let fetched = get_candidate(&destination, &owner, original.id())
            .await
            .unwrap()
            .unwrap();
        assert_preserved(&fetched, original, state);
    }
    assert_source_unchanged(&source, &current, &originals, publication).await;
    for store in [&source, &destination] {
        let record = den_memory::fetch_record_by_id(store, &note.memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_value(record).unwrap(), source_body);
        assert_generic_hidden(store, publication, &fixture.private_record).await;
    }

    let imported_candidate = get_candidate(&destination, &owner, publication.candidate_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!capture_enabled(&destination, &owner).await.unwrap());
    assert!(
        retract_preferred_name(&destination, &owner, publication.fact_id)
            .await
            .unwrap()
    );
    assert_head(
        &destination,
        &owner,
        publication,
        PersonalFactState::Retracted,
        None,
    )
    .await;
    // Replay needs the current destination binding, not an online original source account.
    let replay_proof = VerifiedPersonalSource::for_authorized_conversation(
        &owner,
        imported_candidate.source_conversation_id(),
        imported_candidate.source_memory_id(),
    );
    assert_eq!(
        publish_preferred_name(&destination, &replay_proof, publication.candidate_id, None)
            .await
            .unwrap(),
        publication
    );
    assert!(!capture_enabled(&destination, &owner).await.unwrap());
    assert_head(
        &destination,
        &owner,
        publication,
        PersonalFactState::Retracted,
        None,
    )
    .await;
    let replayed_candidate = get_candidate(&destination, &owner, publication.candidate_id)
        .await
        .unwrap()
        .unwrap();
    assert_preserved(
        &replayed_candidate,
        &imported_candidate,
        CandidateState::Published,
    );
    assert_source_unchanged(&source, &current, &originals, publication).await;
    assert_generic_hidden(&destination, publication, &fixture.private_record).await;

    let conversation = Uuid::new_v4();
    let note = destination
        .append_record(
            &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
            "note",
            "pair",
            None,
            "Please remember my preferred name here with fresh consent.",
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    let proof = VerifiedPersonalSource::for_authorized_conversation(
        &owner,
        conversation,
        Uuid::parse_str(&note.memory_id).unwrap(),
    );
    let name = originals[0].preferred_name().clone();
    assert!(matches!(
        stage_preferred_name(&destination, &proof, name.clone()).await,
        Err(den_core::DenError::Authorization(_))
    ));
    set_capture_enabled(&destination, &owner, true)
        .await
        .unwrap();
    let fresh = stage_preferred_name(&destination, &proof, name)
        .await
        .unwrap();
    assert!(originals.iter().all(|original| original.id() != fresh.id()));
    assert_eq!(fresh.person_entity_id(), alice.entity_id());
    assert_eq!(fresh.preferred_name(), originals[0].preferred_name());
    assert_eq!(fresh.state(), CandidateState::Pending);
    assert_eq!(fresh.source_bear_id().as_uuid(), imported.id);
    assert_eq!(fresh.source_user_id(), fixture.destination);
    assert_eq!(
        fresh.source_binding_generation(),
        receipt.resulting_generation
    );
    assert_eq!(fresh.source_conversation_id(), conversation);
    assert_eq!(fresh.source_memory_id().to_string(), note.memory_id);
    assert!(capture_enabled(&destination, &owner).await.unwrap());
    let rows = list_candidates(&destination, &owner, 100).await.unwrap();
    assert_eq!(rows.len(), originals.len() + 1);
    assert_preserved(
        rows.iter().find(|row| row.id() == fresh.id()).unwrap(),
        &fresh,
        CandidateState::Pending,
    );
    for original in &originals {
        let state = match original.state() {
            CandidateState::Pending | CandidateState::Quarantined => CandidateState::Quarantined,
            CandidateState::Withdrawn => CandidateState::Withdrawn,
            CandidateState::Published => CandidateState::Published,
        };
        assert_preserved(
            rows.iter().find(|row| row.id() == original.id()).unwrap(),
            original,
            state,
        );
    }
    let mut protected = originals.to_vec();
    protected.push(fresh);
    for user in [fixture.actor, UserId::new(4343), fixture.member] {
        let unrelated = resolve_authenticated_user_entity(&destination, user)
            .await
            .unwrap();
        if user == fixture.actor {
            assert_eq!(unrelated.entity_id(), numeric_collision.entity_id());
        }
        assert_hidden(&destination, &unrelated, &protected).await;
    }
    assert_head(
        &destination,
        &owner,
        publication,
        PersonalFactState::Retracted,
        None,
    )
    .await;
    assert_source_unchanged(&source, &current, &originals, publication).await;
    assert_generic_hidden(&source, publication, &fixture.private_record).await;
    assert_generic_hidden(&destination, publication, &fixture.private_record).await;
    assert_hidden(&source, &bob, &originals).await;
}
