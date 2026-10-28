use super::*;
use crate::oauth::test_support::{fixture, Fixture, CALLBACK};
use axum::{
    body::{to_bytes, Body},
    extract::Path,
    http::Request,
    routing::get,
    Router,
};
use axum_login::{
    tower_sessions::{MemoryStore, SessionManagerLayer},
    AuthManagerLayerBuilder, AuthnBackend,
};
use den_http::auth_backend::Backend;
use serde_json::Value;
use tower::ServiceExt;

async fn login(Path(user_id): Path<i32>, mut auth: AuthSession) -> StatusCode {
    let user = auth.backend.get_user(&user_id).await.unwrap().unwrap();
    auth.login(&user).await.unwrap();
    StatusCode::OK
}

fn app(pool: &PgPool) -> Router {
    let state = OAuthState::new(
        pool.clone(),
        "https://web.example.test".into(),
        "https://api.example.test".into(),
    );
    let auth = AuthManagerLayerBuilder::new(
        Backend::new(pool.clone()),
        SessionManagerLayer::new(MemoryStore::default()).with_secure(false),
    )
    .build();
    Router::new()
        .nest(
            "/oauth",
            crate::oauth::router::create_oauth_router(&den_core::config::Config::test_stub())
                .with_state(state),
        )
        .route("/test/login/{user_id}", get(login))
        .layer(auth)
}

async fn cookie(app: &Router, user: i32) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test/login/{user}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}

fn fields(request: &AuthorizationRequestWithPKCE) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("response_type", request.response_type.clone()),
        ("client_id", request.client_id.clone()),
        ("redirect_uri", request.redirect_uri.clone()),
    ];
    for (key, value) in [
        ("scope", &request.scope),
        ("state", &request.state),
        ("code_challenge", &request.code_challenge),
        ("code_challenge_method", &request.code_challenge_method),
    ] {
        if let Some(value) = value {
            fields.push((key, value.clone()));
        }
    }
    fields
}

fn encoded(fields: &[(&str, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(k, v)| (*k, v)))
        .finish()
}

async fn consent_page(app: &Router, cookie: &str, fixture: &Fixture) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/oauth/authorize?{}",
                    encoded(&fields(&fixture.authorization()))
                ))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    body.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}

async fn post(app: &Router, cookie: &str, fields: Vec<(&str, String)>) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(encoded(&fields)))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn consent_post_enforces_csrf_request_binding_scope_ceiling_and_pkce(pool: PgPool) {
    let fixture = fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    let app = app(&pool);
    let cookie = cookie(&app, fixture.user_id).await;
    let csrf = consent_page(&app, &cookie, &fixture).await;
    for alteration in 0..4 {
        let mut request = fixture.authorization();
        match alteration {
            1 => request.scope = Some("profile:read data:write".into()),
            2 => request.code_challenge_method = Some("plain".into()),
            3 => request.redirect_uri = "com.attacker:/callback".into(),
            _ => (),
        }
        let mut fields = fields(&request);
        fields.push(("action", "allow".into()));
        fields.push((
            "csrf_token",
            if alteration == 0 {
                "invented".into()
            } else {
                csrf.clone()
            },
        ));
        let response = post(&app, &cookie, fields).await;
        assert!(!response.status().is_success());
        assert!(!response.headers().contains_key(header::LOCATION));
    }
    let mut fields = fields(&fixture.authorization());
    fields.extend([("action", "allow".into()), ("csrf_token", csrf)]);
    let response = post(&app, &cookie, fields.clone()).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let callback = Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(callback.scheme(), Url::parse(CALLBACK).unwrap().scheme());
    let query: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
    assert_eq!(query["state"], "native-state");
    db::get_and_validate_authorization_code(&pool, &query["code"], fixture.client.id, CALLBACK)
        .await
        .unwrap();
    assert_eq!(
        post(&app, &cookie, fields).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn userinfo_enforces_active_authority_and_scope_shaped_profile(pool: PgPool) {
    let fixture = fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    let token = fixture.token(&pool).await;
    let app = app(&pool);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["sub"], fixture.user_id.to_string());
    assert_eq!(body["name"], "Native human");
    assert!(body["email"].is_null());
    db::revoke_access_token(&pool, &token).await.unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn public_http_exchange_is_single_use_active_and_has_no_refresh_token(pool: PgPool) {
    let fixture = fixture(
        &pool,
        true,
        vec![
            OAuthScope::ProfileRead,
            OAuthScope::DataRead,
            OAuthScope::DataWrite,
        ],
    )
    .await;
    let app = app(&pool);
    let request = fixture.code(&pool).await;
    let fields = vec![
        ("grant_type", "authorization_code".into()),
        ("client_id", request.client_id),
        ("code", request.code),
        ("redirect_uri", request.redirect_uri),
        ("code_verifier", request.code_verifier.unwrap()),
    ];
    let exchange = || {
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(encoded(&fields)))
            .unwrap()
    };
    let response = app.clone().oneshot(exchange()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::PRAGMA], "no-cache");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["scope"], "profile:read data:read data:write");
    assert_eq!(body["token_type"], "Bearer");
    assert!(body["expires_in"].as_i64().unwrap() > 0);
    assert!(body.get("refresh_token").is_none());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", body["access_token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    crate::auth::authenticate_bearer_active(&headers, &pool)
        .await
        .unwrap();
    let response = app.clone().oneshot(exchange()).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let failure: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(failure["error"], "invalid_grant");
    assert!(!failure.to_string().contains(&fields[2].1));
    let mut refresh = fields.clone();
    refresh[0].1 = "refresh_token".into();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(encoded(&refresh)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["error"], "unsupported_grant_type");
}

#[test]
fn login_return_and_callback_preserve_state_and_pkce() {
    let request = AuthorizationRequestWithPKCE {
        response_type: "code".into(),
        client_id: "native".into(),
        redirect_uri: CALLBACK.into(),
        scope: Some("profile:read data:read data:write".into()),
        state: Some("state + value".into()),
        code_challenge: Some("challenge".into()),
        code_challenge_method: Some("S256".into()),
    };
    let url = Url::parse(&build_return_url(&request, "https://api.example.test").unwrap()).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["state"], "state + value");
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(pairs["redirect_uri"], CALLBACK);
    let response = generate_authorization_response(
        CALLBACK,
        None,
        Some(OAuthError::AccessDenied),
        request.state.as_deref(),
    )
    .unwrap();
    assert!(response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .contains("state=state+%2B+value"));
}
