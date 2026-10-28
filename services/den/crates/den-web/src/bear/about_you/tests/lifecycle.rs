use super::*;

fn name(inspection: &PersonalMemoryInspection) -> Option<&str> {
    inspection
        .current
        .as_ref()
        .and_then(|head| head.preferred_name.as_ref())
        .map(PreferredName::as_str)
}

#[sqlx::test(migrations = "../../migrations")]
async fn member_consent_request_approval_correction_retraction_and_optout(pool: PgPool) {
    let f = fixture(pool).await;
    let off = f.get().await;
    assert!(!f.inspect().await.capture_enabled);
    assert!(!off.body.contains("id=\"request-name\""));
    assert!(!f
        .submit("capture", &form(&off.body, "capture", ""))
        .await
        .status
        .is_success());
    assert!(!f.inspect().await.capture_enabled);
    f.enable().await;

    let page = f.get().await;
    let request = form(
        &page.body,
        "request-name",
        &format!("name={}", urlencoding::encode(OWNER_NAME)),
    );
    assert_eq!(
        f.submit("request", &request).await.status,
        StatusCode::SEE_OTHER
    );
    let first = f.inspect().await.candidates[0].id();
    assert!(
        f.inspect().await.current.is_none(),
        "staging must not publish"
    );
    assert_eq!(
        f.submit("request", &request).await.status,
        StatusCode::SEE_OTHER
    );
    let inspected = f.inspect().await;
    assert_eq!(
        inspected.candidates.len(),
        1,
        "same form keeps its request UUID on retries"
    );
    assert_eq!(inspected.candidates[0].id(), first);

    let pending = f.get().await;
    let publish = form(
        &pending.body,
        &format!("publish-{first}"),
        "acknowledged=true",
    );
    assert!(!f
        .submit(
            "publish",
            &publish.replace("acknowledged=true", "acknowledged=false")
        )
        .await
        .status
        .is_success());
    assert!(f.inspect().await.current.is_none());
    assert_eq!(
        f.submit("publish", &publish).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(name(&f.inspect().await), Some(OWNER_NAME));

    let active = f.get().await;
    let stale_retract = form(&active.body, "retract", "acknowledged=true");
    let second = f.stage("Alicia corrected").await;
    assert_eq!(name(&f.inspect().await), Some(OWNER_NAME));
    f.approve(&second).await;
    assert_eq!(name(&f.inspect().await), Some("Alicia corrected"));
    assert!(!f
        .submit("retract", &stale_retract)
        .await
        .status
        .is_success());
    assert_eq!(
        f.submit("publish", &publish).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        name(&f.inspect().await),
        Some("Alicia corrected"),
        "historical approval is not resurrection"
    );

    let current = f.get().await;
    f.save("retract", &current, "retract", "acknowledged=true")
        .await;
    let retracted = f.inspect().await;
    assert_eq!(
        retracted.current.as_ref().unwrap().state,
        PersonalFactState::Retracted
    );
    assert!(name(&retracted).is_none());
    assert!(f
        .get()
        .await
        .body
        .contains("Retraction stops use of that version; it is not deletion"));

    let third = f.stage("Fresh after retraction").await;
    f.approve(&third).await;
    f.stage("Pending before optout").await;
    let active = f.get().await;
    f.save("capture", &active, "capture", "acknowledged=true")
        .await;
    let disabled = f.inspect().await;
    assert!(!disabled.capture_enabled);
    assert_eq!(
        disabled.current.as_ref().unwrap().state,
        PersonalFactState::Retracted
    );
    assert_eq!(disabled.candidates[0].state(), CandidateState::Withdrawn);
    f.enable().await;
    let enabled = f.inspect().await;
    assert_eq!(
        enabled.current.as_ref().unwrap().state,
        PersonalFactState::Retracted
    );
    assert_eq!(enabled.candidates[0].state(), CandidateState::Withdrawn);
    assert!(name(&enabled).is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn review_decisions_use_the_server_held_review_version(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let id = f.stage("Review me").await;
    let pending = f.get().await;
    let stale_reject = form(&pending.body, &format!("reject-{id}"), "");
    f.save("review", &pending, &format!("defer-{id}"), "").await;
    assert_eq!(
        f.inspect().await.candidates[0].state(),
        CandidateState::Deferred
    );
    assert!(!f.submit("review", &stale_reject).await.status.is_success());
    let deferred = f.get().await;
    assert!(deferred.body.contains(&format!("id=\"publish-{id}\"")));
    f.save("review", &deferred, &format!("resume-{id}"), "")
        .await;
    assert_eq!(
        f.inspect().await.candidates[0].state(),
        CandidateState::Pending
    );
    let pending = f.get().await;
    f.save("review", &pending, &format!("reject-{id}"), "")
        .await;
    assert_eq!(
        f.inspect().await.candidates[0].state(),
        CandidateState::Rejected
    );
    let id = f.stage("Close me").await;
    let pending = f.get().await;
    f.save("review", &pending, &format!("expire-{id}"), "")
        .await;
    assert_eq!(
        f.inspect().await.candidates[0].state(),
        CandidateState::Expired
    );
    assert!(f.inspect().await.current.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn validation_and_rendering_are_escaped_and_keep_private_headers(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let page = f.get().await;
    for value in ["", "bad\nname", &"é".repeat(65)] {
        let response = f
            .submit(
                "request",
                &form(
                    &page.body,
                    "request-name",
                    &format!("name={}", urlencoding::encode(value)),
                ),
            )
            .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert!(response.body.contains("1–128 UTF-8 bytes"));
    }
    assert!(f.inspect().await.candidates.is_empty());
    let id = f.stage(OWNER_NAME).await;
    f.approve(&id).await;
    let page = f.get().await;
    assert!(page.body.contains("Alice&lt;owner-only&gt;"));
    assert!(!page.body.contains(OWNER_NAME));
    assert_eq!(page.body.matches("class=\"bear-manage-nav\"").count(), 1);
    assert!(page.body.contains("/assets/css/style.css"));
    assert!(page
        .body
        .contains("Showing up to the 100 most recent requests"));
    assert!(page.body.contains("Explicit erasure is not available here"));
    assert!(!page.body.contains("name=\"expected"));
    assert!(!page.body.contains("name=\"candidate_id\""));
    let response = f
        .submit(
            "request",
            &format!("token={}&name=private-error-canary", Uuid::new_v4()),
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(!response.body.contains("private-error-canary"));
    assert!(!response.body.contains("Alice&lt;owner-only&gt;"));
    assert!(response.body.contains("<h2>Check the current state</h2>"));
    crate::admin::usability_tests::assert_link(&response.body, BASE, "Refresh About you");
}

#[test]
fn content_headings_and_error_recovery_have_clean_markup() {
    use crate::admin::usability_tests::{assert_link, render};
    let common = minijinja::context! {
        bear => minijinja::context! { name => "About you Bear", slug => "aboutyou" },
        can_manage_bear => false,
        capture_enabled => true,
        capture_token => Uuid::new_v4().to_string(),
        request_token => Uuid::new_v4().to_string(),
        candidates => Vec::<minijinja::Value>::new(),
    };
    let page = render("bear/memory/about_you.html", common.clone());
    for heading in [
        "<h2>About you</h2>",
        "<h3>Preferred-name capture</h3>",
        "<h3>Current preferred name</h3>",
        "<h3>Request a preferred name</h3>",
        "<h3>Recent preferred-name requests</h3>",
    ] {
        assert!(page.contains(heading), "missing complete heading {heading}");
    }
    assert!(page.contains("Capture is <strong>on</strong>"));
    assert!(!page.contains("resume review before approval"));
    let error = render(
        "bear/memory/about_you.html",
        minijinja::context! {
            error => "Safe recovery <escaped>", ..common,
        },
    );
    assert!(error.contains("<h2>Check the current state</h2>"));
    assert!(error.contains("Safe recovery &lt;escaped&gt;"));
    assert_link(&error, BASE, "Refresh About you");
    assert!(!error.contains("id=\"capture\""));
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_and_deferred_candidates_both_allow_explicit_approval(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let id = f.stage("Approval prepared before deferral").await;
    let pending = f.get().await;
    let approval = form(&pending.body, &format!("publish-{id}"), "acknowledged=true");
    f.save("review", &pending, &format!("defer-{id}"), "").await;
    let deferred = f.get().await;
    assert!(deferred.body.contains(&format!("id=\"publish-{id}\"")));
    // Deferral does not change the immutable value or the expected fact head.
    assert_eq!(
        f.submit("publish", &approval).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        name(&f.inspect().await),
        Some("Approval prepared before deferral")
    );

    let id = f.stage("Approval prepared while deferred").await;
    let pending = f.get().await;
    f.save("review", &pending, &format!("defer-{id}"), "").await;
    let deferred = f.get().await;
    f.save(
        "publish",
        &deferred,
        &format!("publish-{id}"),
        "acknowledged=true",
    )
    .await;
    assert_eq!(
        name(&f.inspect().await),
        Some("Approval prepared while deferred")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn old_enable_form_cannot_reenable_after_capture_is_disabled(pool: PgPool) {
    let f = fixture(pool).await;
    let off = f.get().await;
    let enable = form(&off.body, "capture", "acknowledged=true");
    assert_eq!(
        f.submit("capture", &enable).await.status,
        StatusCode::SEE_OTHER
    );
    let on = f.get().await;
    f.save("capture", &on, "capture", "acknowledged=true").await;
    let response = f.submit("capture", &enable).await;
    assert!(matches!(
        response.status,
        StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
    ));
    assert!(
        !response
            .body
            .contains(super::super::session_state::UNAVAILABLE),
        "the retained token must reach the service's capture-revision check"
    );
    assert!(!f.inspect().await.capture_enabled);
    crate::admin::usability_tests::assert_link(&response.body, BASE, "Refresh About you");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unused_request_form_cannot_cross_capture_off_and_back_on(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let on = f.get().await;
    let unused = form(&on.body, "request-name", "name=unused-before-optout");
    f.save("capture", &on, "capture", "acknowledged=true").await;
    f.enable().await;
    let response = f.submit("request", &unused).await;
    assert!(matches!(
        response.status,
        StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
    ));
    assert!(
        !response
            .body
            .contains(super::super::session_state::UNAVAILABLE),
        "the retained token must reach the service's capture-revision check"
    );
    let inspection = f.inspect().await;
    assert!(inspection.capture_enabled);
    assert!(inspection.candidates.is_empty());
    assert!(!response.body.contains("unused-before-optout"));
    f.stage("Fresh request after optout").await;
    assert_eq!(f.inspect().await.candidates.len(), 1);
}
