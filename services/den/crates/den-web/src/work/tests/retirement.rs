//! Real HTTP confirmation/session flows, including the common retirement → hard-delete case.
use super::*;
use den_cabinet::{ActorScope, ReadRequest};
use den_core::{BearId, UserId};
use den_service::{artifacts, cabinet};
use serde_json::json;

async fn full_app(pool: sqlx::PgPool) -> axum::Router {
    let state = test_state(pool.clone());
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    Router::new()
        .merge(router())
        .merge(crate::cabinet::router())
        .merge(crate::bear::management::router())
        .merge(crate::connections::router())
        .merge(crate::management_hub::router())
        .nest("/bear/{bear_slug}", docket_router())
        .route("/test-login/{user_id}", get(test_login))
        .with_state(state)
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool),
                axum_login::tower_sessions::SessionManagerLayer::new(store),
            )
            .build(),
        )
}
async fn saved_copy(pool: &sqlx::PgPool, owner: i32, bear: Uuid) -> (artifacts::ArtifactRef, Uuid) {
    let page = knowledge::page(pool, owner, "PRIVATE COPY <script>", "Immutable copy body").await;
    let view = cabinet::read(
        pool,
        ReadRequest {
            scope: ActorScope::user(UserId::new(owner)),
            cabinet_ref: page.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    let job=sqlx::query_scalar!("INSERT INTO bear_jobs(bear_id,created_by_user_id,created_by_role,goal,lifecycle_intent,visibility) VALUES($1,$2,'ui','Copy Job','cancelled','same_user') RETURNING id",bear,owner).fetch_one(pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let reference = cabinet::snapshots::capture_in_tx(
        &mut tx,
        pool,
        &ActorScope::user(UserId::new(owner)),
        BearId::new(bear),
        &page,
        view.version.version_ref(),
    )
    .await
    .unwrap();
    artifacts::attach_docket_artifact_in_tx(
        &mut tx,
        artifacts::AttachDocketArtifactInput {
            artifact_ref: reference.as_str().into(),
            bear_id: bear,
            target_kind: artifacts::DocketArtifactTargetKind::Job,
            target_id: job,
            role: artifacts::DocketArtifactRole::Source,
            metadata: json!({}),
            created_by_user_id: Some(owner),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (reference, job)
}
fn hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    html.split_once(&marker)
        .unwrap_or_else(|| panic!("Missing {name} input"))
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn retirement_body(html: &str) -> String {
    format!(
        "expected={}&token={}&reason=Completed+copy+no+longer+needed&acknowledged=true",
        hidden(html, "expected"),
        hidden(html, "token")
    )
}
fn delete_body(html: &str, slug: &str) -> String {
    format!(
        "expected={}&token={}&confirm_slug={slug}&acknowledged=true",
        hidden(html, "expected"),
        hidden(html, "token")
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn browser_retirement_resolves_simple_bear_delete_with_separate_fresh_confirmation(
    pool: sqlx::PgPool,
) {
    let (owner, bear, slug, _) = seed_member(&pool).await;
    usability::verify_fixture_user(&pool, owner).await;
    let (reference, job) = saved_copy(&pool, owner, bear).await;
    let app = full_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let retirement_url = format!(
        "/cabinet/saved-copies/{reference}/retire",
        reference = reference.as_str()
    );
    let delete_url = format!("/bear/{slug}/delete");
    let (status, blocked) = get_page(&app, &cookie, &delete_url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(blocked.contains("Review retirement"));
    assert!(!blocked.contains("name=\"confirm_slug\""));
    let (_, preview) = get_page(&app, &cookie, &retirement_url).await;
    assert!(preview.contains("<!doctype html>"));
    assert!(preview.contains("PRIVATE COPY &lt;script&gt;"));
    assert!(preview.contains("not physical erasure"));
    assert!(!preview.contains("storage_key"));
    let response = post_form(&app, &cookie, &retirement_url, retirement_body(&preview)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, receipt) = get_page(&app, &cookie, &retirement_url).await;
    assert!(receipt.contains("Retired"));
    assert!(!receipt.contains("Download saved copy"));
    assert!(!receipt.contains("name=\"acknowledged\""));
    let content_url = format!("/cabinet/saved-copies/{}/content", reference.as_str());
    assert_eq!(
        get_page(&app, &cookie, &content_url).await.0,
        StatusCode::NOT_FOUND
    );
    let old_job_content = format!(
        "/bear/{slug}/jobs/{}/evidence/{}/content",
        route_id(job),
        reference.as_str()
    );
    assert_eq!(
        get_page(&app, &cookie, &old_job_content).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, history) = get_page(&app, &cookie, "/cabinet/saved-copies").await;
    assert!(history.contains("View retirement receipt"));
    assert!(!history.contains("Download saved copy"));
    let (_, delete) = get_page(&app, &cookie, &delete_url).await;
    assert!(delete.contains("name=\"confirm_slug\""));
    assert!(delete.contains("eligible retired snapshot audit"));
    let response = post_form(&app, &cookie, &delete_url, delete_body(&delete, &slug)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(bears_db::get_bear(&pool, bear).await.unwrap().is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_admin_and_nonmember_never_borrow_private_copy_or_blocker_metadata(
    pool: sqlx::PgPool,
) {
    let (owner, bear, slug, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    let (outsider, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("admin"))
        .await
        .unwrap();
    usability::verify_fixture_user(&pool, peer).await;
    let (reference, _) = saved_copy(&pool, owner, bear).await;
    let app = full_app(pool.clone()).await;
    for user in [peer, outsider] {
        let cookie = login_cookie(&app, user).await;
        let (_, html) = get_page(&app, &cookie, "/cabinet/saved-copies").await;
        assert!(!html.contains("PRIVATE COPY"));
        assert!(!html.contains(reference.as_str()));
        for suffix in ["retire", "content"] {
            let url = format!("/cabinet/saved-copies/{}/{suffix}", reference.as_str());
            let (status, html) = get_page(&app, &cookie, &url).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(!html.contains("PRIVATE COPY"));
        }
    }
    let peer_cookie = login_cookie(&app, peer).await;
    let (_, blocked) = get_page(&app, &peer_cookie, &format!("/bear/{slug}/delete")).await;
    assert!(blocked.contains("handled by their creator"));
    assert!(!blocked.contains("PRIVATE COPY"));
    assert!(!blocked.contains(reference.as_str()));
    assert!(!blocked.contains("name=\"confirm_slug\""));
}

#[sqlx::test(migrations = "../../migrations")]
async fn stale_inventory_replay_and_missing_acknowledgement_preserve_evidence(pool: sqlx::PgPool) {
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (reference, job) = saved_copy(&pool, owner, bear).await;
    let app = full_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let url = format!("/cabinet/saved-copies/{}/retire", reference.as_str());
    let (_, preview) = get_page(&app, &cookie, &url).await;
    sqlx::query!(
        "UPDATE bear_jobs SET goal='Changed after preview' WHERE id=$1",
        job
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = post_form(&app, &cookie, &url, retirement_body(&preview)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
        .into_owned();
    assert!(html.contains("requirements changed"));
    assert!(html.contains("Completed copy no longer needed"));
    assert!(!html.contains("required checked"));
    assert!(artifacts::json_content_for_reader(
        &pool,
        &reference,
        artifacts::ArtifactReader::Human(UserId::new(owner))
    )
    .await
    .is_ok());
    let missing = retirement_body(&html).replace("&acknowledged=true", "");
    let response = post_form(&app, &cookie, &url, missing).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(artifacts::json_content_for_reader(
        &pool,
        &reference,
        artifacts::ArtifactReader::Human(UserId::new(owner))
    )
    .await
    .is_ok());
}

#[test]
fn full_parent_templates_show_consequences_and_never_render_retired_downloads() {
    let copy = json!({"reference":"artifact_00000000000000000000000000000000","title":"Copy <script>","bear_name":"Atlas","retired":true,"readable":false});
    let history = usability::render(
        "cabinet/saved_copies.html",
        json!({"history":{"copies":[copy],"next":null}}),
    );
    assert!(history.contains("Copy &lt;script&gt;"));
    assert!(!history.contains("Download saved copy"));
    let reason = "<script>reason</script>";
    let preview = usability::render(
        "cabinet/retire_snapshot.html",
        json!({"view":{"reference":"artifact_00000000000000000000000000000000","title":"Copy","bear_name":"Atlas","bear_slug":"atlas","fingerprint":"hash"},"token":"token","can_retire":true,"reason":reason}),
    );
    assert!(preview.contains("preserves audit"));
    let escaped_reason = minijinja::Environment::new()
        .render_str("{{ reason | e }}", minijinja::context! { reason })
        .unwrap();
    assert!(preview.contains(&escaped_reason));
    assert!(!preview.contains(reason));
    assert!(!preview.contains("onsubmit="));
    let delete = usability::render(
        "bear/delete.html",
        json!({"bear":{"slug":"atlas","name":"Atlas"},"preview":{"can_delete":false},"blockers":["Private evidence must be handled by its creator"],"copies":{"copies":[]}}),
    );
    assert!(delete.contains("separate destructive action"));
    assert!(!delete.contains("name=\"confirm_slug\""));
    assert!(!delete.contains("artifact_"));
}
