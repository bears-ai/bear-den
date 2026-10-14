use super::auth::{self, AuthError};
use axum::http::{header, HeaderMap, HeaderValue, Uri};
use den_cabinet::Actor;
use den_core::ids::UserId;
use den_oauth::oauth::{
    jwt::{JwtClaims, JwtManager},
    utils::scopes_to_json,
    AccessTokenWithContext, OAuthClient, OAuthScope,
};
use time::{Duration, OffsetDateTime};

fn fixture() -> (
    JwtClaims,
    AccessTokenWithContext,
    OAuthClient,
    OffsetDateTime,
) {
    let now = OffsetDateTime::now_utc();
    let scopes = [OAuthScope::DataRead];
    let claims =
        JwtClaims::new_access_token(31, "delegated-client", &scopes, now + Duration::hours(1));
    let token = AccessTokenWithContext {
        token_id: 17,
        token: "never-log-this-token".into(),
        client_id: 3,
        user_id: 31,
        scopes: scopes_to_json(&scopes),
        expires_at: now + Duration::hours(1),
        revoked: false,
        token_created_at: now,
        client_identifier: "delegated-client".into(),
        client_name: "External runtime".into(),
        username: "person".into(),
        email: "person@example.test".into(),
        display_name: "Person".into(),
    };
    let client = OAuthClient {
        id: 3,
        client_id: "delegated-client".into(),
        client_secret: None,
        name: "External runtime".into(),
        redirect_uris: serde_json::json!(["http://127.0.0.1/callback"]),
        scopes: scopes_to_json(&scopes),
        active: true,
        trusted: false,
        public: true,
        created_at: now,
        updated_at: now,
    };
    (claims, token, client, now)
}

#[test]
fn delegated_identity_comes_only_from_verified_subject_and_live_rows() {
    let (claims, token, client, now) = fixture();
    let principal = auth::validate_current(&claims, &token, &client, 31, now).unwrap();
    assert_eq!(
        principal.scope.actor,
        Actor::User {
            user_id: UserId(31)
        }
    );
    assert_eq!(principal.client_id, "delegated-client");
    assert_eq!(principal.token_id, 17);
    assert!(principal.scope.run_id.is_none());
    assert!(principal.scope.conversation_id.is_none());
    assert!(principal.scope.task_id.is_none());
    for subject in ["32", "0", "bear_foo", "-1"] {
        let mut altered = claims.clone();
        altered.sub = subject.into();
        assert!(matches!(
            auth::validate_current(&altered, &token, &client, 31, now),
            Err(AuthError::InvalidToken)
        ));
    }
    assert!(matches!(
        auth::validate_current(&claims, &token, &client, 32, now),
        Err(AuthError::InvalidToken)
    ));
    let mut altered = claims;
    altered.client_id = "another-client".into();
    assert!(auth::validate_current(&altered, &token, &client, 31, now).is_err());
}

#[test]
fn stale_revoked_expired_inactive_and_reduced_grants_fail_closed() {
    let (claims, token, client, now) = fixture();
    assert!(auth::validate_current(&claims, &token, &client, 31, now).is_ok());
    let mut revoked = token.clone();
    revoked.revoked = true;
    assert!(matches!(
        auth::validate_current(&claims, &revoked, &client, 31, now),
        Err(AuthError::InvalidToken)
    ));
    let mut expired = token.clone();
    expired.expires_at = now;
    assert!(auth::validate_current(&claims, &expired, &client, 31, now).is_err());
    let mut inactive = client.clone();
    inactive.active = false;
    assert!(auth::validate_current(&claims, &token, &inactive, 31, now).is_err());
    let mut changed_token = token.clone();
    changed_token.scopes = scopes_to_json(&[OAuthScope::ProfileRead]);
    assert!(matches!(
        auth::validate_current(&claims, &changed_token, &client, 31, now),
        Err(AuthError::InsufficientScope)
    ));
    let mut changed_client = client.clone();
    changed_client.scopes = scopes_to_json(&[OAuthScope::ProfileRead]);
    assert!(matches!(
        auth::validate_current(&claims, &token, &changed_client, 31, now),
        Err(AuthError::InsufficientScope)
    ));
    let mut changed_claims = claims.clone();
    changed_claims.scope = "data:write armature:chat profile:read".into();
    assert!(matches!(
        auth::validate_current(&changed_claims, &token, &client, 31, now),
        Err(AuthError::InsufficientScope)
    ));
    let mut bad_scopes = token.clone();
    bad_scopes.scopes = serde_json::json!(["unknown:scope"]);
    assert!(auth::validate_current(&claims, &bad_scopes, &client, 31, now).is_err());
    assert!(
        auth::validate_current(&claims, &token, &client, 31, now + Duration::hours(2)).is_err()
    );
    // An earlier successful authentication never changes later row validation.
    assert!(auth::validate_current(&claims, &token, &client, 31, now).is_ok());
}

