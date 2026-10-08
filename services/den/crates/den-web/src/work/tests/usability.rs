//! Full-parent render regressions and authorized browser flow checks.

use super::*;
use den_core::ids::UserId;
use den_service::{cabinet, connections};
use serde_json::{json, Value};

pub(super) async fn verify_fixture_user(pool: &sqlx::PgPool, user: i32) {
    let result = sqlx::query!(
        "INSERT INTO email_configs (user_id, email_address, active, verified_at) SELECT id, email, true, NOW() FROM users WHERE id = $1",
        user,
    )
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(result.rows_affected(), 1);
}

pub(super) fn repository_view(
    linked_account: connection_view::LinkedAccount,
    readiness_error: Option<&str>,
) -> Value {
    json!({
        "name":"Repository", "surface_id":route_id(linked_account.surface_id),
        "surface_reference":entity_ref(linked_account.surface_id, "Repository", "Repository", None),
        "description":null, "upstream_url":"https://example.test/repository.git", "default_ref":"main",
        "default_image":null, "allowed_outbound_hosts":"", "credential_kind":null,
        "github_app_installation_id":null, "github_app_write_enabled":false,
        "linked_account":linked_account, "readiness_error":readiness_error,
        "provider_root_status":null, "provider_root_inspection":null, "message":null,
        "managers":[], "assigned_bears":[], "assignable_bears":[], "images":[],
    })
}

pub(super) fn render(name: &str, fields: Value) -> String {
    let config = work_test_config();
    let env = crate::template_environment(&config);
    let mut data = json!({
        "bear_slug": "test-bear", "bear_name": "Test Bear", "bear": {"slug":"test-bear", "name":"Test Bear"},
        "session": {"username":"tester", "is_admin":false}, "app_display_name":"Den",
        "draft": NewJobForm::default(), "mission":{}, "canonical_diagnostics":{},
        "page":{}, "authored_by":{},
    });
    data.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    let html = env.get_template(name).unwrap().render(data).unwrap();
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("/assets/css/style.css"));
    assert!(html.contains("name=\"viewport\""));
    html
}

/// Essential state/actions must not be inside a closed native disclosure.
pub(super) fn is_visible(html: &str, text: &str) -> bool {
    let end = html.find(text).unwrap_or_else(|| panic!("missing {text}"));
    let mut cursor = 0;
    let mut closed = Vec::new();
    while cursor < end {
        let remainder = &html[cursor..end];
        let start = remainder.find("<details");
        let finish = remainder.find("</details>");
        match (start, finish) {
            (Some(start), finish) if finish.is_none_or(|finish| start < finish) => {
                cursor += start;
                let tag_end = html[cursor..].find('>').unwrap() + cursor;
                closed.push(
                    !html[cursor..tag_end]
                        .split_whitespace()
                        .any(|word| word == "open"),
                );
                cursor = tag_end + 1;
            }
            (_, Some(finish)) => {
                closed.pop();
                cursor += finish + "</details>".len();
            }
            _ => break,
        }
    }
    !closed.into_iter().any(|closed| closed)
}

#[test]
fn browser_policies_follow_canonical_repository_change_preflight() {
    for policy in [
        DocketCommitPolicy::PerTask,
        DocketCommitPolicy::PerJob,
        DocketCommitPolicy::None,
    ] {
        let supported = policy == DocketCommitPolicy::PerTask;
        assert_eq!(
            presentation::require_supported_policy(policy).is_ok(),
            supported
        );
        assert_eq!(
            presentation::browser_preflight(Some(policy), None).dispatchable,
            supported
        );
    }
    let html = render(
        "work/new.html",
        json!({"surfaces":[{"id":"repo", "name":"Repository"}], "hat_choices":[{"id":"hat", "name":"Hat", "surface_ids":["repo"]}]}),
    );
    assert!(!html.contains("value=\"per_job\""));
    assert!(!html.contains("value=\"none\""));
    assert!(html.contains("Creating a Job does not start work"));
    assert!(html.contains("bypasses the normal work-branch safety boundary"));
}

