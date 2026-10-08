use super::*;
use crate::admin::usability_tests::{opening_tag, render};
use minijinja::context;

#[test]
fn unavailable_and_partial_dashboard_reads_never_claim_empty_or_healthy() {
    let html = render(
        "bear/memory/dashboard.html",
        context! {
            can_manage_bear => true, stats => Option::<u8>::None,
            head_count => Option::<i64>::None, reviewable_proposal_count => Option::<i64>::None,
            pending_proposals_complete => false, proposals_complete => false,
            proposals => [context! { id => "surviving", title => "SURVIVING PROPOSAL" }],
            pending_proposals => [context! { id => "surviving", title => "SURVIVING REVIEW" }],
            reflection_run_summary => Option::<u8>::None, reflection_slo => Option::<u8>::None,
            reflection_runs => Option::<Vec<u8>>::None, pair_reflection_runs => Option::<Vec<u8>>::None,
            recent => Option::<Vec<u8>>::None, by_kind => Option::<Vec<u8>>::None, by_profile => Option::<Vec<u8>>::None,
            inspection_errors => ["RECORDED READ FAILURE"],
        },
    );
    for text in [
        "RECORDED READ FAILURE",
        "SURVIVING PROPOSAL",
        "SURVIVING REVIEW",
        "Review queue is incomplete",
        "Proposal list is incomplete",
        "Reflection summary unavailable",
        "Reflection performance unavailable",
        "Reflection runs unavailable",
        "Pair reflection runs unavailable",
        "Recent additions unavailable",
        "Kind counts unavailable",
        "Scope counts unavailable",
    ] {
        assert_visible(&html, text);
    }
    for false_state in [
        "Nothing awaiting review",
        "No memory proposals",
        "No memory recorded yet",
        "No reflection runs match",
        "No pair reflection runs",
        "0%",
    ] {
        assert!(!html.contains(false_state), "{false_state}");
    }
}

#[test]
fn entity_inspection_errors_do_not_become_empty_results() {
    let list = render(
        "bear/memory/entities.html",
        context! {
            entities => Option::<Vec<u8>>::None,
            inspection_errors => ["ENTITY READ FAILED"],
        },
    );
    assert_visible(&list, "ENTITY READ FAILED");
    assert_visible(&list, "Entity list unavailable");
    assert!(!list.contains("0 entities shown"));
    assert!(!list.contains("No entities"));
    let detail = render(
        "bear/memory/entity.html",
        context! {
            entity => context! { entity_id => "entity", display_name => "RECORDED ENTITY", metadata_json => context! { evidence => "RECORDED EVIDENCE" } },
            handles => Option::<Vec<u8>>::None, related => Option::<Vec<u8>>::None,
            inspection_errors => ["RELATION READ FAILED"],
        },
    );
    for text in [
        "RECORDED ENTITY",
        "RECORDED EVIDENCE",
        "RELATION READ FAILED",
        "Entity handles unavailable",
        "Linked memory records unavailable",
    ] {
        assert_visible(&detail, text);
    }
    assert!(!detail.contains("No handles"));
    assert!(!detail.contains("No memory records reference"));
}

#[test]
fn browse_feedback_preserves_selected_paths_and_open_review_drafts() {
    let html = render(
        "bear/memory/browse.html",
        context! {
            can_manage_bear => true, paths_available => true, delete_error => "ACTION FAILED",
            groups => [context! { label => "pair", paths => [context! { logical_path => "pair/note.md", head_memory_id => "note", version_count => 1 }] }],
            form => context! { role => "invalid", confirm => "typed confirmation", paths => ["pair/note.md"],
                review_title => "TITLE DRAFT <unsafe>", suggested_action => "retain", review_summary => "SUMMARY DRAFT",
                review_rationale => "RATIONALE DRAFT", requires_human => "on" },
        },
    );
    for text in [
        "ACTION FAILED",
        "TITLE DRAFT",
        "SUMMARY DRAFT",
        "RATIONALE DRAFT",
    ] {
        assert_visible(&html, text);
    }
    let selection = opening_tag(&html, "input", "name", "paths");
    assert_eq!(selection.attribute("type"), Some("checkbox"));
    assert_eq!(selection.attribute("value"), Some("pair/note.md"));
    assert_eq!(
        selection.attribute("aria-label"),
        Some("Select pair/note.md")
    );
    assert_eq!(selection.attribute("form"), Some("memory-delete-form"));
    assert!(selection.has_attribute("checked"));
    assert_eq!(
        opening_tag(&html, "input", "name", "role").attribute("value"),
        Some("invalid")
    );
    assert_eq!(
        opening_tag(&html, "input", "name", "confirm").attribute("value"),
        Some("typed confirmation")
    );
    assert!(html.contains("TITLE DRAFT &lt;unsafe&gt;"));
    let acknowledgement = opening_tag(&html, "input", "name", "requires_human");
    assert_eq!(acknowledgement.attribute("value"), Some("on"));
    assert!(acknowledgement.has_attribute("checked"));
    assert_visible(&html, "cannot be undone");
}

