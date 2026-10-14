//! Exercise the real Cabinet router/login boundary, not a test-only login route.

use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use den_cabinet::{ActorScope, CreateItemRequest, HistoryRequest, ItemKind};
use den_core::ids::UserId;
use den_service::cabinet;
use tower::ServiceExt;

async fn request(
    app: &Router,
    path: &str,
    cookie: Option<&str>,
    form: Option<&str>,
    origin: Option<&str>,
) -> Response {
    let mut request = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    let body = if let Some(form) = form {
        request = request
            .method(Method::POST)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        Body::from(form.to_owned())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
    .replace("&#x2f;", "/")
}

#[sqlx::test(migrations = "../../migrations")]
async fn dedicated_login_discussion_history_and_den_handoff_share_one_authority(pool: PgPool) {
    let hash = password_auth::generate_hash("surface-password");
    let user = sqlx::query_scalar!(
        "INSERT INTO users (username, email, display_name, passhash, is_admin) VALUES ($1, $2, $1, $3, $4) RETURNING id",
        "surfacehuman", "surfacehuman@example.invalid", hash, false,
    ).fetch_one(&pool).await.unwrap();
    let scope = ActorScope::user(UserId::new(user));
    let page = cabinet::create_item(
        &pool,
        CreateItemRequest {
            scope: scope.clone(),
            kind: ItemKind::Document,
            title: "Shared release page".into(),
            content: "Original publication".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap();
    let reference = &page.item.cabinet_ref;
    let mut config = Config::test_stub();
    config.cabinet_web_origin = Some("http://cabinet.test".parse().unwrap());
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let state = cabinet_state(pool.clone(), Arc::new(config)).unwrap();
    assert!(!state.has_native_runtime());
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    let app = cabinet_router(state.clone(), store);

    let login = request(
        &app,
        "/login/password",
        None,
        Some("username=surfacehuman&password=surface-password&next=%2Fcabinet"),
        Some("http://cabinet.test"),
    )
    .await;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert_eq!(login.headers()[header::LOCATION], "/cabinet");
    let set_cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("cabinet_session="));
    assert!(!set_cookie.contains("Domain="));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();

    let page_path = format!("/cabinet/{reference}");
    let response = request(&app, &page_path, Some(&cookie), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = html(response).await;
    assert!(body.contains("Cabinet navigation"));
    assert!(body.contains("Shared release page"));
    assert!(body.contains(&format!("{page_path}/discussion")));
    assert!(body.contains(&format!("{page_path}/history")));
    assert!(!body.contains("Den management"));

    let discussion_path = format!("{page_path}/discussion");
    let form = "content=Release+discussion&user_id=999999";
    assert_eq!(
        request(&app, &discussion_path, Some(&cookie), Some(form), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = request(
        &app,
        &discussion_path,
        Some(&cookie),
        Some(form),
        Some("http://cabinet.test"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let entries = cabinet::discussion::list(&pool, &scope, reference)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].authored_by, scope);
    assert_eq!(entries[0].content, "Release discussion");
    let response = request(&app, &discussion_path, Some(&cookie), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(html(response).await.contains("Release discussion"));
    let history_path = format!("{page_path}/history");
    let response = request(&app, &history_path, Some(&cookie), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(html(response).await.contains("Revision 1"));
    assert_eq!(
        cabinet::history(
            &pool,
            HistoryRequest {
                scope,
                cabinet_ref: reference.clone(),
            }
        )
        .await
        .unwrap()
        .len(),
        1,
        "discussion cannot create a revision"
    );

    let den_links = den_cabinet_router(&state.config).with_state(state);
    let response = request(&den_links, &history_path, None, None, None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("http://cabinet.test{history_path}")
    );
    assert_eq!(
        request(
            &den_links,
            &discussion_path,
            Some(&cookie),
            Some(form),
            Some("http://cabinet.test")
        )
        .await
        .status(),
        StatusCode::GONE
    );
    // Browser login is never an MCP credential, even with a legitimate cookie.
    assert_eq!(
        request(&app, "/mcp", Some(&cookie), Some("{}"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