#[test]
fn healthy_queue_diagnostics_follow_actions_and_failures_remain_visible() {
    let healthy = render(
        "work/index.html",
        json!({
            "provider_status":{"configured":true,"reachable":true,"backend_available":true,"url":"HEALTHY_PROVIDER_MARKER","active_sandboxes":2},
            "jobs":[],
        }),
    );
    assert!(!is_visible(&healthy, "HEALTHY_PROVIDER_MARKER"));
    assert!(healthy.find("New Job").unwrap() < healthy.find("Provider diagnostics").unwrap());
    let failed = render(
        "work/index.html",
        json!({"provider_status":{"configured":true,"reachable":false,"url":"https://provider.example.test","error":"Failure evidence"},"jobs":[]}),
    );
    assert!(is_visible(&failed, "Sandbox provider unreachable"));
}

#[test]
fn cabinet_publish_and_move_summaries_keep_inherited_audience_and_conflict_drafts_visible() {
    let audience = json!({"kind":"restricted","people":["alice"],"bears":["careful-bear"]});
    for template in ["cabinet/new.html", "cabinet/edit.html", "cabinet/move.html"] {
        let html = render(
            template,
            json!({"cabinet_ref":"page","content":"Preserved draft","item_title":"Page","page_title":"Page","destination_title":"Destination","audience":audience,"current_audience":audience,"target_audience":audience,"error":"Conflict explanation"}),
        );
        assert!(is_visible(&html, "alice"));
        assert!(is_visible(&html, "careful-bear"));
        assert!(html.contains("Attachments and saved copies keep their own access policy"));
        if template == "cabinet/edit.html" {
            assert!(is_visible(&html, "Conflict explanation"));
            assert!(html.contains("Preserved draft"));
            assert!(html.contains("Inspect the latest revision"));
        }
    }
}

#[test]
fn job_blockers_retry_and_results_precede_optional_setup() {
    let html = render(
        "work/job.html",
        json!({
            "job_id":"job", "goal":"Repair the build", "status":"blocked", "commit_policy":"per_job",
            "selected_work_surface_id":"repo", "dispatch_preflight":presentation::browser_preflight(Some(DocketCommitPolicy::PerJob), None),
            "tasks":[{"id":"task", "display_id":"task", "title":"Build", "depth":0, "status":"blocked", "can_retry":true, "blocker_reason":"Missing dependency"}],
            "has_runnable_work":true, "outcome_entries":[{"summary":"Recorded result"}],
        }),
    );
    for text in [
        "Missing dependency",
        "Retry task",
        "Recorded result",
        "This historical output policy",
    ] {
        assert!(is_visible(&html, text), "hidden: {text}");
    }
    assert!(html.find("Retry task").unwrap() < html.find("Mission page").unwrap());
    assert!(
        html.find("Recorded result").unwrap() < html.find("Job setup &amp; maintenance").unwrap()
    );
    assert!(html.contains("Historical: Publish to the job branch"));
    assert!(!html.contains("Promote a durable decision"));
}

#[test]
fn run_controls_diffs_and_outcome_are_primary_but_healthy_logs_are_closed() {
    let fields = json!({
        "run":{"id":"run", "job_route_id":"job", "display_id":"run", "title":"Build", "state":"succeeded", "is_active":false, "execution_target":"sandbox", "result_summary":"Built successfully"},
        "outcome":"Built successfully", "can_retry":true, "log_tail":"ROUTINE_LOG_MARKER",
        "changed_files":[{"status":"M", "path":"src/main.rs"}], "diff_patch":"CHANGED_CONTENT_MARKER",
    });
    let html = render("work/run.html", fields.clone());
    assert!(is_visible(&html, "Built successfully"));
    assert!(is_visible(&html, "Retry (new attempt)"));
    assert!(is_visible(&html, &"src/main.rs".replace('/', "&#x2f;")));
    assert!(!is_visible(&html, "ROUTINE_LOG_MARKER"));
    assert!(html.find("Changed files").unwrap() < html.find("Execution metadata").unwrap());
    let mut failed = fields;
    failed["run"]["error"] = json!("FAILURE_MARKER");
    let html = render("work/run.html", failed);
    assert!(is_visible(&html, "FAILURE_MARKER"));
    assert!(is_visible(&html, "ROUTINE_LOG_MARKER"));
}

