use super::*;

async fn rename(fixture: &Fixture, user_id: UserId, username: &str, display_name: &str) {
    let user = den_http::user::db::get_user_by_id(&fixture.pool, user_id.get())
        .await
        .unwrap()
        .unwrap();
    den_http::user::db::update_user_by_id(
        &fixture.pool,
        user_id.get(),
        &user.email,
        username,
        display_name,
        &user.theme,
        1,
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn changed_account_labels_require_fresh_review_on_get_and_commit(pool: PgPool) {
    let fixture = fixture(pool).await;
    for (user, name) in [
        (fixture.destination, "renameddestination"),
        (fixture.actor, "renamedadmin"),
    ] {
        let location = fixture
            .prepare(&fixture.alice, Some(fixture.destination))
            .await;
        let form = fixture.reviewed_form(&location, false).await;
        rename(&fixture, user, name, "Changed label <script>").await;
        let (status, page, _) = request(&fixture.app, &fixture.cookie, "GET", &location, "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert!(page.contains("Identity metadata or account labels"));
        let (status, page, _) = fixture.confirm(&form).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert!(page.contains("Identity metadata or account labels"));
        assert!(page.contains("versions, expiry, or replay state"));
        assert!(page.contains("This request did not authorize a new change. Review current mappings and recent receipts before preparing another review."));
        assert!(page.contains("href=\"/bear/identityreview/portability/identities\""));
        assert!(core::list_identities(&fixture.store().await)
            .await
            .unwrap()
            .iter()
            .all(|identity| identity.active_user_id.is_none()));
    }
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    let (status, receipt, _) = fixture.confirm(&form).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(receipt.contains("renameddestination"));
    assert!(receipt.contains("renamedadmin"));
    assert!(receipt.contains("Changed label &lt;script&gt;"));
    assert!(receipt.contains(&fixture.alice));
    assert!(receipt.contains("Recorded at"));
    assert!(receipt.contains("result of the completed decision"));
    assert_eq!(
        request(&fixture.app, &fixture.cookie, "GET", &location, "")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn changed_displaced_provenance_invalidates_the_review_without_combining_facts(pool: PgPool) {
    let fixture = fixture(pool).await;
    let store = fixture.store().await;
    let displaced = den_memory::resolve_authenticated_user_entity(&store, fixture.destination)
        .await
        .unwrap();
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, true).await;
    core::record_export_origins(&store, "https://new-origin.example", fixture.actor, &[])
        .await
        .unwrap();
    assert_eq!(
        request(&fixture.app, &fixture.cookie, "GET", &location, "")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        den_memory::resolve_authenticated_user_entity(&store, fixture.destination)
            .await
            .unwrap()
            .entity_id(),
        displaced.entity_id()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn selected_epoch_change_cannot_be_presented_as_the_original_review(pool: PgPool) {
    let fixture = fixture(pool).await;
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    let store = fixture.store().await;
    let elsewhere =
        core::review_reconnection(&store, fixture.actor, &fixture.alice, fixture.destination)
            .await
            .unwrap();
    core::commit_review(&store, &elsewhere, core::ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let revoke = core::review_revocation(&store, fixture.actor, &fixture.alice)
        .await
        .unwrap();
    core::commit_review(&store, &revoke, core::ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    // Same parked tuple as the old page, but its canonical generation is newer.
    assert_eq!(
        request(&fixture.app, &fixture.cookie, "GET", &location, "")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn revocation_requires_current_actor_authority_not_a_still_eligible_old_target(pool: PgPool) {
    let fixture = fixture(pool).await;
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::OK);
    let location = fixture.prepare(&fixture.alice, None).await;
    let form = fixture.reviewed_form(&location, false).await;
    den_http::user::email_settings::set_admin_email_verified(
        &fixture.pool,
        fixture.actor.get(),
        false,
    )
    .await
    .unwrap();
    assert!(!fixture.confirm(&form).await.0.is_success());
    den_http::user::email_settings::set_admin_email_verified(
        &fixture.pool,
        fixture.actor.get(),
        true,
    )
    .await
    .unwrap();
    den_http::user::email_settings::set_admin_email_verified(
        &fixture.pool,
        fixture.destination.get(),
        false,
    )
    .await
    .unwrap();
    bears_db::revoke_membership(
        &fixture.pool,
        fixture.destination.get(),
        fixture.bear_id.as_uuid(),
    )
    .await
    .unwrap();
    let (status, receipt, _) = fixture.confirm(&form).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(receipt.contains("Mapping revoked; no active destination was attached"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn full_bundle_import_rewrites_generation_ownership_not_historical_origins(pool: PgPool) {
    use den_memory::{
        record_visible, resolve_authenticated_user_entity, scoped, AccessContext, MemorySource,
    };

    async fn personal_record(store: &BearMemoryStore, person: &str, kind: &str) -> String {
        let record = store
            .append_record(
                &LogicalMemoryPath::shared_core(kind),
                "note",
                "curate",
                None,
                kind,
                &json!({}),
                "normal",
            )
            .await
            .unwrap();
        den_memory::append_relation(
            store,
            &record.memory_id,
            person,
            "audience",
            &json!({}),
            "curate",
            None,
            None,
        )
        .await
        .unwrap();
        record.memory_id
    }

    let fixture = fixture(pool).await;
    let source = fixture.store().await;
    assert_ne!(fixture.actor, UserId::new(4242));
    // The shared fixture starts parked. Establish actual source bindings and a
    // valid cached source context before exporting the original portable people.
    for (person, user) in [
        (&fixture.alice, UserId::new(4242)),
        (&fixture.bob, UserId::new(4343)),
    ] {
        let review = core::review_reconnection(&source, fixture.actor, person, user)
            .await
            .unwrap();
        core::commit_review(&source, &review, core::ReplacementConsent::NoReplacement)
            .await
            .unwrap();
    }
    let source_alice = resolve_authenticated_user_entity(&source, UserId::new(4242))
        .await
        .unwrap();
    assert_eq!(source_alice.entity_id(), fixture.alice);
    let source_context = AccessContext::for_authenticated_user(&source_alice);
    assert!(
        record_visible(&source, &fixture.private_record, &source_context)
            .await
            .unwrap()
    );
    let bob_memory = personal_record(&source, &fixture.bob, "bob-private-roundtrip").await;
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
    // Export records additional provenance; compare against the actual source snapshot.
    let before = core::list_identities(&source).await.unwrap();
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
    let store = fixture
        .state
        .memory_stores()
        .unwrap()
        .store_for_bear(imported.id)
        .await
        .unwrap();
    let after = core::list_identities(&store).await.unwrap();
    for person in [&fixture.alice, &fixture.bob] {
        let original = before
            .iter()
            .find(|identity| identity.entity_id == *person)
            .unwrap();
        let restored = after
            .iter()
            .find(|identity| identity.entity_id == *person)
            .unwrap();
        assert_eq!(restored.binding_version, original.binding_version + 1);
        assert!(restored.active_user_id.is_none());
        assert_eq!(
            serde_json::to_value(&restored.origins).unwrap(),
            serde_json::to_value(&original.origins).unwrap()
        );
        assert!(restored
            .origins
            .iter()
            .any(|origin| origin.source_host == "https://source.example"));
    }
    let (_, _, foreign) = den_memory::test_support::identity_test_counts(&store)
        .await
        .unwrap();
    assert_eq!(foreign, 0);
    // Matching host-local numbers allocate different people, never imported authority.
    let mut numeric_contexts = Vec::new();
    for user in [UserId::new(4242), UserId::new(4343)] {
        let local = resolve_authenticated_user_entity(&store, user)
            .await
            .unwrap();
        assert_ne!(local.entity_id(), fixture.alice);
        assert_ne!(local.entity_id(), fixture.bob);
        let context = AccessContext::for_authenticated_user(&local)
            .with_audience([fixture.alice.clone(), fixture.bob.clone()]);
        for memory_id in [&fixture.private_record, &bob_memory] {
            assert!(!record_visible(&store, memory_id, &context).await.unwrap());
        }
        numeric_contexts.push(context);
    }
    let displaced = resolve_authenticated_user_entity(&store, fixture.actor)
        .await
        .unwrap();
    assert_ne!(displaced.entity_id(), fixture.alice);
    assert_ne!(displaced.entity_id(), fixture.bob);
    let displaced_context = AccessContext::for_authenticated_user(&displaced);
    let local_memory = personal_record(
        &store,
        displaced.entity_id(),
        "destination-private-roundtrip",
    )
    .await;
    assert!(record_visible(&store, &local_memory, &displaced_context)
        .await
        .unwrap());

    let identities_path = format!("/bear/{slug}/portability/identities");
    let (status, page, _) =
        request(&fixture.app, &fixture.cookie, "GET", &identities_path, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let prepare_form = format!(
        "prepare_token={}&entity_id={}&action=reconnect&destination_user_id={}",
        input(&page, "prepare_token"),
        fixture.alice,
        fixture.actor
    );
    let (status, page, location) = request(
        &fixture.app,
        &fixture.cookie,
        "POST",
        &format!("{identities_path}/prepare"),
        &prepare_form,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{page}");
    let (status, page, _) =
        request(&fixture.app, &fixture.cookie, "GET", &location.unwrap(), "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains(displaced.entity_id()));
    assert!(page.contains("name=\"confirm_replacement\""));
    let confirm_form = format!(
        "nonce={}&confirmation_token={}&confirm_audience=true&confirm_replacement=true",
        input(&page, "nonce"),
        input(&page, "confirmation_token")
    );
    let (status, receipt, _) = request(
        &fixture.app,
        &fixture.cookie,
        "POST",
        &format!("{identities_path}/confirm"),
        &confirm_form,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(receipt.contains("previous identity is parked"));
    let connected = resolve_authenticated_user_entity(&store, fixture.actor)
        .await
        .unwrap();
    assert_eq!(connected.entity_id(), fixture.alice);
    let connected_context = AccessContext::for_authenticated_user(&connected);
    let identities = core::list_identities(&store).await.unwrap();
    assert!(identities
        .iter()
        .any(|identity| identity.entity_id == fixture.bob && identity.active_user_id.is_none()));
    assert!(identities
        .iter()
        .any(|identity| identity.entity_id == displaced.entity_id()
            && identity.active_user_id.is_none()));
    let parked = den_memory::get_entity(&store, displaced.entity_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.resolution, den_memory::ResolutionState::Provisional);
    assert!(parked.superseded_by_entity_id.is_none());
    let expected_contents = [
        (
            fixture.private_record.as_str(),
            "private-memory-canary-never-render",
        ),
        (bob_memory.as_str(), "bob-private-roundtrip"),
        (local_memory.as_str(), "destination-private-roundtrip"),
    ];
    assert!(
        den_memory::test_support::canonical_record_contents_match(&store, &expected_contents)
            .await
            .unwrap(),
        "import and reconnection must preserve every private body"
    );
    for (memory_id, person) in [
        (fixture.private_record.as_str(), fixture.alice.as_str()),
        (bob_memory.as_str(), fixture.bob.as_str()),
        (local_memory.as_str(), displaced.entity_id()),
    ] {
        // Administrative inspection is not an audience override, even after reconnecting Alice.
        assert!(den_memory::get_memory_record_by_id(
            fixture.state.memory_stores().unwrap(),
            imported.id,
            memory_id,
        )
        .await
        .unwrap()
        .is_none());
        let rules = den_memory::list_access_rules_for_source(&store, memory_id)
            .await
            .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].src_memory_id, memory_id);
        assert_eq!(rules[0].entity_id, person);
    }
    for context in &numeric_contexts {
        for memory_id in [&fixture.private_record, &bob_memory] {
            assert!(!record_visible(&store, memory_id, context).await.unwrap());
        }
    }
    let grant = scoped::MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), None);
    for (kind, context, expected) in [
        (
            "personal",
            &connected_context,
            Some(fixture.private_record.as_str()),
        ),
        ("bob-private-roundtrip", &connected_context, None),
        ("destination-private-roundtrip", &connected_context, None),
        ("destination-private-roundtrip", &displaced_context, None),
        ("personal", &source_context, None),
    ] {
        let recalled = scoped::read_path(
            &store,
            grant,
            context,
            &LogicalMemoryPath::shared_core(kind).to_logical_path(),
            10,
        )
        .await
        .unwrap();
        if let Some(memory_id) = expected {
            assert!(recalled.iter().any(|record| record.memory_id == memory_id));
        } else {
            assert!(recalled.is_empty(), "{kind} must remain audience-gated");
        }
    }
    assert!(
        record_visible(&source, &fixture.private_record, &source_context)
            .await
            .unwrap()
    );
}
