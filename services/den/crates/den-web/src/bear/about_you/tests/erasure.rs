use super::*;

#[path = "erasure_authority.rs"]
mod authority;
#[path = "erasure_templates.rs"]
mod templates;

fn assert_stale(response: &Page) {
    assert!(
        matches!(
            response.status,
            StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
        ),
        "{}",
        response.body
    );
    assert!(
        !response
            .body
            .contains(super::super::session_state::UNAVAILABLE),
        "the retained token must reach the service's reviewed-owner checks"
    );
    crate::admin::usability_tests::assert_link(&response.body, BASE, "Refresh About you");
}

fn current_name(inspection: &PersonalMemoryInspection) -> Option<&str> {
    inspection
        .current
        .as_ref()
        .and_then(|head| head.preferred_name.as_ref())
        .map(PreferredName::as_str)
}

async fn publish_as(f: &Fixture, actor: UserId, cookie: &str, name: &str) -> Page {
    let off = request(&f.app, cookie, "GET", BASE, "").await;
    assert_eq!(off.status, StatusCode::OK);
    let enabled = request(
        &f.app,
        cookie,
        "POST",
        &format!("{BASE}/capture"),
        &form(&off.body, "capture", "acknowledged=true"),
    )
    .await;
    assert_eq!(enabled.status, StatusCode::SEE_OTHER);
    let on = request(&f.app, cookie, "GET", BASE, "").await;
    let staged = request(
        &f.app,
        cookie,
        "POST",
        &format!("{BASE}/request"),
        &form(
            &on.body,
            "request-name",
            &format!("name={}", urlencoding::encode(name)),
        ),
    )
    .await;
    assert_eq!(staged.status, StatusCode::SEE_OTHER);
    let id = f.inspect_as(actor).await.candidates[0].id();
    let pending = request(&f.app, cookie, "GET", BASE, "").await;
    let approved = request(
        &f.app,
        cookie,
        "POST",
        &format!("{BASE}/publish"),
        &form(&pending.body, &format!("publish-{id}"), "acknowledged=true"),
    )
    .await;
    assert_eq!(approved.status, StatusCode::SEE_OTHER);
    let active = request(&f.app, cookie, "GET", BASE, "").await;
    assert_eq!(active.status, StatusCode::OK);
    active
}

