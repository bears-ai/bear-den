//! Privacy, canonical eligibility, unavailable choices and contextual retry regressions.

use super::usability::{is_visible, render, repository_view, verify_fixture_user};
use super::*;
use den_core::ids::UserId;
use den_docket::DocketJobRow;
use den_service::{cabinet, connections};
use serde_json::json;

async fn response_html(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

async fn create_fixture_job(
    app: &axum::Router,
    cookie: &str,
    slug: &str,
    hat: den_core::ids::HatId,
    repository: Uuid,
) -> String {
    let response = post_form(app, cookie, &format!("/bear/{slug}/jobs/new"),
        format!("goal=Audit+fixture&hat_id={hat}&surface_id={repository}&commit_policy=per_task&task_title=Inspect&task_criteria=record")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}

async fn review_fixture_surfaces(
    pool: &sqlx::PgPool,
    bear: Uuid,
    hat: den_core::ids::HatId,
    surfaces: &[Uuid],
) {
    hats::manage::disable_work(pool, BearId::new(bear), hat)
        .await
        .unwrap();
    hats::manage::replace_surfaces(pool, BearId::new(bear), hat, surfaces)
        .await
        .unwrap();
    let current = hats::manage::get_hat(pool, BearId::new(bear), hat)
        .await
        .unwrap();
    let fingerprint = hats::identity::identity_fingerprint(
        &current.name,
        &current.purpose,
        &current.identity_prompt,
    );
    let stores = den_memory::MemoryStoreManager::new(&Config::test_stub());
    hats::manage::enable_work_if_empty(pool, &stores, BearId::new(bear), hat, &fingerprint)
        .await
        .unwrap();
}

async fn fixture_job(pool: &sqlx::PgPool, bear: Uuid, user: i32) -> DocketJobRow {
    PgDocketService::from_pool(pool)
        .list_jobs_for_viewer(bear, user, true, DocketJobListFilter::default())
        .await
        .unwrap()
        .remove(0)
}

#[test]
fn unavailable_choices_require_explicit_replacement_and_keep_nonsecret_edits() {
    let html = render(
        "work/job.html",
        json!({
            "job_id":"job", "goal":"Stored goal", "status":"draft", "repository_available":false,
            "edit_repository_unavailable":true, "edit_error":"Repository access changed",
            "edit_draft":{"goal":"Preserved edit", "surface_id":"old", "commit_policy":"per_task", "work_branch":"draft/branch", "allow_default_ref":true},
            "available_surfaces":[{"id":"replacement", "name":"Replacement"}],
            "dispatch_preflight":presentation::browser_preflight(Some(DocketCommitPolicy::PerTask), None), "browser_dispatch_ready":false,
        }),
    );
    assert!(is_visible(&html, "Repository access changed"));
    assert!(html.contains("<h2>Stored goal</h2>"));
    assert!(html.contains("value=\"Preserved edit\""));
    assert!(html.contains("value=\"draft&#x2f;branch\""));
    assert!(html.contains("value=\"\" selected disabled>Previous repository unavailable"));
    assert!(!html.contains("value=\"replacement\" selected"));
    assert!(is_visible(&html, "Save job settings"));
    let html = render(
        "work/new.html",
        json!({
            "draft":{"goal":"Draft", "surface_id":"removed", "hat_id":"removed-hat", "task_title":["Inspect"], "task_criteria":["record"]},
            "draft_surface_unavailable":true, "draft_hat_unavailable":true,
            "surfaces":[{"id":"replacement", "name":"Replacement"}],
            "hat_choices":[{"id":"another-hat", "name":"Hat", "surface_ids":["replacement"]}],
        }),
    );
    assert!(html.contains("selected disabled>Previous repository unavailable"));
    assert!(html.contains("selected disabled>Previous responsibility unavailable"));
    assert!(!html.contains("value=\"another-hat\" selected"));
    assert!(html.contains("value=\"Inspect\""));
}

#[test]
fn queue_links_and_readonly_account_copy_follow_projected_permissions() {
    let html = render(
        "work/index.html",
        json!({
            "provider_status":{"configured":false},
            "jobs":[{"bear_slug":"test-bear", "route_id":"job", "title":"Job", "status":"draft",
                "repositories":[{"name":"Readable repository", "route_id":null},{"name":null,"route_id":null}]}],
        }),
    );
    assert!(is_visible(&html, "Readable repository"));
    assert!(html.contains("Repository unavailable"));
    let queue = html
        .split_once("<th>Repositories</th>")
        .unwrap()
        .1
        .split_once("</table>")
        .unwrap()
        .0;
    assert!(!queue.contains("href=\"/work/surfaces/"));
    let html = render(
        "work/surface.html",
        repository_view(
            connection_view::LinkedAccount {
                surface_id: Uuid::nil(),
                id: connections::ConnectionId(Uuid::nil()),
                name: "Read-only App".into(),
                provider: connections::Provider::GithubApp,
                provider_label: connection_view::provider_label(connections::Provider::GithubApp),
                state: connection_view::AccountState::Available,
                can_manage: false,
                github_app_write_enabled: Some(false),
            },
            None,
        ),
    );
    assert!(is_visible(
        &html,
        "read-only — cannot publish repository changes"
    ));
    assert!(is_visible(&html, "Publication is disabled"));
    assert!(html.contains("A repository manager can attach their own replacement account"));
    assert!(!html.contains("or create its replacement"));
    assert!(!html.contains("name=\"credential_value\""));
}

#[tokio::test]
async fn repository_privacy_and_stale_job_drafts_follow_current_grants() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, slug, hat) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let repository = assigned_surface_id(&pool, owner, bear).await;
    let name = work_surfaces::surface_by_id(&pool, repository)
        .await
        .unwrap()
        .unwrap()
        .name;
    let account = connections::create(
        &pool,
        UserId::new(owner),
        "PRIVATE ACCOUNT METADATA",
        connections::Material::HttpsToken("PRIVATE ACCOUNT SECRET".into()),
        &work_test_config().den_secret_encryption_key,
    )
    .await
    .unwrap();
    connections::attach(&pool, UserId::new(owner), account, repository)
        .await
        .unwrap();
    assert_eq!(
        connection_view::linked_accounts(&pool, UserId::new(owner), &[repository])
            .await
            .unwrap()
            .len(),
        1
    );
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let url = create_fixture_job(&app, &cookie, &slug, hat, repository).await;
    let job = fixture_job(&pool, bear, owner).await;
    sqlx::query!(
        "UPDATE bear_jobs SET visibility = 'bear_visible' WHERE id = $1",
        job.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, html) = get_page(&app, &peer_cookie, &format!("/bear/{slug}/jobs")).await;
    assert!(html.contains(&name));
    assert!(!html.contains("PRIVATE ACCOUNT METADATA"));
    assert!(!html.contains("PRIVATE ACCOUNT SECRET"));
    assert!(!html.contains(&format!("href=\"/work/surfaces/{}\"", route_id(repository))));
    let accounts = connection_view::linked_accounts(&pool, UserId::new(peer), &[repository])
        .await
        .unwrap();
    assert!(
        accounts.is_empty(),
        "Bear membership is not repository-management permission"
    );
    sqlx::query!(
        "DELETE FROM work_surface_bears WHERE surface_id = $1 AND bear_id = $2",
        repository,
        bear
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, html) = get_page(&app, &peer_cookie, &format!("/bear/{slug}/jobs")).await;
    assert!(!html.contains(&name));
    assert!(html.contains("Repository unavailable"));
    hats::manage::disable_work(&pool, BearId::new(bear), hat)
        .await
        .unwrap();
    let replacement = assigned_surface_id(&pool, owner, bear).await;
    let response = post_form(&app, &peer_cookie, &format!("/bear/{slug}/jobs/new"),
        format!("goal=Preserved+private+draft&hat_id={hat}&surface_id={repository}&commit_policy=per_task&task_title=Inspect&task_criteria=record")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = response_html(response).await;
    assert!(
        !html.contains(&name),
        "an unassigned repository must not be named in validation"
    );
    assert!(html.contains("value=\"Preserved private draft\""));
    assert!(html.contains("selected disabled>Previous repository unavailable"));
    let response = post_form(&app, &peer_cookie, &format!("/bear/{slug}/jobs/new"),
        format!("goal=Hat+draft&hat_id={}&surface_id={replacement}&commit_policy=per_task&task_title=Inspect&task_criteria=record", Uuid::new_v4())).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let html = response_html(response).await;
    assert!(html.contains("selected disabled>Previous responsibility unavailable"));
    assert!(html.contains("value=\"Hat draft\""));
    assert!(!html.contains(&format!("value=\"{hat}\" selected")));
    let response = post_form(&app, &peer_cookie, &format!("{url}/edit"),
        format!("goal=Preserved+edit&surface_id={repository}&commit_policy=per_task&work_branch=draft%2Fbranch&allow_default_ref=true")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = response_html(response).await;
    assert!(!html.contains(&name));
    assert!(html.contains("value=\"Preserved edit\""));
    assert!(html.contains("value=\"draft&#x2f;branch\""));
    assert!(html.contains("selected disabled>Previous repository unavailable"));
    assert!(!html.contains(&format!("value=\"{replacement}\" selected")));
    let response = post_form(&app, &peer_cookie, &format!("{url}/edit"),
        format!("goal=Policy+draft&surface_id={replacement}&commit_policy=none&work_branch=draft%2Fbranch")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = response_html(response).await;
    assert!(is_visible(&html, "No Job settings were changed"));
    assert!(html.contains("value=\"Policy draft\""));
    assert!(html.contains("selected disabled>Output policy unavailable"));
    assert!(!html.contains("value=\"per_task\" selected"));
    let unchanged = PgDocketService::from_pool(&pool)
        .get_job(bear, job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.job.goal, "Audit fixture");
    assert_eq!(unchanged.job.work_surface_id, Some(repository));
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        peer,
        bear
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        get_page(&app, &peer_cookie, &url).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn queue_and_detail_use_all_assignment_hat_eligibility_and_preserve_assignments_on_edit() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, slug, hat) = seed_member(&pool).await;
    let first = assigned_surface_id(&pool, owner, bear).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let url = create_fixture_job(&app, &cookie, &slug, hat, first).await;
    let job = fixture_job(&pool, bear, owner).await;
    hats::manage::disable_work(&pool, BearId::new(bear), hat)
        .await
        .unwrap();
    let second = assigned_surface_id(&pool, owner, bear).await;
    sqlx::query!("INSERT INTO job_work_surface_assignments (job_id, work_surface_id, mutation_policy) VALUES ($1, $2, 'required')", job.id, second)
        .execute(&pool).await.unwrap();
    review_fixture_surfaces(&pool, bear, hat, &[first]).await;
    let readiness = presentation::browser_readiness(&pool, &job).await.unwrap();
    assert!(!readiness.hat_available);
    assert_eq!(
        readiness.blocker,
        Some(presentation::BrowserDispatchBlocker::ResponsibilityUnavailable)
    );
    for endpoint in [&url, &format!("/bear/{slug}/jobs")] {
        let (_, html) = get_page(&app, &cookie, endpoint).await;
        assert!(is_visible(
            &html,
            "Work responsibility does not currently permit every Job assignment"
        ));
    }
    let response = post_form(
        &app,
        &cookie,
        &format!("{url}/edit"),
        format!("goal=Edited+intent&surface_id={first}&commit_policy=per_task&work_branch="),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let assignments = sqlx::query_scalar!(
        "SELECT work_surface_id FROM job_work_surface_assignments WHERE job_id = $1",
        job.id
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(assignments.contains(&first) && assignments.contains(&second));
    review_fixture_surfaces(&pool, bear, hat, &[first, second]).await;
    assert!(
        presentation::browser_readiness(&pool, &job)
            .await
            .unwrap()
            .hat_available
    );
    sqlx::query!("UPDATE job_work_surface_assignments SET mutation_policy = 'forbidden' WHERE job_id = $1 AND work_surface_id = $2", job.id, first)
        .execute(&pool).await.unwrap();
    assert_eq!(
        presentation::browser_readiness(&pool, &job)
            .await
            .unwrap()
            .blocker,
        Some(presentation::BrowserDispatchBlocker::MutationForbidden)
    );
}

#[tokio::test]
async fn reusable_app_readonly_state_is_visible_and_manager_can_replace_the_account() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, slug, hat) = seed_member(&pool).await;
    let (manager, _, _, _) = seed_member(&pool).await;
    verify_fixture_user(&pool, owner).await;
    let config = work_test_config();
    let repository = work_surfaces::create_surface(
        &pool,
        owner,
        NewWorkSurface {
            name: format!("app-ui-{}", Uuid::new_v4().simple()),
            description: None,
            upstream_url: "https://github.com/example/audit.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        repository.id,
        bear
    )
    .execute(&pool)
    .await
    .unwrap();
    enable_fixture_hat(&pool, bear, repository.id).await;
    sqlx::query!(
        "INSERT INTO work_surface_managers (surface_id, user_id, role) VALUES ($1, $2, 'manager')",
        repository.id,
        manager
    )
    .execute(&pool)
    .await
    .unwrap();
    let account = connections::create(
        &pool,
        UserId::new(owner),
        "Read-only App fixture",
        connections::Material::GithubApp {
            installation: 123,
            write: false,
        },
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    connections::attach(&pool, UserId::new(owner), account, repository.id)
        .await
        .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let url = create_fixture_job(&app, &cookie, &slug, hat, repository.id).await;
    let job = fixture_job(&pool, bear, owner).await;
    let linked = connection_view::linked_accounts(&pool, UserId::new(manager), &[repository.id])
        .await
        .unwrap();
    assert_eq!(linked[0].github_app_write_enabled, Some(false));
    assert!(!linked[0].can_manage);
    let readiness = presentation::browser_readiness(&pool, &job).await.unwrap();
    assert!(!readiness.account_available);
    assert_eq!(
        readiness.blocker,
        Some(presentation::BrowserDispatchBlocker::AccountUnavailable)
    );
    let (_, html) = get_page(&app, &cookie, &url).await;
    assert!(is_visible(
        &html,
        "Repository authentication is unavailable or explicitly read-only"
    ));
    let (_, html) = get_page(&app, &cookie, "/connections").await;
    assert!(is_visible(
        &html,
        "Read-only installation — cannot publish repository changes"
    ));
    let replacement = connections::create(
        &pool,
        UserId::new(manager),
        "Write-enabled replacement",
        connections::Material::GithubApp {
            installation: 456,
            write: true,
        },
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    connections::attach(&pool, UserId::new(manager), replacement, repository.id)
        .await
        .unwrap();
    assert!(
        presentation::browser_readiness(&pool, &job)
            .await
            .unwrap()
            .account_available
    );
    let linked = connection_view::linked_accounts(&pool, UserId::new(owner), &[repository.id])
        .await
        .unwrap();
    assert_eq!(linked[0].github_app_write_enabled, Some(true));
    assert!(!linked[0].can_manage);
}

#[tokio::test]
async fn attachment_validation_preserves_selection_without_echoing_secrets_or_hidden_metadata() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    verify_fixture_user(&pool, owner).await;
    let repository = assigned_surface_id(&pool, owner, bear).await;
    let name = work_surfaces::surface_by_id(&pool, repository)
        .await
        .unwrap()
        .unwrap()
        .name;
    let account = connections::create(
        &pool,
        UserId::new(owner),
        "SSH attachment fixture",
        connections::Material::SshKey("SECRET_ATTACHMENT_MUST_NOT_RENDER".into()),
        &work_test_config().den_secret_encryption_key,
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let endpoint = format!("/connections/{}/repositories", account.0);
    let response = post_form(
        &app,
        &cookie,
        &endpoint,
        format!("surface_id={repository}&confirmed=true"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = response_html(response).await;
    assert!(is_visible(
        &html,
        "No repository authentication was changed"
    ));
    assert!(html.contains(&format!("value=\"{repository}\" selected")));
    assert!(is_visible(&html, "Attach / replace account"));
    assert!(!html.contains("SECRET_ATTACHMENT_MUST_NOT_RENDER"));
    assert!(!html.contains("No account was created"));
    assert!(connections::linked_repositories(&pool, &[repository])
        .await
        .unwrap()
        .is_empty());
    sqlx::query!(
        "DELETE FROM work_surface_managers WHERE surface_id = $1 AND user_id = $2",
        repository,
        owner
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = post_form(
        &app,
        &cookie,
        &endpoint,
        format!("surface_id={repository}&confirmed=true"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = response_html(response).await;
    assert!(!html.contains(&name));
    assert!(html.contains("selected disabled>Previous repository unavailable"));
    assert!(
        connection_view::linked_accounts(&pool, UserId::new(owner), &[repository])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn move_preview_rejects_cycles_and_inaccessible_destinations_without_disclosing_titles() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, _, _, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    let scope = den_cabinet::ActorScope::user(UserId::new(owner));
    let page = knowledge::page(&pool, owner, "Cycle source", "Document").await;
    let child = knowledge::page(&pool, owner, "Cycle child", "Document").await;
    cabinet::pages::organize(&pool, &scope, &child, Some(&page), 0, true)
        .await
        .unwrap();
    let hidden = knowledge::page(&pool, peer, "HIDDEN DESTINATION TITLE", "PRIVATE BODY").await;
    cabinet::pages::configure(
        &pool,
        &den_cabinet::ActorScope::user(UserId::new(peer)),
        &hidden,
        den_cabinet::CabinetPolicy {
            bears_may_write: true,
            review_required: false,
            allowed_kinds: None,
        },
        &[peer],
        &[],
        &[],
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    for destination in [&page, &child] {
        let (status, html) = get_page(
            &app,
            &cookie,
            &format!("/cabinet/{page}/move?parent={destination}&position=0"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!html.contains("Confirm move or reorder"));
    }
    let (status, html) = get_page(
        &app,
        &cookie,
        &format!("/cabinet/{page}/move?parent={hidden}&position=0"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("HIDDEN DESTINATION TITLE"));
    assert!(!html.contains("PRIVATE BODY"));
    assert!(!html.contains("Confirm move or reorder"));
    assert!(cabinet::pages::metadata(&pool, &scope, &page)
        .await
        .unwrap()
        .parent
        .is_none());
    assert_eq!(
        cabinet::pages::metadata(&pool, &scope, &child)
            .await
            .unwrap()
            .parent,
        Some(page)
    );
}