#[test]
fn audience_issuer_type_not_before_and_client_row_binding_are_enforced() {
    let (claims, token, client, now) = fixture();
    let mut wrong = claims.clone();
    wrong.aud = "bearwire".into();
    assert!(auth::validate_current(&wrong, &token, &client, 31, now).is_err());
    wrong = claims.clone();
    wrong.iss = "other-issuer".into();
    assert!(auth::validate_current(&wrong, &token, &client, 31, now).is_err());
    wrong = claims.clone();
    wrong.token_type = "refresh_token".into();
    assert!(auth::validate_current(&wrong, &token, &client, 31, now).is_err());
    wrong = claims.clone();
    wrong.nbf = now.unix_timestamp() + 1;
    assert!(auth::validate_current(&wrong, &token, &client, 31, now).is_err());
    let mut other_client = client.clone();
    other_client.id = 9;
    assert!(auth::validate_current(&claims, &token, &other_client, 31, now).is_err());
    let mut other_token = token;
    other_token.client_identifier = "foreign".into();
    assert!(auth::validate_current(&claims, &other_token, &client, 31, now).is_err());
}

#[test]
fn shared_oauth_manager_rejects_unsigned_foreign_and_expired_credentials() {
    let manager = JwtManager::new(b"test-only-secret");
    let foreign = JwtManager::new(b"different-test-secret");
    let expiry = OffsetDateTime::now_utc() + Duration::hours(1);
    let good = manager
        .generate_access_token(31, "delegated-client", &[OAuthScope::DataRead], expiry)
        .unwrap();
    assert!(manager.validate_access_token(&good).is_ok());
    assert!(foreign.validate_access_token(&good).is_err());
    assert!(manager
        .validate_access_token("opaque-armature-token")
        .is_err());
    let expired = manager
        .generate_access_token(
            31,
            "delegated-client",
            &[OAuthScope::DataRead],
            OffsetDateTime::now_utc() - Duration::hours(1),
        )
        .unwrap();
    assert!(manager.validate_access_token(&expired).is_err());
}

#[test]
fn bearer_scheme_is_case_insensitive_without_changing_the_credential() {
    let mut headers = HeaderMap::new();
    assert!(auth::bearer_token(&headers).is_err());
    for value in ["Bearer MiXeD.Jwt", "bearer MiXeD.Jwt", "BEARER  MiXeD.Jwt "] {
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        assert_eq!(auth::bearer_token(&headers).unwrap(), "MiXeD.Jwt");
    }
    for value in [
        "Basic token",
        "Bearer",
        "Bearer ",
        "Bearer token other",
        "Bearer token\tother",
    ] {
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        assert!(auth::bearer_token(&headers).is_err(), "{value}");
    }
}

