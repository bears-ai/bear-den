//! Exercise real authenticated routes and canonical services with the full template parent.
use super::*;
use den_service::{bears::model_configurations, skills};

#[path = "recovery_tests.rs"]
mod recovery_tests;

async fn new_bear(pool: &PgPool, slug: &str) -> Uuid {
    bears_db::create_bear(
        pool,
        BearParams {
            slug,
            name: "Management test",
            description: "Purpose",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

async fn app(pool: &PgPool, config: Config) -> Router {
    app_with_state(pool, config).await.0
}

async fn app_with_state(pool: &PgPool, config: Config) -> (Router, AppState) {
    let config = Arc::new(config);
    let state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        config,
    );
    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await.unwrap();
    let app = Router::new()
        .merge(crate::bear::management::router())
        .merge(crate::bear::manage::router())
        .merge(crate::bear::skills::router())
        .route("/test-login/{user_id}", get(login))
        .route(
            "/test-review-draft/{bear}/{hat}/{actor}",
            get(recovery_tests::inspect_draft_scope),
        )
        .with_state(state.clone())
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool.clone()),
                axum_login::tower_sessions::SessionManagerLayer::new(sessions),
            )
            .build(),
        );
    (app, state)
}

fn escaped_html(value: &str) -> String {
    minijinja::Environment::new()
        .render_str("{{ value | e }}", minijinja::context! { value })
        .unwrap()
}

