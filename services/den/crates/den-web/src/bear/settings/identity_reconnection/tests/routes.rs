use super::*;
use den_memory::{record_visible, resolve_authenticated_user_entity, AccessContext};

#[sqlx::test(migrations = "../../migrations")]
async fn reconnect_replace_replay_and_revoke_preserve_people_and_audiences(pool: PgPool) {
    let fixture = fixture(pool).await;
    let store = fixture.store().await;
    let previous = resolve_authenticated_user_entity(&store, fixture.destination)
        .await
        .unwrap();
    let previous_record = store
        .append_record(
            &LogicalMemoryPath::shared_core("destination"),
            "note",
            "curate",
            None,
            "destination private fact",
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    den_memory::append_relation(
        &store,
        &previous_record.memory_id,
        previous.entity_id(),
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    let stale_context = AccessContext::for_authenticated_user(&previous);
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let without_replacement = fixture.reviewed_form(&location, false).await;
    let (status, page, _) = fixture.confirm(&without_replacement).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("Separately acknowledge"));
    assert!(page.contains("No facts are lost or combined"));
    assert!(
        record_visible(&store, &previous_record.memory_id, &stale_context)
            .await
            .unwrap()
    );
    let form = fixture.reviewed_form(&location, true).await;
    let (one, two) = tokio::join!(fixture.confirm(&form), fixture.confirm(&form));
    assert_eq!(
        usize::from(one.0 == StatusCode::OK) + usize::from(two.0 == StatusCode::OK),
        1
    );
    assert!(if one.0 == StatusCode::OK {
        &one.1
    } else {
        &two.1
    }
    .contains("previous identity is parked"));
    assert!(!fixture.confirm(&form).await.0.is_success());
    let connected = resolve_authenticated_user_entity(&store, fixture.destination)
        .await
        .unwrap();
    assert_eq!(connected.entity_id(), fixture.alice);
    assert!(record_visible(
        &store,
        &fixture.private_record,
        &AccessContext::for_authenticated_user(&connected)
    )
    .await
    .unwrap());
    assert!(
        !record_visible(&store, &previous_record.memory_id, &stale_context)
            .await
            .unwrap()
    );
    let identities = core::list_identities(&store).await.unwrap();
    assert!(identities
        .iter()
        .any(|row| row.entity_id == fixture.bob && row.active_user_id.is_none()));
    assert!(identities
        .iter()
        .any(|row| row.entity_id == previous.entity_id() && row.active_user_id.is_none()));
    let (records, rules, _) = den_memory::test_support::identity_test_counts(&store)
        .await
        .unwrap();
    assert_eq!(records, 2);
    assert_eq!(rules, 2);
    let revocation = fixture.prepare(&fixture.alice, None).await;
    let form = fixture.reviewed_form(&revocation, false).await;
    let (status, page, _) = fixture.confirm(&form).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Mapping revoked"));
    assert!(!record_visible(
        &store,
        &fixture.private_record,
        &AccessContext::for_authenticated_user(&connected)
    )
    .await
    .unwrap());
    // A parked destination identity can be explicitly reconnected, without merging.
    let location = fixture
        .prepare(previous.entity_id(), Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::OK);
    let current = resolve_authenticated_user_entity(&store, fixture.destination)
        .await
        .unwrap();
    assert_eq!(current.entity_id(), previous.entity_id());
    assert!(record_visible(
        &store,
        &previous_record.memory_id,
        &AccessContext::for_authenticated_user(&current)
    )
    .await
    .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_admin_operator_other_session_and_cross_bear_inputs_cannot_authorize(pool: PgPool) {
    let fixture = fixture(pool).await;
    let member_cookie = cookie(&fixture.app, fixture.member).await;
    let (status, _, _) = request(&fixture.app, &member_cookie, "GET", LIST, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    sqlx::query!(
        "UPDATE users SET is_admin = $1 WHERE id = $2",
        true,
        fixture.member.get()
    )
    .execute(&fixture.pool)
    .await
    .unwrap();
    let operator_cookie = cookie(&fixture.app, fixture.member).await;
    assert_eq!(
        request(&fixture.app, &operator_cookie, "GET", LIST, "")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&fixture.app, "", "GET", LIST, "").await.0,
        StatusCode::UNAUTHORIZED
    );
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    let another_session = cookie(&fixture.app, fixture.actor).await;
    for cookie in [&member_cookie, &operator_cookie, &another_session] {
        assert!(!request(&fixture.app, cookie, "GET", &location, "")
            .await
            .0
            .is_success());
        assert!(!request(
            &fixture.app,
            cookie,
            "POST",
            &format!("{LIST}/confirm"),
            &form
        )
        .await
        .0
        .is_success());
    }
    let (status, page, _) = request(&fixture.app, &fixture.cookie, "GET", LIST, "").await;
    assert_eq!(status, StatusCode::OK);
    let foreign_id = Uuid::new_v4();
    let forged = format!(
        "prepare_token={}&action=reconnect&entity_id={foreign_id}&destination_user_id={}",
        input(&page, "prepare_token"),
        fixture.destination
    );
    assert!(!request(
        &fixture.app,
        &fixture.cookie,
        "POST",
        &format!("{LIST}/prepare"),
        &forged
    )
    .await
    .0
    .is_success());
}

#[sqlx::test(migrations = "../../migrations")]
async fn csrf_checkbox_and_fresh_eligibility_are_required_at_confirmation(pool: PgPool) {
    let fixture = fixture(pool).await;
    let forged = format!(
        "prepare_token={}&action=reconnect&entity_id={}&destination_user_id={}",
        Uuid::new_v4(),
        fixture.alice,
        fixture.destination
    );
    assert_eq!(
        request(
            &fixture.app,
            &fixture.cookie,
            "POST",
            &format!("{LIST}/prepare"),
            &forged
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let form = fixture.reviewed_form(&location, false).await;
    assert_eq!(
        fixture
            .confirm(&form.replace("confirm_audience=true", "confirm_audience=false"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, page, _) = request(&fixture.app, &fixture.cookie, "GET", &location, "").await;
    let forged_confirmation = form.replace(
        &input(&page, "confirmation_token"),
        &Uuid::new_v4().to_string(),
    );
    assert_eq!(
        fixture.confirm(&forged_confirmation).await.0,
        StatusCode::BAD_REQUEST
    );
    den_http::user::email_settings::set_admin_email_verified(
        &fixture.pool,
        fixture.destination.get(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::FORBIDDEN);
    den_http::user::email_settings::set_admin_email_verified(
        &fixture.pool,
        fixture.destination.get(),
        true,
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
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::FORBIDDEN);
    bears_db::grant_membership(
        &fixture.pool,
        fixture.destination.get(),
        fixture.bear_id.as_uuid(),
        Some(BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    bears_db::grant_membership(
        &fixture.pool,
        fixture.member.get(),
        fixture.bear_id.as_uuid(),
        Some(BEAR_ROLE_ADMIN),
    )
    .await
    .unwrap();
    bears_db::grant_membership(
        &fixture.pool,
        fixture.actor.get(),
        fixture.bear_id.as_uuid(),
        Some(BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::FORBIDDEN);
    assert!(core::list_identities(&fixture.store().await)
        .await
        .unwrap()
        .iter()
        .all(|row| row.active_user_id.is_none()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn stale_core_review_and_replacement_session_pointer_cannot_commit(pool: PgPool) {
    let fixture = fixture(pool).await;
    let first = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    let old_form = fixture.reviewed_form(&first, false).await;
    let second = fixture
        .prepare(&fixture.bob, Some(fixture.destination))
        .await;
    assert_eq!(fixture.confirm(&old_form).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        request(&fixture.app, &fixture.cookie, "GET", &first, "")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let form = fixture.reviewed_form(&second, false).await;
    // A destination binding created after review invalidates that exact core snapshot.
    den_memory::resolve_authenticated_user_entity(&fixture.store().await, fixture.destination)
        .await
        .unwrap();
    assert!(!fixture.confirm(&form).await.0.is_success());
    assert!(core::list_identities(&fixture.store().await)
        .await
        .unwrap()
        .iter()
        .any(|row| row.entity_id == fixture.bob && row.active_user_id.is_none()));
}