#[sqlx::test(migrations = "../../migrations")]
async fn erasure_removes_old_and_pending_bodies_and_retry_does_not_erase_future_names(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    f.enable().await;
    let first = f.stage("Erasure first version sentinel").await;
    f.approve(&first).await;
    let correction = f.stage("Erasure corrected version sentinel").await;
    f.approve(&correction).await;
    let pending = f.stage("Erasure pending version sentinel").await;
    let page = f.get().await;
    let erase = form(&page.body, "erase-names", "acknowledged=true");
    let old_approval = form(
        &page.body,
        &format!("publish-{pending}"),
        "acknowledged=true",
    );
    let old_review = form(&page.body, &format!("reject-{pending}"), "");
    let before = f.inspect().await;
    let fact_id = before.current.as_ref().unwrap().fact_id;
    let ids: Vec<_> = before
        .candidates
        .iter()
        .map(|candidate| candidate.id())
        .collect();
    assert!(!f
        .submit(
            "erase",
            &erase.replace("acknowledged=true", "acknowledged=false")
        )
        .await
        .status
        .is_success());
    assert!(f
        .inspect()
        .await
        .candidates
        .iter()
        .all(|candidate| candidate.preferred_name().is_some()));
    assert_eq!(
        f.submit("erase", &erase).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        f.submit("erase", &erase).await.status,
        StatusCode::SEE_OTHER
    );
    let erased = f.inspect().await;
    assert!(!erased.capture_enabled);
    assert_eq!(erased.current.as_ref().unwrap().fact_id, fact_id);
    assert_eq!(
        erased.current.as_ref().unwrap().state,
        PersonalFactState::Retracted
    );
    assert!(current_name(&erased).is_none());
    assert_eq!(erased.candidates.len(), 3);
    for candidate in &erased.candidates {
        assert!(ids.contains(&candidate.id()));
        assert_eq!(candidate.state(), CandidateState::Erased);
        assert!(candidate.preferred_name().is_none());
    }
    assert_stale(&f.submit("publish", &old_approval).await);
    assert_stale(&f.submit("review", &old_review).await);
    let after = f.get().await;
    assert_eq!(
        after
            .body
            .matches("Erased preferred-name request</h4>")
            .count(),
        3
    );
    for sentinel in [
        "Erasure first version sentinel",
        "Erasure corrected version sentinel",
        "Erasure pending version sentinel",
    ] {
        assert!(!after.body.contains(sentinel));
    }
    for id in [&first, &correction, &pending] {
        for action in ["publish", "reject", "expire", "defer", "resume"] {
            assert!(!after.body.contains(&format!("id=\"{action}-{id}\"")));
        }
    }
    assert!(!after.body.contains("id=\"request-name\""));
    assert!(after
        .body
        .contains("Original human conversation history is separate"));
    // Reuse the displayed enable form so the original erase page stays among
    // the four retained pages: this retry must reach durable service idempotence.
    f.save("capture", &after, "capture", "acknowledged=true")
        .await;
    let future = f.stage("Future name survives an erasure retry").await;
    f.approve(&future).await;
    assert_eq!(
        f.submit("erase", &erase).await.status,
        StatusCode::SEE_OTHER
    );
    let latest = f.inspect().await;
    assert!(latest.capture_enabled);
    assert_eq!(
        current_name(&latest),
        Some("Future name survives an erasure retry")
    );
    assert_eq!(latest.candidates[0].state(), CandidateState::Published);
    assert_eq!(
        latest.candidates[0]
            .preferred_name()
            .map(PreferredName::as_str),
        Some("Future name survives an erasure retry")
    );
    assert_eq!(
        latest
            .candidates
            .iter()
            .filter(|candidate| candidate.state() == CandidateState::Erased)
            .count(),
        3
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn erasure_rejects_a_changed_fact_head(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    let first = f.stage("Before erasure review").await;
    f.approve(&first).await;
    let correction = f.stage("Head changed after erasure review").await;
    let reviewed = f.get().await;
    let erase = form(&reviewed.body, "erase-names", "acknowledged=true");
    f.save(
        "publish",
        &reviewed,
        &format!("publish-{correction}"),
        "acknowledged=true",
    )
    .await;
    assert_stale(&f.submit("erase", &erase).await);
    let inspection = f.inspect().await;
    assert_eq!(
        current_name(&inspection),
        Some("Head changed after erasure review")
    );
    assert!(inspection.capture_enabled);
    assert!(inspection
        .candidates
        .iter()
        .all(|candidate| candidate.preferred_name().is_some()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn erasure_rejects_a_new_candidate_since_review(pool: PgPool) {
    let f = fixture(pool).await;
    f.enable().await;
    f.stage("Candidate already reviewed").await;
    let reviewed = f.get().await;
    let erase = form(&reviewed.body, "erase-names", "acknowledged=true");
    f.stage("Candidate added after review").await;
    assert_stale(&f.submit("erase", &erase).await);
    let inspection = f.inspect().await;
    assert!(inspection.capture_enabled);
    assert_eq!(inspection.candidates.len(), 2);
    assert!(inspection
        .candidates
        .iter()
        .all(|candidate| candidate.preferred_name().is_some()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn erasure_rejects_a_capture_optout_snapshot_but_fresh_opted_out_review_can_erase(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    f.enable().await;
    f.stage("Retained after capture optout").await;
    let reviewed = f.get().await;
    let erase = form(&reviewed.body, "erase-names", "acknowledged=true");
    f.save("capture", &reviewed, "capture", "acknowledged=true")
        .await;
    assert_stale(&f.submit("erase", &erase).await);
    let opted_out = f.inspect().await;
    assert!(!opted_out.capture_enabled);
    assert_eq!(opted_out.candidates[0].state(), CandidateState::Withdrawn);
    assert!(opted_out.candidates[0].preferred_name().is_some());
    let fresh = f.get().await;
    f.save("erase", &fresh, "erase-names", "acknowledged=true")
        .await;
    let erased = f.inspect().await;
    assert!(!erased.capture_enabled);
    assert_eq!(erased.candidates[0].state(), CandidateState::Erased);
    assert!(erased.candidates[0].preferred_name().is_none());
}