fn config() -> Config {
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("management-ui-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    config
}

#[sqlx::test(migrations = "../../migrations")]
async fn management_controls_preserve_authorization_and_canonical_skill_uses(pool: PgPool) {
    let bear = new_bear(&pool, "usabilitybear").await;
    let admin = user(&pool, bear, "usabilityadmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "usabilitymember", BEAR_ROLE_MEMBER).await;
    let other = new_bear(&pool, "otherusabilitybear").await;
    let outsider = user(&pool, other, "usabilityoutsider", BEAR_ROLE_MEMBER).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Engineering",
        "Review changes",
    )
    .await
    .unwrap();
    let app = app(&pool, config()).await;
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let outsider_cookie = cookie(&app, outsider).await;
    let hat_path = format!("/bear/usabilitybear/hats/{}", hat.id);
    let (status, page, _) = request(&app, &admin_cookie, "GET", &hat_path, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("<!doctype html>"));
    assert!(page.contains("Work is blocked: no repositories are permitted"));
    assert!(page.contains("#hat-resources"));
    assert_eq!(
        request(&app, &member_cookie, "GET", &hat_path, "").await.0,
        StatusCode::FORBIDDEN
    );
    assert!(!request(&app, &outsider_cookie, "GET", &hat_path, "")
        .await
        .0
        .is_success());
    let identity =
        hats::identity::identity_fingerprint(&hat.name, &hat.purpose, &hat.identity_prompt);
    let body = format!("action=enable&confirmation=enable+work&confirm_identity_audience=true&expected_identity_sha256={identity}");
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "POST",
            &format!("{hat_path}/work"),
            &body
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{hat_path}/work"),
            &body
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        !manage::get_hat(&pool, BearId::new(bear), hat.id)
            .await
            .unwrap()
            .work_enabled
    );
    let (status, page, _) = request(
        &app,
        &admin_cookie,
        "GET",
        &format!(
            "{hat_path}/work-review?page=2&expected_sha256=stale&expected_identity_sha256=stale"
        ),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("Hat memory or identity changed during review"));
    assert!(page.contains("Restart review at page 1"));

    let draft = skills::create_draft(
        &pool,
        UserId::new(admin),
        "Procedure",
        "1",
        "Reviewed procedure",
        "Procedure content",
    )
    .await
    .unwrap();
    let checksum = skills::hash("Procedure content");
    skills::approve(&pool, UserId::new(admin), draft, &checksum, true)
        .await
        .unwrap();
    skills::attach(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        draft,
        &checksum,
        &[den_core::RuntimeContextLabel::ChannelConversation],
        false,
    )
    .await
    .unwrap();
    let skill_path = format!("/bear/usabilitybear/skills/{}", draft.0);
    let (status, page, _) =
        request(&app, &admin_cookie, "GET", "/bear/usabilitybear/skills", "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Current permitted uses:</strong> Browser Chat"));
    assert!(page.contains("value=\"chat\" checked"));
    assert!(page.contains("Save permitted uses"));
    let update = format!("operation=attach&content_hash={checksum}&profiles=pair&profiles=work");
    assert_eq!(
        request(&app, &member_cookie, "POST", &skill_path, &update)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, page, _) = request(&app, &admin_cookie, "POST", &skill_path, &update).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("acknowledge autonomous Work"));
    assert!(page.contains("value=\"pair\" checked"));
    assert!(page.contains("Current permitted uses:</strong> Browser Chat"));
    assert!(skills::effective(
        &pool,
        BearId::new(bear),
        den_core::RuntimeContextLabel::JobRun
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &skill_path,
            &format!("{update}&confirm_work=true")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let canonical = skills::list(&pool, BearId::new(bear), UserId::new(admin))
        .await
        .unwrap();
    let attached = canonical.iter().find(|item| item.id == draft).unwrap();
    assert_eq!(attached.profiles, ["pair", "work"]);
    assert!(skills::effective(
        &pool,
        BearId::new(bear),
        den_core::RuntimeContextLabel::ChannelConversation
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(
        skills::effective(
            &pool,
            BearId::new(bear),
            den_core::RuntimeContextLabel::JobRun
        )
        .await
        .unwrap()
        .len(),
        1
    );
    let (status, page, _) = request(
        &app,
        &member_cookie,
        "GET",
        "/bear/usabilitybear/skills",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Current permitted uses:</strong> Editor, Autonomous Work"));
    assert!(!page.contains("Save permitted uses"));
    assert!(!page.contains("<form"));

    let model =
        super::super::super::settings::model_configurations::selectable_model_options(&pool)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("migrations seed selectable models");
    let named = model_configurations::create(
        &pool,
        BearId::new(bear),
        "Named default",
        &model.handle,
        None,
    )
    .await
    .unwrap();
    model_configurations::set_default(&pool, BearId::new(bear), Some(named.id))
        .await
        .unwrap();
    for viewer in [&admin_cookie, &member_cookie] {
        let (status, page, _) =
            request(&app, viewer, "GET", "/bear/usabilitybear/identity", "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.contains("Named default"));
        assert!(page.contains("Bear default"));
        assert!(page.contains(&format!("<code>{}</code>", escaped_html(&model.handle))));
        assert!(page.contains(">Models</a>"));
    }
    for viewer in [&admin_cookie, &member_cookie] {
        let (status, page, _) =
            request(&app, viewer, "GET", "/bear/usabilitybear/models", "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.contains("Named default"));
        assert!(page.contains("No Bifrost virtual key is configured"));
        assert_eq!(page.contains("Save Bear default"), viewer == &admin_cookie);
        assert_eq!(
            page.contains("Advanced settings: loop budgets"),
            viewer == &admin_cookie
        );
    }
    model_configurations::set_default(&pool, BearId::new(bear), None)
        .await
        .unwrap();
    let (status, page, _) = request(
        &app,
        &member_cookie,
        "GET",
        "/bear/usabilitybear/identity",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Deployment default"));
    assert!(!page.contains("Named default"));
    let (status, _, location) = request(
        &app,
        &admin_cookie,
        "GET",
        "/bear/usabilitybear/edit/configuration",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/bear/usabilitybear/models"));
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "GET",
            "/bear/usabilitybear/edit/configuration",
            ""
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn saved_editor_failure_posts_to_renamed_bear_and_preserves_steering(pool: PgPool) {
    let bear = new_bear(&pool, "beforeedit").await;
    let admin = user(&pool, bear, "savededitoradmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear, "savededitormember", BEAR_ROLE_MEMBER).await;
    let mut config = config();
    let blocked_path = std::path::PathBuf::from(&config.bear_sqlite_data_dir);
    std::fs::write(&blocked_path, "not a directory").unwrap();
    config.bear_sqlite_data_dir = blocked_path.to_string_lossy().into_owned();
    let app = app(&pool, config).await;
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let edit = "slug=afteredit&name=Saved+name&description=Saved+purpose";
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "POST",
            "/bear/beforeedit/edit/overview",
            edit
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, page, _) = request(
        &app,
        &admin_cookie,
        "POST",
        "/bear/beforeedit/edit/overview",
        edit,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Saved, but Bear initialization failed"));
    assert!(page.contains("action=\"/bear/afteredit/edit/overview\""));
    assert!(page.contains("Saved name"));
    assert!(page.contains("href=\"/bear/afteredit/hats\""));
    assert!(page.contains("href=\"/bear/afteredit/activity\""));
    assert!(page.contains(">Diagnostics</a>"));
    let saved = bears_db::get_bear(&pool, bear).await.unwrap().unwrap();
    assert_eq!(saved.slug, "afteredit");
    let (status, page, _) = request(
        &app,
        &admin_cookie,
        "POST",
        "/bear/afteredit/edit/prompt",
        "system_prompt=Saved+%3Csteering%3E",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Saved, but Bear initialization failed"));
    assert!(page.contains("Saved &lt;steering&gt;"));
    assert!(page.contains("action=\"/bear/afteredit/edit/prompt\""));
    assert!(page.contains("href=\"/bear/afteredit/hats\""));
    assert!(page.contains("href=\"/bear/afteredit/activity\""));
    assert!(page.contains(">Diagnostics</a>"));
    assert_eq!(
        bears_db::get_bear(&pool, bear)
            .await
            .unwrap()
            .unwrap()
            .system_prompt,
        "Saved <steering>"
    );
    std::fs::remove_file(blocked_path).unwrap();
}
