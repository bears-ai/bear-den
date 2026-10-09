use super::*;
use den_core::tools::repository::RepositorySurfaceId;
use den_service::{connections, repository::grants, work_surfaces};

#[sqlx::test(migrations = "../../migrations")]
async fn external_reference_and_exact_grant_forms_preserve_scope_and_missing_backend_state(
    pool: PgPool,
) {
    let compact = Uuid::new_v4().simple().to_string();
    let suffix = &compact[..12];
    let slug = format!("repoui{suffix}");
    let admin_name = format!("repoadmin{suffix}");
    let member_name = format!("repomember{suffix}");
    for name in [&admin_name, &member_name] {
        assert!(name.len() <= 30 && name.bytes().all(|ch| ch.is_ascii_alphanumeric()));
    }
    let bear = new_bear(&pool, &slug).await;
    let admin = user(&pool, bear, &admin_name, BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, &member_name, BEAR_ROLE_MEMBER).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Repo",
        "Bounded reads",
    )
    .await
    .unwrap();
    let app = app(&pool, config()).await;
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let backend = Uuid::new_v4();
    let reference = Uuid::new_v4();
    let form = format!("name=Inactive+reference&provider=github_external&backend_binding_id={backend}&external_secret_id={reference}&external_secret_version=1");
    let unconfirmed = request(&app, &admin_cookie, "POST", "/connections/create", &form).await;
    assert_eq!(unconfirmed.0, StatusCode::BAD_REQUEST);
    assert!(unconfirmed
        .1
        .contains("Confirm the unconfigured external-backend boundary"));
    assert!(connections::list(&pool, UserId::new(admin))
        .await
        .unwrap()
        .is_empty());
    let created = request(
        &app,
        &admin_cookie,
        "POST",
        "/connections/create",
        &format!("{form}&confirm_external_boundary=true"),
    )
    .await;
    let safe_feedback: String = created
        .1
        .replace(&backend.to_string(), "[redacted]")
        .replace(&reference.to_string(), "[redacted]")
        .chars()
        .take(800)
        .collect();
    assert_eq!(
        created.0,
        StatusCode::SEE_OTHER,
        "external-reference create response: {safe_feedback}"
    );
    let connection = connections::list(&pool, UserId::new(admin))
        .await
        .unwrap()
        .remove(0);
    let metadata = serde_json::to_string(&connection).unwrap();
    assert!(!metadata.contains(&backend.to_string()));
    assert!(!metadata.contains(&reference.to_string()));
    let surface = work_surfaces::create_surface(
        &pool,
        admin,
        NewWorkSurface {
            name: format!("repoui{suffix}"),
            description: None,
            upstream_url: "https://github.com/acme/widget".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec!["api.github.com".into()],
            credential: None,
        },
        "unused",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(&pool, surface.id, bear, admin)
        .await
        .unwrap();
    hats::manage::replace_surfaces(&pool, BearId::new(bear), hat.id, &[surface.id])
        .await
        .unwrap();
    connections::attach(&pool, UserId::new(admin), connection.id, surface.id)
        .await
        .unwrap();
    let choice = grants::choices(&pool, BearId::new(bear), hat.id, UserId::new(admin))
        .await
        .unwrap()
        .remove(0);
    let path = format!("/bear/{slug}/hats/{}/repository-access", hat.id);
    let grant = format!(
        "action=grant&surface_id={}&expected_target={}",
        surface.id, choice.target_key
    );
    assert_ne!(
        request(&app, &admin_cookie, "POST", &path, &grant).await.0,
        StatusCode::SEE_OTHER
    );
    assert!(grants::list(&pool, BearId::new(bear), hat.id)
        .await
        .unwrap()
        .is_empty());
    assert_ne!(
        request(
            &app,
            &member_cookie,
            "POST",
            &path,
            &format!("{grant}&confirm_future_job_audience=true")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &path,
            &format!("{grant}&confirm_future_job_audience=true")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let detail = request(
        &app,
        &admin_cookie,
        "GET",
        &format!("/bear/{slug}/hats/{}", hat.id),
        "",
    )
    .await;
    assert_eq!(detail.0, StatusCode::OK);
    assert!(detail.1.contains("repository_head is unavailable"));
    assert!(!detail.1.contains(&backend.to_string()));
    assert!(!detail.1.contains(&reference.to_string()));
    sqlx::query!(
        "UPDATE git_work_surface_details SET default_ref='changed' WHERE id=$1",
        surface.id
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(grants::grant(
        &pool,
        BearId::new(bear),
        hat.id,
        UserId::new(admin),
        RepositorySurfaceId(surface.id),
        &choice.target_key,
        true
    )
    .await
    .is_err());
    let stored = grants::list(&pool, BearId::new(bear), hat.id)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &path,
            &format!("action=revoke&grant_id={}", stored[0].id)
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert!(grants::list(&pool, BearId::new(bear), hat.id)
        .await
        .unwrap()
        .is_empty());
}
