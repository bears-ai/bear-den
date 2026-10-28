use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn erase_requires_a_scoped_confirmation_and_preserves_other_members_admins_and_their_tokens(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    f.enable().await;
    let own = f.stage("Owner erasure isolation sentinel").await;
    f.approve(&own).await;
    let other_page = publish_as(
        &f,
        f.other,
        &f.other_cookie,
        "Other member preserved sentinel",
    )
    .await;
    let admin_page = publish_as(&f, f.admin, &f.admin_cookie, "Admin preserved sentinel").await;
    let other_request = form(
        &other_page.body,
        "request-name",
        "name=Other-future-request",
    );
    let admin_request = form(
        &admin_page.body,
        "request-name",
        "name=Admin-future-request",
    );
    let reviewed = f.get().await;
    let erase = form(&reviewed.body, "erase-names", "acknowledged=true");
    let another_session = cookie(&f.app, f.owner).await;
    for cookie in [&f.other_cookie, &f.admin_cookie, &another_session] {
        let denied = request(&f.app, cookie, "POST", &format!("{BASE}/erase"), &erase).await;
        assert_eq!(denied.status, StatusCode::BAD_REQUEST);
        assert!(!denied.body.contains("Owner erasure isolation sentinel"));
    }
    assert_eq!(
        request(&f.app, "", "POST", &format!("{BASE}/erase"), &erase)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.submit("erase", &form(&reviewed.body, "request-name", ""))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    for forged in [
        format!("{erase}&request_id={}", Uuid::new_v4()),
        format!("{erase}&expected_head={}", Uuid::new_v4()),
        format!("{erase}&user_id={}", f.other.get()),
        format!("{erase}&name=not-an-erasure-field"),
        format!("token={}&acknowledged=true", Uuid::new_v4()),
    ] {
        assert_eq!(
            f.submit("erase", &forged).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    let other_bear = bear(&f.pool, "eraseotherbear").await;
    bears_db::grant_membership(
        &f.pool,
        f.owner.get(),
        other_bear.as_uuid(),
        Some(BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &f.app,
            &f.cookie,
            "POST",
            "/bear/eraseotherbear/memory/about-you/erase",
            &erase
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        current_name(&f.inspect().await),
        Some("Owner erasure isolation sentinel")
    );
    f.save("erase", &reviewed, "erase-names", "acknowledged=true")
        .await;
    for (actor, expected) in [
        (f.other, "Other member preserved sentinel"),
        (f.admin, "Admin preserved sentinel"),
    ] {
        let inspection = f.inspect_as(actor).await;
        assert!(inspection.capture_enabled);
        assert_eq!(
            inspection.current.as_ref().unwrap().state,
            PersonalFactState::Active
        );
        assert_eq!(current_name(&inspection), Some(expected));
        assert_eq!(
            inspection.candidates[0]
                .preferred_name()
                .map(PreferredName::as_str),
            Some(expected)
        );
    }
    for (cookie, body) in [
        (&f.other_cookie, &other_request),
        (&f.admin_cookie, &admin_request),
    ] {
        let response = request(&f.app, cookie, "POST", &format!("{BASE}/request"), body).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    }
    let inspection = f.inspect().await;
    assert!(!inspection.capture_enabled);
    assert_eq!(inspection.candidates[0].state(), CandidateState::Erased);
    assert!(inspection.candidates[0].preferred_name().is_none());
    assert_eq!(
        current_name(&f.inspect_as(f.other).await),
        Some("Other member preserved sentinel")
    );
    assert_eq!(
        current_name(&f.inspect_as(f.admin).await),
        Some("Admin preserved sentinel")
    );
}
