use super::*;
use crate::admin::usability_tests::render;
use den_http::user::db::deletion::UserDeletionBear;

#[test]
fn delete_templates_render_real_parents_and_no_javascript_confirmation_or_unsafe_metadata() {
    let target = UserId::new(42);
    let atlas = BearId::new(Uuid::nil());
    let html = render(
        "admin/users/delete.html",
        context! {
            preview => UserDeletionPreview {
                user_id: target,
                username: "casey<script>".into(),
                bears: vec![UserDeletionBear {
                    bear_id: atlas, name: "Private Atlas<script>".into(), is_admin: true, last_admin: true,
                }],
            },
            confirmation_token => Uuid::new_v4(), confirmed => true,
            error => "Grant another person Admin access.",
        },
    );
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("/assets/css/style.css"));
    assert!(html.contains("name=\"viewport\""));
    assert!(html.contains("aria-label=\"Operator navigation\""));
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("User was not deleted."));
    assert!(html.contains("Deletion blocked: this user is the only Admin."));
    assert!(html.contains("href=\"/admin/membership/grant\""));
    assert!(html.contains("href=\"/admin/users/42\""));
    assert!(html.contains("method=\"post\" action=\"/admin/users/42/delete\""));
    assert!(html.contains("name=\"confirm_delete\" value=\"true\" required checked"));
    assert!(html.contains("name=\"confirmation_token\""));
    assert!(html.contains("casey&lt;script&gt;"));
    assert!(html.contains("Private Atlas&lt;script&gt;"));
    assert!(!html.contains("onsubmit="));
    assert!(!html.contains("confirm('"));
    let view = render(
        "admin/users/view.html",
        context! { id => 42, user => context! { username => "casey" }, invites => Vec::<minijinja::Value>::new() },
    );
    assert!(view.contains("href=\"/admin/users/42/delete\""));
    assert!(!view.contains("action=\"/admin/users/42/delete\""));
}

#[test]
fn confirmation_requires_matching_target_token_and_unexpired_server_time() {
    let confirmation = DeleteConfirmation {
        user_id: UserId::new(42),
        token: Uuid::new_v4(),
        issued_at: 1000,
    };
    assert!(confirmation.is_fresh(UserId::new(42), Some(confirmation.token), 1000));
    assert!(confirmation.is_fresh(UserId::new(42), Some(confirmation.token), 1900));
    for now in [999, 1901] {
        assert!(!confirmation.is_fresh(UserId::new(42), Some(confirmation.token), now));
    }
    assert!(!confirmation.is_fresh(UserId::new(43), Some(confirmation.token), 1000));
    assert!(!confirmation.is_fresh(UserId::new(42), None, 1000));
    assert!(!confirmation.is_fresh(UserId::new(42), Some(Uuid::new_v4()), 1000));
}

