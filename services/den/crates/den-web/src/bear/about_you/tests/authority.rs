use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn member_owner_forms_do_not_lend_authority_to_admins_sessions_or_bears(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let id = f.stage(OWNER_NAME).await;
    let owner_page = f.get().await;
    let publish = form(
        &owner_page.body,
        &format!("publish-{id}"),
        "acknowledged=true",
    );
    for cookie in [&f.other_cookie, &f.admin_cookie] {
        let page = request(
            &f.app,
            cookie,
            "GET",
            &format!("{BASE}?user_id={}", f.owner.get()),
            "",
        )
        .await;
        assert_eq!(page.status, StatusCode::OK);
        assert!(!page.body.contains("Alice&lt;owner-only&gt;"));
        assert!(
            page.body.contains("About you</a>"),
            "ordinary members have the navigation link"
        );
        let denied = request(&f.app, cookie, "POST", &format!("{BASE}/publish"), &publish).await;
        assert_eq!(denied.status, StatusCode::BAD_REQUEST);
        assert!(!denied.body.contains("Alice&lt;owner-only&gt;"));
    }
    let another_session = cookie(&f.app, f.owner).await;
    assert_eq!(
        request(
            &f.app,
            &another_session,
            "POST",
            &format!("{BASE}/publish"),
            &publish
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&f.app, "", "GET", BASE, "").await.status,
        StatusCode::UNAUTHORIZED
    );

    let other_bear = bear(&f.pool, "anotherbear").await;
    bears_db::grant_membership(
        &f.pool,
        f.owner.get(),
        other_bear.as_uuid(),
        Some(BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    let cross_bear = request(
        &f.app,
        &f.cookie,
        "POST",
        "/bear/anotherbear/memory/about-you/publish",
        &publish,
    )
    .await;
    assert_eq!(cross_bear.status, StatusCode::BAD_REQUEST);
    let outsider = account(&f.pool, other_bear, "outsider", BEAR_ROLE_MEMBER).await;
    let outsider_cookie = cookie(&f.app, outsider).await;
    assert_eq!(
        request(&f.app, &outsider_cookie, "GET", BASE, "")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(f.inspect().await.current.is_none());

    den_http::user::email_settings::set_admin_email_verified(&f.pool, f.owner.get(), false)
        .await
        .unwrap();
    let unverified = f.submit("publish", &publish).await;
    assert_eq!(unverified.status, StatusCode::SEE_OTHER);
    assert_eq!(
        unverified.location.as_deref(),
        Some("/settings/email/verify")
    );
    den_http::user::email_settings::set_admin_email_verified(&f.pool, f.owner.get(), true)
        .await
        .unwrap();
    bears_db::revoke_membership(&f.pool, f.owner.get(), f.bear_id.as_uuid())
        .await
        .unwrap();
    let revoked = f.submit("publish", &publish).await;
    assert_eq!(revoked.status, StatusCode::NOT_FOUND);
    assert!(!revoked.body.contains("Alice&lt;owner-only&gt;"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_nonce_operation_and_authority_fields_are_rejected_without_echo(pool: PgPool) {
    let f = fixture(pool).await;
    let off = f.get().await;
    let capture = form(&off.body, "capture", "acknowledged=true");
    assert_eq!(
        f.submit("request", &capture).await.status,
        StatusCode::BAD_REQUEST
    );
    for body in [
        "acknowledged=true".to_owned(),
        "token=malformed-private-canary&acknowledged=true".to_owned(),
        format!("token={}&acknowledged=true", Uuid::new_v4()),
        format!("{capture}&expected_head={}", Uuid::new_v4()),
        format!("{capture}&user_id={}", f.other.get()),
        format!("{capture}&candidate_id={}", Uuid::new_v4()),
    ] {
        let response = f.submit("capture", &body).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert!(!response.body.contains("malformed-private-canary"));
    }
    assert!(!f.inspect().await.capture_enabled);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reconnection_aba_invalidates_every_old_owner_action(pool: PgPool) {
    use den_memory::identity_reconnection::{self as core, ReplacementConsent};
    let f = fixture(pool).await;
    let off = f.get().await;
    let stale_enable = form(&off.body, "capture", "acknowledged=true");
    f.enable().await;
    let first = f.stage("Active before ABA").await;
    f.approve(&first).await;
    let pending_id = f.stage("Pending before ABA").await;
    let page = f.get().await;
    let stale = [
        ("capture", form(&page.body, "capture", "acknowledged=true")),
        (
            "request",
            form(&page.body, "request-name", "name=must-not-stage-after-ABA"),
        ),
        (
            "review",
            form(&page.body, &format!("reject-{pending_id}"), ""),
        ),
        (
            "publish",
            form(
                &page.body,
                &format!("publish-{pending_id}"),
                "acknowledged=true",
            ),
        ),
        ("retract", form(&page.body, "retract", "acknowledged=true")),
        (
            "erase",
            form(&page.body, "erase-names", "acknowledged=true"),
        ),
    ];
    let before = f.inspect().await.current.unwrap().fact_id;
    let store = f
        .state
        .memory_stores()
        .unwrap()
        .store_for_bear(f.bear_id.as_uuid())
        .await
        .unwrap();
    let binding = den_memory::resolve_authenticated_user_entity(&store, f.owner)
        .await
        .unwrap();
    let revoke = core::review_revocation(&store, f.admin, binding.entity_id())
        .await
        .unwrap();
    core::commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let reconnect = core::review_reconnection(&store, f.admin, binding.entity_id(), f.owner)
        .await
        .unwrap();
    core::commit_review(&store, &reconnect, ReplacementConsent::NoReplacement)
        .await
        .unwrap();

    assert!(
        !f.submit("capture", &stale_enable).await.status.is_success(),
        "same tuple must not revive old capture consent"
    );
    assert!(!f.inspect().await.capture_enabled);
    // Fresh consent rules out capture=false as the explanation for later denials.
    f.enable().await;
    for (endpoint, form) in stale {
        let response = f.submit(endpoint, &form).await;
        assert!(
            !response.status.is_success(),
            "stale {endpoint} survived ABA"
        );
        assert!(!response.body.contains("Active before ABA"));
        assert!(!response.body.contains("Pending before ABA"));
    }
    let after = f.inspect().await;
    assert!(after.capture_enabled);
    assert_eq!(after.current.as_ref().unwrap().fact_id, before);
    assert_eq!(
        after.current.as_ref().unwrap().state,
        PersonalFactState::Active
    );
    assert_eq!(after.candidates.len(), 2);
    assert_eq!(after.candidates[0].state(), CandidateState::Quarantined);
}