#[test]
fn repository_uses_linked_account_without_offering_forbidden_local_edits() {
    let html = render(
        "work/surface.html",
        repository_view(
            connection_view::LinkedAccount {
                surface_id: Uuid::nil(),
                id: connections::ConnectionId(Uuid::nil()),
                name: "Team token".into(),
                provider: connections::Provider::GitHttps,
                provider_label: connection_view::provider_label(connections::Provider::GitHttps),
                state: connection_view::AccountState::Revoked,
                can_manage: true,
                github_app_write_enabled: None,
            },
            Some("Provider unavailable"),
        ),
    );
    assert!(is_visible(&html, "Provider unavailable"));
    assert!(is_visible(
        &html,
        "Provider readiness has not been verified"
    ));
    assert!(is_visible(&html, "Team token"));
    assert!(html.contains(&format!("/connections#account-{}", Uuid::nil())));
    assert!(!html.contains("name=\"credential_value\""));
    assert!(!html.contains("/github-app\""));
    assert!(!html.contains("No credential set"));
    assert!(html.find("Readiness").unwrap() < html.find("Repository settings").unwrap());
}

#[test]
fn cabinet_content_and_consequences_do_not_depend_on_javascript() {
    let html = render(
        "cabinet/item.html",
        json!({
            "cabinet_ref":"page", "item_title":"Mission", "content":"READABLE_CONTENT_MARKER", "lifecycle":"active", "is_current":true,
            "page":{"can_write":true,"can_manage":true}, "audience":{"kind":"open_wiki"},
            "byte_storage_enabled":true, "upload_bears":[{"id":"bear","name":"Bear"}],
            "attachments":[{"reference":"file","artifact":{"title":"File"}}],
        }),
    );
    assert!(is_visible(&html, "READABLE_CONTENT_MARKER"));
    assert!(html.find("Child pages").unwrap() < html.find("Add source").unwrap());
    assert!(html.find("Attachments").unwrap() < html.find("Add source").unwrap());
    assert_eq!(html.matches("/cabinet/page/edit\"").count(), 1);
    assert_eq!(html.matches("/cabinet/page/history\"").count(), 1);
    assert!(html.contains("File (maximum 16 MiB)"));
    assert!(is_visible(&html, "This removes a retaining link"));
    assert!(is_visible(
        &html,
        "Page readers do not automatically receive file access"
    ));
    assert!(html.contains("type=\"checkbox\" required>Delete this page"));
    assert!(!html.contains("onsubmit=\"return confirm"));
    assert!(html.contains("method=\"get\" action=\"/cabinet/page/move\""));
    assert!(html.contains("Blank <strong>People and Bears together</strong>"));
    let cleanup = render(
        "cabinet/uploads.html",
        json!({"storage_enabled":true,"uploads":[{"reference":"file","title":"File","can_retry":true}]}),
    );
    assert!(is_visible(&cleanup, "Cleanup permanently removes"));
    assert!(cleanup.contains("Retry file removal"));
}

#[test]
fn shared_connections_and_reviews_have_truthful_scoped_states() {
    let html = render(
        "connections.html",
        json!({
            "draft":null, "error":null, "sync_pending":false,
            "attachment_feedback":null, "attachment_account_available":false,
            "attachment_repository_available":false, "has_linkable_repositories":true,
            "connection_catalog":[{
                "id":"account", "name":"Account", "provider":"git_https", "revision":1,
                "repository_count":1, "revoked":false, "other_repository_count":0,
                "repositories":[{"id":"repo", "name":"Repository"}], "github_app_write_enabled":null,
            }],
            "repositories":[{
                "id":"repo", "name":"Repository", "can_link":true,
                "credential_configured":false, "github_app_configured":false,
                "linked_account":{
                    "surface_id":"repo", "id":"account", "name":"Account", "provider":"git_https",
                    "provider_label":"Git HTTPS token", "state":"available", "can_manage":true,
                    "github_app_write_enabled":null,
                },
            }], "bears":[],
        }),
    );
    assert!(html.contains("name=\"confirmed\" value=\"true\" required"));
    assert!(html.contains("value=\"\" selected disabled>Choose a repository"));
    assert!(!html.contains("value=\"repo\" selected"));
    assert!(html.contains("HTTPS access token"));
    assert!(html.contains("SSH private key"));
    assert!(html.contains("GitHub App installation ID"));
    assert!(is_visible(&html, "Revoking this account"));
    assert!(is_visible(&html, "Detaching"));
    let reviews = render(
        "reviews.html",
        json!({"bears":[],"cabinet_reviews":[{"cabinet_ref":"page","version_ref":"version","title":"Pending page","revision":2}]}),
    );
    assert!(reviews.contains("No Bear-admin reviews in this scope"));
    assert!(reviews.contains("Pending page"));
    assert!(!reviews.contains("No review access"));
}