#[sqlx::test(migrations = "../../migrations")]
async fn real_parent_routes_reject_anonymous_members_bear_admins_and_username_permission_collision(
    pool: PgPool,
) {
    let target = user(&pool, "private-delete-target", false).await;
    let member = user(&pool, "private-delete-member", false).await;
    let bear_admin = user(&pool, "private-delete-admin", false).await;
    // Backend legacy username permissions must not substitute for the operator flag.
    let named_admin = user(&pool, "admin", false).await;
    let atlas = bear(&pool, "private-delete-atlas", "Secret Deletion Atlas").await;
    grant(&pool, target, atlas, "admin").await;
    grant(&pool, member, atlas, "member").await;
    grant(&pool, bear_admin, atlas, "admin").await;
    let app = app(&pool).await;
    let cookies = [
        String::new(),
        login(&app.router, &pool, member).await,
        login(&app.router, &pool, bear_admin).await,
        login(&app.router, &pool, named_admin).await,
    ];
    for cookie in cookies {
        for target_id in [target, UserId::new(i32::MAX)] {
            let path = format!("/admin/users/{target_id}/delete");
            for form in [None, Some("confirm_delete=true"), Some("")] {
                let response = request(&app.router, &cookie, &path, form).await;
                assert!(!response.status().is_success());
                let page = html(response).await;
                for metadata in [
                    "Secret Deletion Atlas",
                    "private-delete-target",
                    "Historical records",
                    "Affected Bears",
                ] {
                    assert!(
                        !page.contains(metadata),
                        "unauthorized disclosure: {metadata}"
                    );
                }
            }
        }
    }
    assert!(exists(&pool, target).await);
    assert_eq!(admins(&pool, atlas).await, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn explicit_single_use_target_bound_confirmation_required_and_success_redirect_preserved(
    pool: PgPool,
) {
    let operator = user(&pool, "confirm-delete-operator", true).await;
    let target = user(&pool, "confirm-delete-target", false).await;
    let other = user(&pool, "confirm-delete-other", false).await;
    let app = app(&pool).await;
    let cookie = login(&app.router, &pool, operator).await;
    let path = format!("/admin/users/{target}/delete");
    let response = request(&app.router, &cookie, &path, Some("")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let page = html(response).await;
    assert!(page.contains("Confirm deletion on this page"));
    assert!(page.contains("action=\"/admin/users/"));
    assert!(exists(&pool, target).await);

    let (_, first_token) = preview(&app.router, &cookie, target).await;
    assert!(exists(&pool, target).await); // GET is always non-destructive.
    let other_session = login(&app.router, &pool, operator).await;
    let response = request(
        &app.router,
        &other_session,
        &path,
        Some(&confirmation(first_token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(exists(&pool, target).await);
    let response = request(
        &app.router,
        &cookie,
        &path,
        Some(&format!("confirmation_token={first_token}")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(exists(&pool, target).await);
    // Even a failed attempt consumes the old token.
    let response = request(
        &app.router,
        &cookie,
        &path,
        Some(&confirmation(first_token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let page = html(response).await;
    assert!(page.contains("required checked"));
    assert!(exists(&pool, target).await);

    let (_, other_token) = preview(&app.router, &cookie, other).await;
    let response = request(
        &app.router,
        &cookie,
        &path,
        Some(&confirmation(other_token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(exists(&pool, target).await);
    assert!(exists(&pool, other).await);

    let (_, token) = preview(&app.router, &cookie, target).await;
    let response = request(&app.router, &cookie, &path, Some(&confirmation(token))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/admin/users/");
    assert!(!exists(&pool, target).await);
    let response = request(&app.router, &cookie, &path, None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fresh_post_rechecks_stale_preview_keeps_form_and_requires_explicit_membership_handoff(
    pool: PgPool,
) {
    let operator = user(&pool, "handoff-delete-operator", true).await;
    let target = user(&pool, "handoff-delete-target", false).await;
    let next = user(&pool, "handoff-delete-next", false).await;
    let uninvolved = user(&pool, "handoff-delete-uninvolved", false).await;
    let atlas = bear(&pool, "handoff-delete-atlas", "Affected Atlas").await;
    for user in [target, next] {
        grant(&pool, user, atlas, "admin").await;
    }
    grant(&pool, uninvolved, atlas, "member").await;
    let app = app(&pool).await;
    let cookie = login(&app.router, &pool, operator).await;
    let path = format!("/admin/users/{target}/delete");
    let (page, stale_token) = preview(&app.router, &cookie, target).await;
    assert!(page.contains("Affected Atlas"));
    assert!(!page.contains("Deletion blocked: this user is the only Admin."));
    bears_db::revoke_membership(&pool, next.get(), atlas.as_uuid())
        .await
        .unwrap();
    let response = request(
        &app.router,
        &cookie,
        &path,
        Some(&confirmation(stale_token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let page = html(response).await;
    assert!(page.contains("User was not deleted."));
    assert!(page.contains("Affected Atlas"));
    assert!(page.contains("only Admin"));
    assert!(page.contains("required checked"));
    assert!(page.contains("href=\"/admin/membership/grant\""));
    assert!(page.contains("Keep account and return to user"));
    assert_ne!(token(&page), stale_token);
    assert!(exists(&pool, target).await);
    assert_eq!(
        role(&pool, uninvolved, atlas).await,
        Some(Some("member".into()))
    );
    assert_eq!(role(&pool, operator, atlas).await, None);
    assert_eq!(role(&pool, next, atlas).await, None);

    // The linked canonical membership form is the only place this operator grants a replacement.
    let form = format!("user_id={next}&bear_id={atlas}&role=admin");
    let response = request(&app.router, &cookie, "/admin/membership/grant", Some(&form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, fresh_token) = preview(&app.router, &cookie, target).await;
    let response = request(
        &app.router,
        &cookie,
        &path,
        Some(&confirmation(fresh_token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(admins(&pool, atlas).await, 1);
    assert_eq!(role(&pool, next, atlas).await, Some(Some("admin".into())));
    assert_eq!(
        role(&pool, uninvolved, atlas).await,
        Some(Some("member".into()))
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn operator_historical_fk_failure_keeps_form_and_account_without_leaking_database_details(
    pool: PgPool,
) {
    let operator = user(&pool, "fk-delete-operator", true).await;
    let target = user(&pool, "fk-delete-account", false).await;
    let next = user(&pool, "fk-delete-next", false).await;
    let atlas = bear(&pool, "fk-route-atlas", "Historical Atlas").await;
    for user in [target, next] {
        grant(&pool, user, atlas, "admin").await;
    }
    den_http::user::invites::db::create(&pool, target.get(), "historical-delete-invite")
        .await
        .unwrap();
    let app = app(&pool).await;
    let cookie = login(&app.router, &pool, operator).await;
    let (_, token) = preview(&app.router, &cookie, target).await;
    let response = request(
        &app.router,
        &cookie,
        &format!("/admin/users/{target}/delete"),
        Some(&confirmation(token)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let page = html(response).await;
    for message in [
        "User was not deleted.",
        "Historical records still refer to this account.",
        "handing off Bear Admin access alone does not remove those references.",
        "No changes were made.",
        "required checked",
        "Historical Atlas",
    ] {
        assert!(page.contains(message), "missing {message}");
    }
    assert!(page.contains("href=\"/admin/membership/\""));
    assert!(page.contains(&format!("href=\"/admin/users/{target}\"")));
    for internal in [
        "invites_user_id_fkey",
        "23503",
        "DELETE FROM",
        "historical-delete-invite",
        "passhash",
    ] {
        assert!(!page.contains(internal));
    }
    assert!(exists(&pool, target).await);
    assert_eq!(admins(&pool, atlas).await, 2);
    assert_eq!(
        den_http::user::invites::db::by_user_id(&pool, target.get())
            .await
            .unwrap()
            .len(),
        1
    );
}
