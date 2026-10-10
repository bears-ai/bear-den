//! Canonical route/service regressions for review recovery, session isolation, and permission projections.

use super::*;
use crate::bear::hats::work_review_draft::WorkReviewDraftScope;
use axum_login::tower_sessions::Session;
use den_core::RuntimeContextLabel;
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource};

fn assert_admin_navigation(page: &str, slug: &str) {
    assert!(
        page.contains(&format!("href=\"/bear/{slug}/hats\"")),
        "{page}"
    );
    assert!(
        page.contains(&format!("href=\"/bear/{slug}/activity\"")),
        "{page}"
    );
    assert!(page.contains(">Diagnostics</a>"), "{page}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn editors_keep_admin_navigation_and_code_tokens_remain_member_readable(pool: PgPool) {
    let bear = new_bear(&pool, "navrecovery").await;
    let admin = user(&pool, bear, "navadmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "navmember", BEAR_ROLE_MEMBER).await;
    let other = new_bear(&pool, "navother").await;
    let outsider = user(&pool, other, "navoutsider", BEAR_ROLE_MEMBER).await;
    let app = app(&pool, config()).await;
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let outsider_cookie = cookie(&app, outsider).await;
    for editor in ["overview", "prompt"] {
        let path = format!("/bear/navrecovery/edit/{editor}");
        let (status, page, _) = request(&app, &admin_cookie, "GET", &path, "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_admin_navigation(&page, "navrecovery");
        assert_eq!(
            request(&app, &member_cookie, "GET", &path, "").await.0,
            StatusCode::FORBIDDEN
        );
    }
    for (editor, body, expected_status) in [
        (
            "overview",
            "slug=&name=Preserved&description=Purpose",
            StatusCode::OK,
        ),
        (
            "configuration",
            "default_model=missing%2Fmodel",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let path = format!("/bear/navrecovery/edit/{editor}");
        let (status, page, _) = request(&app, &admin_cookie, "POST", &path, body).await;
        assert_eq!(status, expected_status, "{page}");
        assert_admin_navigation(&page, "navrecovery");
        assert_eq!(
            request(&app, &member_cookie, "POST", &path, body).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let invalid_steering = format!("system_prompt={}", "a".repeat(100_001));
    let (status, page, _) = request(
        &app,
        &admin_cookie,
        "POST",
        "/bear/navrecovery/edit/prompt",
        &invalid_steering,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_admin_navigation(&page, "navrecovery");
    let path = "/bear/navrecovery/code-token";
    for (viewer, is_admin) in [(&admin_cookie, true), (&member_cookie, false)] {
        for (method, body) in [("GET", ""), ("POST", "name=Editor+recovery+test")] {
            let (status, page, _) = request(&app, viewer, method, path, body).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(page.contains(">Diagnostics</a>"), is_admin);
            assert_eq!(
                page.contains("href=\"/bear/navrecovery/activity\""),
                is_admin
            );
            if is_admin {
                assert_admin_navigation(&page, "navrecovery");
            } else {
                assert!(page.contains("href=\"/bear/navrecovery/identity#hats\""));
            }
            if method == "POST" {
                assert!(page.contains("You will not be able to see it again"));
            }
        }
    }
    for (method, body) in [("GET", ""), ("POST", "name=Denied")] {
        assert!(!request(&app, &outsider_cookie, method, path, body)
            .await
            .0
            .is_success());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn disabled_attached_skills_remain_visible_but_never_effective_for_any_use(pool: PgPool) {
    let bear = new_bear(&pool, "disabledskills").await;
    let owner = user(&pool, bear, "skillowner", BEAR_ROLE_ADMIN).await;
    let admin = user(&pool, bear, "skillotheradmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "skillviewer", BEAR_ROLE_MEMBER).await;
    let content = "A previously reviewed procedure";
    let id = skills::create_draft(
        &pool,
        UserId::new(owner),
        "Disabled attached",
        "1",
        "Procedure",
        content,
    )
    .await
    .unwrap();
    let checksum = skills::hash(content);
    skills::approve(&pool, UserId::new(owner), id, &checksum, true)
        .await
        .unwrap();
    skills::attach(
        &pool,
        BearId::new(bear),
        UserId::new(owner),
        id,
        &checksum,
        &RuntimeContextLabel::ALL,
        true,
    )
    .await
    .unwrap();
    let unrelated = skills::create_draft(
        &pool,
        UserId::new(owner),
        "Unrelated disabled",
        "1",
        "Private",
        "Private draft",
    )
    .await
    .unwrap();
    skills::disable(&pool, UserId::new(owner), unrelated)
        .await
        .unwrap();
    let draft = skills::create_draft(
        &pool,
        UserId::new(owner),
        "Unrelated draft",
        "1",
        "Private",
        "Other private draft",
    )
    .await
    .unwrap();
    skills::disable(&pool, UserId::new(owner), id)
        .await
        .unwrap();
    let app = app(&pool, config()).await;
    for (actor, can_manage) in [(owner, true), (admin, true), (member, false)] {
        let listed = skills::list(&pool, BearId::new(bear), UserId::new(actor))
            .await
            .unwrap();
        let attached = listed.iter().find(|skill| skill.id == id).unwrap();
        assert!(attached.attached && attached.disabled && !attached.approved);
        if actor != owner {
            assert!(!listed
                .iter()
                .any(|skill| skill.id == unrelated || skill.id == draft));
        }
        let viewer = cookie(&app, actor).await;
        let (status, page, _) =
            request(&app, &viewer, "GET", "/bear/disabledskills/skills", "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.contains("Disabled attached"));
        assert!(page.contains("disabled; not used in model requests"));
        assert!(!page.contains("Save permitted uses"));
        assert_eq!(page.contains("Detach from Bear"), can_manage);
        if actor != owner {
            assert!(!page.contains("Unrelated disabled"));
            assert!(!page.contains("Unrelated draft"));
        }
    }
    for profile in RuntimeContextLabel::ALL {
        assert!(skills::effective(&pool, BearId::new(bear), profile)
            .await
            .unwrap()
            .is_empty());
    }
    skills::detach(&pool, BearId::new(bear), UserId::new(admin), id)
        .await
        .unwrap();
    for actor in [admin, member] {
        assert!(!skills::list(&pool, BearId::new(bear), UserId::new(actor))
            .await
            .unwrap()
            .iter()
            .any(|skill| skill.id == id));
    }
}

pub(super) async fn inspect_draft_scope(
    Path((bear, hat, actor)): Path<(Uuid, Uuid, i32)>,
    session: Session,
) -> String {
    WorkReviewDraftScope::new(BearId::new(bear), HatId::new(hat), UserId::new(actor))
        .load(&session)
        .await
        .unwrap()
        .map(|draft| draft.rationale)
        .unwrap_or_default()
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_review_rationale_survives_pagination_is_scoped_and_clears_on_success(pool: PgPool) {
    let bear = new_bear(&pool, "workdraft").await;
    let admin = user(&pool, bear, "workdraftadmin", BEAR_ROLE_ADMIN).await;
    let other_admin = user(&pool, bear, "workdraftother", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "workdraftmember", BEAR_ROLE_MEMBER).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Engineering",
        "Review",
    )
    .await
    .unwrap();
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Other",
        "Other review",
    )
    .await
    .unwrap();
    let (app, state) = app_with_state(&pool, config()).await;
    let memory = state.memory_stores.store_for_bear(bear).await.unwrap();
    for index in 0..=den_memory::hat_review::REVIEW_PAGE_SIZE {
        append_memory_record(
            &memory,
            &LogicalMemoryPath::hat(hat.id, &format!("record-{index}")),
            "note",
            "curate",
            None,
            "Historical reviewed content",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    let surface = work_surfaces::create_surface(
        &pool,
        admin,
        NewWorkSurface {
            name: "work-draft-repository".into(),
            description: None,
            upstream_url: "https://example.test/work-draft.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        "",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(&pool, surface.id, bear, admin)
        .await
        .unwrap();
    manage::replace_surfaces(&pool, BearId::new(bear), hat.id, &[surface.id])
        .await
        .unwrap();
    let snapshot = hats::work_review::snapshot_for_admin(
        &pool,
        &state.memory_stores,
        BearId::new(bear),
        hat.id,
        UserId::new(admin),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.page_count, 2);
    let path = format!("/bear/workdraft/hats/{}/work-review", hat.id);
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let other_cookie = cookie(&app, other_admin).await;
    let rationale = "Private rationale <not for URLs>";
    let form = format!(
        "expected_sha256={}&expected_record_count={}&expected_identity_sha256={}&rationale={}",
        snapshot.sha256.as_ref().unwrap(),
        snapshot.total_records,
        snapshot.identity_sha256,
        urlencoding::encode(rationale)
    );
    let (status, page, location) = request(&app, &admin_cookie, "POST", &path, &form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(location.is_none());
    assert!(page.contains("Private rationale &lt;not for URLs&gt;"));
    assert!(!page.contains("name=\"rationale\""));
    assert!(!page.contains("name=\"confirm_work_audience\""));
    assert!(
        !manage::get_hat(&pool, BearId::new(bear), hat.id)
            .await
            .unwrap()
            .work_enabled
    );
    let stale_form = format!(
        "{}&confirm_work_audience=true",
        form.replace(snapshot.sha256.as_ref().unwrap(), &"0".repeat(64)),
    );
    let (status, page, _) = request(&app, &admin_cookie, "POST", &path, &stale_form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("hat memory changed since review"));
    assert!(page.contains("Private rationale &lt;not for URLs&gt;"));
    for (scope_bear, scope_hat, actor, expected) in [
        (bear, hat.id.as_uuid(), admin, rationale),
        (Uuid::new_v4(), hat.id.as_uuid(), admin, ""),
        (bear, other_hat.id.as_uuid(), admin, ""),
        (bear, hat.id.as_uuid(), other_admin, ""),
    ] {
        let (_, saved, _) = request(
            &app,
            &admin_cookie,
            "GET",
            &format!("/test-review-draft/{scope_bear}/{scope_hat}/{actor}"),
            "",
        )
        .await;
        assert_eq!(saved, expected);
    }
    let next = format!(
        "{path}?page=2&expected_sha256={}&expected_identity_sha256={}",
        snapshot.sha256.as_ref().unwrap(),
        snapshot.identity_sha256
    );
    let (status, page, _) = request(&app, &admin_cookie, "GET", &next, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Page 2 of 2"));
    assert!(page.contains("name=\"rationale\""));
    assert!(page.contains("Private rationale &lt;not for URLs&gt;"));
    assert!(page.contains("name=\"confirm_work_audience\" value=\"true\" required"));
    assert!(!page.contains(" checked"));
    assert!(!next.contains("rationale"));
    let (status, page, _) = request(&app, &other_cookie, "GET", &next, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(!page.contains("Private rationale"));
    let (status, page, _) = request(&app, &member_cookie, "GET", &next, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!page.contains("Private rationale"));
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &path,
            &format!("{form}&confirm_work_audience=true")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let (_, saved, _) = request(
        &app,
        &admin_cookie,
        "GET",
        &format!("/test-review-draft/{bear}/{}/{admin}", hat.id),
        "",
    )
    .await;
    assert!(saved.is_empty());
    manage::replace_surfaces(&pool, BearId::new(bear), hat.id, &[])
        .await
        .unwrap();
    assert!(
        manage::get_hat(&pool, BearId::new(bear), hat.id)
            .await
            .unwrap()
            .work_enabled
    );
    let (status, page, _) = request(
        &app,
        &admin_cookie,
        "GET",
        &format!("/bear/workdraft/hats/{}", hat.id),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Work is blocked: no repositories are permitted"));
    assert!(page.contains("Disable Job use below before adding repositories"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn knowledge_review_stale_conflicts_keep_authoring_in_authorized_forms(pool: PgPool) {
    let bear = new_bear(&pool, "reviewdrafts").await;
    let admin = user(&pool, bear, "reviewdraftadmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "reviewdraftmember", BEAR_ROLE_MEMBER).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Engineering",
        "Review",
    )
    .await
    .unwrap();
    let (app, state) = app_with_state(&pool, config()).await;
    let memory = state.memory_stores.store_for_bear(bear).await.unwrap();
    let conversation = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "review-draft-source",
        None,
        None,
    )
    .await
    .unwrap();
    let private = append_memory_record(
        &memory,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation.id), "source"),
        "source",
        "pair",
        None,
        "Private source to inspect",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let hat_source = append_memory_record(
        &memory,
        &LogicalMemoryPath::hat(hat.id, "source"),
        "source",
        "curate",
        None,
        "Reviewed hat source to inspect",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let legacy = append_memory_record(
        &memory,
        &LogicalMemoryPath::profile_local("pair", "legacy"),
        "legacy",
        "pair",
        None,
        "Unattributed source to inspect",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    for (route, source, acknowledgement, audience_status, target_path) in [
        (
            "review",
            private.memory_id.as_str(),
            "acknowledge_sharing",
            StatusCode::BAD_REQUEST,
            LogicalMemoryPath::hat(hat.id, "edited"),
        ),
        (
            "core-review",
            hat_source.memory_id.as_str(),
            "acknowledge_bear_and_work_audience",
            StatusCode::FORBIDDEN,
            LogicalMemoryPath::shared_core("edited"),
        ),
        (
            "legacy-review",
            legacy.memory_id.as_str(),
            "acknowledge_unverified_source_and_members",
            StatusCode::FORBIDDEN,
            LogicalMemoryPath::hat(hat.id, "legacy-edited"),
        ),
    ] {
        let path = format!("/bear/reviewdrafts/hats/{}/{route}", hat.id);
        let kind = if route == "legacy-review" {
            "legacy-edited"
        } else {
            "edited"
        };
        let form = format!("source_memory_id={source}&kind={kind}&reviewed_content=Authored+%3Cdraft%3E&review_notes=Preserved+%3Crationale%3E&{acknowledgement}=true");
        let (status, page, _) = request(
            &app,
            &admin_cookie,
            "POST",
            &path,
            &form.replace(&format!("&{acknowledgement}=true"), ""),
        )
        .await;
        assert_eq!(status, audience_status, "{page}");
        assert!(page.contains("No entry was published"));
        assert!(page.contains("Authored &lt;draft&gt;"));
        assert!(!page.contains(" checked"));
        let invalid_form = form.replace(
            "review_notes=Preserved+%3Crationale%3E",
            "review_notes=short",
        );
        let (status, page, _) = request(&app, &admin_cookie, "POST", &path, &invalid_form).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert!(page.contains("Authored &lt;draft&gt;"));
        assert!(page.contains(">short</textarea>"));
        assert!(!page.contains(" checked"));
        let head = append_memory_record(
            &memory,
            &target_path,
            "note",
            "curate",
            None,
            "Concurrent target that must be reviewed",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
        let (status, page, location) = request(&app, &admin_cookie, "POST", &path, &form).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert!(location.is_none());
        assert!(page.contains("changed since review"));
        assert!(page.contains("No entry was published"));
        assert!(page.contains("Authored &lt;draft&gt;"));
        assert!(page.contains("Preserved &lt;rationale&gt;"));
        assert!(page.contains("Concurrent target that must be reviewed"));
        assert!(page.contains(&format!(
            "name=\"expected_head\" value=\"{}\"",
            head.memory_id
        )));
        assert!(page.contains(&format!(
            "name=\"{acknowledgement}\" value=\"true\" required"
        )));
        assert!(!page.contains(" checked"));
        for method in ["GET", "POST"] {
            let (status, page, _) = request(&app, &member_cookie, method, &path, &form).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert!(!page.contains("Authored"));
            assert!(!page.contains("source to inspect"));
        }
        let retry = format!("{form}&expected_head={}", head.memory_id);
        let (status, page, _) = request(
            &app,
            &admin_cookie,
            "POST",
            &path,
            &retry.replace(&format!("&{acknowledgement}=true"), ""),
        )
        .await;
        assert_eq!(status, audience_status, "{page}");
        assert_eq!(
            request(&app, &admin_cookie, "POST", &path, &retry).await.0,
            StatusCode::SEE_OTHER
        );
    }
}