#[test]
fn origins_query_credentials_duplicate_auth_and_session_claims_are_rejected() {
    let uri = Uri::from_static("/mcp");
    let mut headers = HeaderMap::new();
    assert!(auth::protect_transport(&uri, &headers).is_ok());
    for origin in [
        "https://cabinet.example.test",
        "https://evil.example.test",
        "null",
    ] {
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        assert_eq!(
            auth::protect_transport(&uri, &headers),
            Err(AuthError::UnsafeTransport)
        );
    }
    headers.remove(header::ORIGIN);
    assert!(
        auth::protect_transport(&Uri::from_static("/mcp?access_token=secret"), &headers).is_err()
    );
    headers.insert(
        "mcp-session-id",
        HeaderValue::from_static("foreign-session"),
    );
    assert!(auth::protect_transport(&uri, &headers).is_err());
    headers.remove("mcp-session-id");
    headers.append(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer one"),
    );
    headers.append(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer two"),
    );
    assert!(auth::protect_transport(&uri, &headers).is_err());
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres; do not run against the live stack"]
async fn database_revocation_and_client_scope_changes_apply_to_next_request(pool: sqlx::PgPool) {
    let user = den_http::user::db::create_user(
        &pool,
        "mcp@example.test",
        "mcpperson",
        "MCP person",
        "unused-test-passhash",
    )
    .await
    .unwrap();
    let client = den_oauth::oauth::db::create_oauth_client(
        &pool,
        "mcp-client",
        None,
        "MCP client",
        &serde_json::json!(["http://127.0.0.1/callback"]),
        &[OAuthScope::DataRead],
        true,
    )
    .await
    .unwrap();
    let manager = JwtManager::new(b"database-test-only-secret");
    let expiry = OffsetDateTime::now_utc() + Duration::hours(1);
    let token = manager
        .generate_access_token(user, "mcp-client", &[OAuthScope::DataRead], expiry)
        .unwrap();
    den_oauth::oauth::db::create_access_token(
        &pool,
        &token,
        client,
        user,
        &[OAuthScope::DataRead],
        expiry,
    )
    .await
    .unwrap();
    assert!(auth::authenticate_with_manager(&pool, &token, &manager)
        .await
        .is_ok());
    den_oauth::oauth::db::update_oauth_client(
        &pool,
        client,
        "MCP client",
        &serde_json::json!(["http://127.0.0.1/callback"]),
        &[OAuthScope::ProfileRead],
    )
    .await
    .unwrap();
    assert!(matches!(
        auth::authenticate_with_manager(&pool, &token, &manager).await,
        Err(AuthError::InsufficientScope)
    ));
    den_oauth::oauth::db::update_oauth_client(
        &pool,
        client,
        "MCP client",
        &serde_json::json!(["http://127.0.0.1/callback"]),
        &[OAuthScope::DataRead],
    )
    .await
    .unwrap();
    assert!(auth::authenticate_with_manager(&pool, &token, &manager)
        .await
        .is_ok());
    den_oauth::oauth::db::revoke_access_token(&pool, &token)
        .await
        .unwrap();
    assert!(matches!(
        auth::authenticate_with_manager(&pool, &token, &manager).await,
        Err(AuthError::InvalidToken)
    ));
    let another = manager
        .generate_access_token(user, "mcp-client", &[OAuthScope::DataRead], expiry)
        .unwrap();
    den_oauth::oauth::db::create_access_token(
        &pool,
        &another,
        client,
        user,
        &[OAuthScope::DataRead],
        expiry,
    )
    .await
    .unwrap();
    den_oauth::oauth::db::deactivate_oauth_client(&pool, client)
        .await
        .unwrap();
    assert!(matches!(
        auth::authenticate_with_manager(&pool, &another, &manager).await,
        Err(AuthError::InvalidToken)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres; do not run against the live stack"]
async fn delegated_human_page_isolation_and_policy_revocation(pool: sqlx::PgPool) {
    use super::tools;
    use den_cabinet::{ActorScope, CabinetPolicy, CreateItemRequest, ItemKind};
    let owner = den_http::user::db::create_user(
        &pool,
        "owner@example.test",
        "mcpowner",
        "Owner",
        "unused-test-passhash",
    )
    .await
    .unwrap();
    let other = den_http::user::db::create_user(
        &pool,
        "other@example.test",
        "mcpother",
        "Other",
        "unused-test-passhash",
    )
    .await
    .unwrap();
    let owner_scope = ActorScope::user(UserId(owner));
    let other_scope = ActorScope::user(UserId(other));
    let item = den_service::cabinet::create_item(
        &pool,
        CreateItemRequest {
            scope: owner_scope.clone(),
            kind: ItemKind::Document,
            title: "PRIVATE MCP TITLE".into(),
            content: "PRIVATE MCP CONTENT".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: Vec::new(),
        },
    )
    .await
    .unwrap();
    den_service::cabinet::pages::configure(
        &pool,
        &owner_scope,
        &item.item.cabinet_ref,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let read = || {
        tools::parse_call(serde_json::json!({"name":"cabinet_read","arguments":{"cabinet_ref":item.item.cabinet_ref}})).unwrap()
    };
    let authorized = tools::dispatch(&pool, None, &owner_scope, read())
        .await
        .unwrap();
    assert_eq!(authorized["isError"], false);
    let denied = tools::dispatch(&pool, None, &other_scope, read())
        .await
        .unwrap();
    assert_eq!(denied["structuredContent"]["error"]["kind"], "not_found");
    assert!(!denied.to_string().contains("PRIVATE MCP"));
    let search = tools::parse_call(
        serde_json::json!({"name":"cabinet_search","arguments":{"query":"PRIVATE MCP"}}),
    )
    .unwrap();
    assert!(tools::dispatch(&pool, None, &other_scope, search)
        .await
        .unwrap()["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .is_empty());
    den_service::cabinet::pages::configure(
        &pool,
        &owner_scope,
        &item.item.cabinet_ref,
        CabinetPolicy::default(),
        &[owner, other],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        tools::dispatch(&pool, None, &other_scope, read())
            .await
            .unwrap()["isError"],
        false
    );
    den_service::cabinet::pages::configure(
        &pool,
        &owner_scope,
        &item.item.cabinet_ref,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        tools::dispatch(&pool, None, &other_scope, read())
            .await
            .unwrap()["isError"],
        true
    );
}