async fn seed_bear(pool: &sqlx::PgPool, slug: &str) -> Uuid {
    bears_db::create_bear(
        pool,
        bears_db::BearParams {
            slug,
            name: "Memory action feedback",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn browse_route_keeps_validation_drafts_without_deleting_records() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("browse-draft-{}", Uuid::new_v4());
    let bear_id = seed_bear(&pool, &slug).await;
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let member_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let (app, stores) = test_app(pool.clone(), &sqlite_dir).await;
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let note = add_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "draft.md"),
        "CANONICAL ENTRY",
    )
    .await;
    let admin = login_cookie(&app, admin_id).await;
    let member = login_cookie(&app, member_id).await;
    let uri = format!("/bear/{slug}/memory/browse");
    let form = format!("role=pair&action=delete&confirm=wrong&paths={}&review_title=TITLE+DRAFT&review_summary=SUMMARY+DRAFT&review_rationale=RATIONALE+DRAFT&requires_human=on",
        urlencoding::encode(note.logical_path.as_deref().unwrap()));
    for (cookie, expected) in [(&member, StatusCode::FORBIDDEN), (&admin, StatusCode::OK)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&uri)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(form.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            let html = String::from_utf8(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap();
            for text in [
                "Type the profile name",
                "TITLE DRAFT",
                "SUMMARY DRAFT",
                "RATIONALE DRAFT",
            ] {
                assert_visible(&html, text);
            }
            assert_eq!(
                opening_tag(&html, "input", "name", "confirm").attribute("value"),
                Some("wrong")
            );
            let selection = opening_tag(&html, "input", "name", "paths");
            assert_eq!(selection.attribute("value"), note.logical_path.as_deref());
            assert!(selection.has_attribute("checked"));
        }
    }
    assert!(get_memory_record_detail(&stores, bear_id, &note.memory_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn migration_accepts_over_default_body_limit_and_exposes_import_failure_locally() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("migration-limit-{}", Uuid::new_v4());
    let bear_id = seed_bear(&pool, &slug).await;
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let member_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let (app, stores) = test_app(pool.clone(), &sqlite_dir).await;
    let admin = login_cookie(&app, admin_id).await;
    let member = login_cookie(&app, member_id).await;
    let uri = format!("/bear/{slug}/memory/import-legacy");
    let body = format!("--migration\r\nContent-Disposition: form-data; name=\"bundle\"; filename=\"legacy.bundle\"\r\nContent-Type: application/octet-stream\r\n\r\n# v2 git bundle\n{}\r\n--migration--\r\n", "x\n".repeat(3 * 1024 * 1024 / 2));
    let upload = |cookie: &str| {
        Request::builder()
            .method("POST")
            .uri(&uri)
            .header(header::COOKIE, cookie)
            .header(
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=migration",
            )
            .body(Body::from(body.clone()))
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(upload(&member)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let response = app.clone().oneshot(upload(&admin)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::LOCATION));
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_visible(&html, "Import failed:");
    assert_visible(&html, "Some records may already have been imported");
    assert_visible(&html, "self-contained");
    assert_visible(&html, "128 MiB");
    assert!(html.contains("id=\"legacy-memory-import\" open"));
    let store = stores.store_for_bear(bear_id).await.unwrap();
    assert_eq!(head_entry_count(&stores, bear_id).await.unwrap(), 0);
    assert!(list_recent_memory_records(&stores, bear_id, 10)
        .await
        .unwrap()
        .is_empty());
    drop(store);
}

#[tokio::test]
async fn migration_staging_failure_is_local_and_does_not_claim_success() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("migration-stage-{}", Uuid::new_v4());
    let bear_id = seed_bear(&pool, &slug).await;
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let (app, stores) = test_app(pool.clone(), &sqlite_dir).await;
    stores.store_for_bear(bear_id).await.unwrap();
    std::fs::write(sqlite_dir.0.join("imports"), b"not a directory").unwrap();
    let cookie = login_cookie(&app, admin_id).await;
    let body = "--stage\r\nContent-Disposition: form-data; name=\"bundle\"; filename=\"legacy.bundle\"\r\n\r\n# v2 git bundle\n\r\n--stage--\r\n";
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/bear/{slug}/memory/import-legacy"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=stage")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::LOCATION));
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_visible(&html, "Could not create the import directory");
    assert_visible(&html, "memory-volume permissions");
    assert_eq!(head_entry_count(&stores, bear_id).await.unwrap(), 0);
}
