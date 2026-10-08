use super::*;

async fn post_form(app: &axum::Router, cookie: &str, path: &str, form: String) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn people_preserves_failed_drafts_and_shared_last_admin_guard_without_widening_authority() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let member = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let member_cookie = login_cookie(&app, member).await;
    let path = format!("/bear/{slug}/members/grant");
    let response = post_form(
        &app,
        &cookie,
        &path,
        "username=missing-user-draft&role=admin".into(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("User not found"));
    assert!(page.contains("value=\"missing-user-draft\""));
    assert!(page.contains("value=\"admin\" selected"));
    let response = post_form(&app, &cookie, &path, format!("user_id={admin}&role=member")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains(bears_db::LAST_BEAR_ADMIN_MESSAGE));
    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{slug}/members/{admin}/revoke"),
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        bears_db::count_bear_admins(&pool, bear_id).await.unwrap(),
        1
    );
    for form in [
        format!("user_id={member}&role=owner"),
        "username=&role=admin".into(),
    ] {
        assert_eq!(
            post_form(&app, &cookie, &path, form).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        post_form(
            &app,
            &member_cookie,
            &path,
            format!("user_id={member}&role=admin")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        bears_db::membership_role_for_user(&pool, member, bear_id)
            .await
            .unwrap()
            .flatten()
            .as_deref(),
        Some(BEAR_ROLE_MEMBER)
    );
}

#[tokio::test]
async fn racing_people_revokes_cannot_remove_every_admin() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let first = create_bear_admin_user(&pool, bear_id).await;
    let second = create_bear_admin_user(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let first_cookie = login_cookie(&app, first).await;
    let second_cookie = login_cookie(&app, second).await;
    let first_path = format!("/bear/{slug}/members/{second}/revoke");
    let second_path = format!("/bear/{slug}/members/{first}/revoke");
    let (a, b) = tokio::join!(
        post_form(&app, &first_cookie, &first_path, String::new()),
        post_form(&app, &second_cookie, &second_path, String::new())
    );
    assert_eq!(
        usize::from(a.status() == StatusCode::SEE_OTHER)
            + usize::from(b.status() == StatusCode::SEE_OTHER),
        1
    );
    assert_eq!(
        bears_db::count_bear_admins(&pool, bear_id).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn advanced_validation_retains_nonsecret_proposals_and_never_writes_before_key_validation() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let path = format!("/bear/{slug}/models");
    let secret = "never-render-this-replacement-key";
    let before = bears_db::get_bear(&pool, bear_id)
        .await
        .unwrap()
        .unwrap()
        .default_tool_budget_multiplier;
    let loop_before = bears_db::bear_agent_loop_control_setting(&pool, bear_id)
        .await
        .unwrap();
    // Config::test_stub has no gateway management credentials; validation fails
    // without contacting an external provider or applying loop/budget settings.
    let response = post_form(&app, &cookie, &path, format!("bear_loop_control=careful&bear_tool_budget_multiplier=1.25&bifrost_virtual_key_id=proposed-id&bifrost_virtual_key_name=proposed-name&bifrost_virtual_key_value={secret}")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response.headers().get(header::LOCATION).is_none());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("Nothing saved"));
    assert!(page.contains("value=\"careful\" selected"));
    assert!(page.contains("value=\"1.25\""));
    assert!(page.contains("value=\"proposed-id\""));
    assert!(page.contains("value=\"proposed-name\""));
    assert!(!page.contains(secret));
    assert_eq!(
        bears_db::bear_agent_loop_control_setting(&pool, bear_id)
            .await
            .unwrap(),
        loop_before
    );
    assert_eq!(
        bears_db::get_bear(&pool, bear_id)
            .await
            .unwrap()
            .unwrap()
            .default_tool_budget_multiplier,
        before
    );
    let response = post_form(&app, &cookie, &path, "bear_loop_control=strict&bear_tool_budget_multiplier=invalid&bifrost_virtual_key_name=keep-draft".into()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("value=\"strict\" selected"));
    assert!(page.contains("keep-draft"));
    assert!(page.contains("Nothing saved"));
}

#[test]
fn reflection_failures_never_claim_inspection_and_empty_reads_are_not_unavailable() {
    let mut errors = Vec::new();
    let empty: Option<Vec<String>> = super::super::super::memory::inspection::read_result(
        Ok::<_, &str>(Vec::new()),
        "Notes",
        &mut errors,
    );
    assert!(empty.unwrap().is_empty());
    assert!(errors.is_empty());
    for (status, skip, error) in [
        (Some("failed"), None, None),
        (
            Some("processed"),
            Some("no_compaction_artifact"),
            Some("Extraction failed"),
        ),
        (None, None, None),
    ] {
        let feedback =
            super::super::super::memory::inspection::reflection_feedback(status, skip, error);
        assert!(!feedback.status_explanation.contains("was inspected"));
        assert!(feedback.needs_attention);
    }
    assert_eq!(
        reflection_counts_label(Some("processed"), None, Some(0), Some(0)),
        "candidates 0, proposals 0"
    );
    assert_eq!(
        reflection_counts_label(
            Some("skipped"),
            Some("no_compaction_artifact"),
            Some(0),
            Some(0)
        ),
        "not extracted"
    );
    assert_eq!(
        reflection_counts_label(Some("failed"), None, Some(4), Some(2)),
        "candidates 4, proposals 2"
    );
}

#[tokio::test]
async fn manual_failure_is_a_bearwire_event_not_transcript_and_get_shows_actual_feedback() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let mut conv = conversation_persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin),
        &format!("failed-reflect-{}", Uuid::new_v4()),
        None,
        Some("Failure evidence"),
    )
    .await
    .unwrap();
    conv.external_conversation_id = None;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let state = test_state(pool.clone());
    let result = reflect_persisted_conversation(&state, Some(admin), &bear, &conv, "manual")
        .await
        .unwrap();
    assert!(result.error.is_some());
    assert!(result.reflection_event_id.is_some());
    assert!(!result.proposals_complete);
    let rows = reflection_rows_for_bear(&pool, bear_id, Some(conv.id), 20)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status.as_deref(), Some("failed"));
    assert_eq!(rows[0].status_label, "Failed");
    assert_eq!(rows[0].candidate_count, None);
    assert_eq!(rows[0].proposal_count, None);
    assert_eq!(rows[0].counts_label, "candidates —, proposals —");
    assert!(rows[0].proposal_links.is_empty());
    assert_eq!(serde_json::to_value(&rows[0]).unwrap()["proposal_count"], 0);
    assert!(rows[0]
        .error
        .as_deref()
        .unwrap()
        .contains("Checkpoint creation"));
    assert!(conversation_timeline_rows(&rows)[0]
        .details
        .contains("Checkpoint creation"));
    assert!(list_messages_page(&pool, conv.id, None, 40)
        .await
        .unwrap()
        .is_empty());
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let (status, page) = get_as(
        &app,
        &cookie,
        &format!(
            "/bear/{slug}/conversations/{}?message=Saved-feedback&error=Stage-failure",
            conv.id
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Saved-feedback"));
    assert!(page.contains("Stage-failure"));
    assert!(page.contains("Checkpoint creation"));
    assert!(page.contains("candidates —, proposals —"));
    assert!(page.contains("Retry now"));
    assert!(!page.contains("Inspected, no memories found"));
}