#[tokio::test]
async fn browser_job_policy_rejection_preserves_draft_and_historical_repair_is_explicit() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user, bear, slug, hat) = seed_member(&pool).await;
    let surface = assigned_surface_id(&pool, user, bear).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user).await;
    let endpoint = format!("/bear/{slug}/jobs/new");
    for policy in ["none", "per_job"] {
        let response = post_form(&app, &cookie, &endpoint, format!("goal=Preserved+goal&hat_id={hat}&surface_id={surface}&commit_policy={policy}&task_title=Inspect&task_criteria=record")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
        assert!(html.contains("value=\"Preserved goal\""));
        assert!(html.contains("No Job was created"));
        assert!(html.contains("value=\"Inspect\""));
    }
    let response = post_form(&app, &cookie, &endpoint, format!("goal=Policy+fixture&hat_id={hat}&surface_id={surface}&commit_policy=per_task&task_title=Inspect&task_criteria=record")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let url = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let service = PgDocketService::from_pool(&pool);
    let jobs = service
        .list_jobs_for_viewer(bear, user, true, DocketJobListFilter::default())
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(
        work_runs::list_work_runs(
            &pool,
            WorkRunListFilter {
                job_id: Some(job.id),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .len(),
        0
    );
    let (_, index) = get_page(&app, &cookie, &format!("/bear/{slug}/jobs")).await;
    let repository = work_surfaces::surface_by_id(&pool, surface)
        .await
        .unwrap()
        .unwrap();
    assert!(index.contains(&repository.name));
    assert!(
        index.contains("(dispatched)"),
        "lifecycle must not be copied from Job status"
    );
    sqlx::query!(
        "UPDATE bear_jobs SET commit_policy = 'per_job' WHERE id = $1",
        job.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, historical) = get_page(&app, &cookie, &url).await;
    assert!(historical.contains("Historical: Publish to the job branch"));
    assert!(is_visible(&historical, "This historical output policy"));
    let body =
        format!("goal=Edited+intent&surface_id={surface}&commit_policy=per_job&work_branch=");
    assert_eq!(
        post_form(&app, &cookie, &format!("{url}/edit"), body)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &format!("{url}/edit"),
            format!("goal=Edited+intent&surface_id={surface}&commit_policy=none&work_branch=")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &format!("{url}/edit"),
            format!("goal=Edited+intent&surface_id={surface}&commit_policy=per_task&work_branch=")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let (_, repaired) = get_page(&app, &cookie, &url).await;
    assert!(!repaired.contains("This historical output policy"));
}

#[tokio::test]
async fn reusable_account_forms_confirm_effects_and_repository_projection_follows_revocation() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    verify_fixture_user(&pool, owner).await;
    let surface = assigned_surface_id(&pool, owner, bear).await;
    let config = work_test_config();
    let account = connections::create(
        &pool,
        UserId::new(owner),
        "Reusable fixture",
        connections::Material::HttpsToken("SECRET_DO_NOT_RENDER".into()),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let attach = format!(
        "/connections/{account_id}/repositories",
        account_id = account.0
    );
    assert_eq!(
        post_form(&app, &cookie, &attach, format!("surface_id={surface}"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &attach,
            format!("surface_id={surface}&confirmed=true")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let (_, html) = get_page(
        &app,
        &cookie,
        &format!("/work/surfaces/{}", route_id(surface)),
    )
    .await;
    assert!(html.contains("Reusable fixture"));
    assert!(!html.contains("SECRET_DO_NOT_RENDER"));
    assert!(!html.contains("name=\"credential_value\""));
    let revision = connections::list(&pool, UserId::new(owner)).await.unwrap()[0].revision;
    let revoke = format!("/connections/{}/revoke", account.0);
    assert_eq!(
        post_form(&app, &cookie, &revoke, format!("revision={revision}"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &revoke,
            format!("revision={revision}&confirmed=true")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let (_, html) = get_page(
        &app,
        &cookie,
        &format!("/work/surfaces/{}", route_id(surface)),
    )
    .await;
    assert!(html.contains("<strong>revoked</strong>"));
    let (_, hub) = get_page(&app, &cookie, "/connections?sync=pending").await;
    assert!(hub.contains("Connection change saved"));
    let invalid = post_form(&app, &cookie, "/connections/create", "name=Preserved+account&provider=github_app&installation=invalid&secret=NEVER_ECHO_THIS_SECRET".into()).await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let html = String::from_utf8(
        invalid
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("value=\"Preserved account\""));
    assert!(!html.contains("NEVER_ECHO_THIS_SECRET"));
    let detach = format!("/connections/repositories/{surface}/detach");
    assert_eq!(
        post_form(&app, &cookie, &detach, String::new())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(&app, &cookie, &detach, "confirmed=true".into())
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let (_, html) = get_page(
        &app,
        &cookie,
        &format!("/work/surfaces/{}", route_id(surface)),
    )
    .await;
    assert!(html.contains("No account or local credential configured"));
}

#[tokio::test]
async fn cabinet_move_preview_inherits_destination_audience_without_changing_membership() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, _, _, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    let scope = den_cabinet::ActorScope::user(UserId::new(owner));
    let parent = knowledge::page(&pool, owner, "Restricted parent", "Public-safe setup").await;
    cabinet::pages::configure(
        &pool,
        &scope,
        &parent,
        den_cabinet::CabinetPolicy {
            bears_may_write: true,
            review_required: false,
            allowed_kinds: None,
        },
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let page = knowledge::page(&pool, owner, "Move fixture", "Readable document").await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let (status, preview) = get_page(
        &app,
        &cookie,
        &format!("/cabinet/{page}/move?parent={parent}&position=0"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(preview.contains("Restricted by this page and its ancestors"));
    assert!(preview.contains("Move fixture"));
    assert!(preview.contains("Restricted parent"));
    let (before, after) = preview.split_once("After-move audience").unwrap();
    assert!(before.contains("Current audience") && before.contains("Open wiki"));
    assert!(after.contains("Restricted by this page and its ancestors"));
    assert!(preview.contains("name=\"confirm_audience\" value=\"true\" required"));
    assert!(cabinet::pages::metadata(&pool, &scope, &page)
        .await
        .unwrap()
        .parent
        .is_none());
    assert_eq!(
        get_page(&app, &peer_cookie, &format!("/cabinet/{page}"))
            .await
            .0,
        StatusCode::OK
    );
    let action = format!("/cabinet/{page}/organize");
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &action,
            format!("parent={parent}&position=0")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &action,
            format!("parent={parent}&position=0&confirm_audience=true")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        get_page(&app, &peer_cookie, &format!("/cabinet/{page}"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let metadata = cabinet::pages::metadata(&pool, &scope, &page)
        .await
        .unwrap();
    assert!(
        metadata.users.is_empty() && metadata.bears.is_empty(),
        "preview/move must not rewrite membership"
    );
    let (_, preview) = get_page(
        &app,
        &cookie,
        &format!("/cabinet/{page}/move?parent=&position=0"),
    )
    .await;
    let (before, after) = preview.split_once("After-move audience").unwrap();
    assert!(before.contains("Restricted by this page and its ancestors"));
    assert!(after.contains("Open wiki"));
    let (status, _) = get_page(
        &app,
        &peer_cookie,
        &format!("/cabinet/{page}/move?parent=&position=0"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
