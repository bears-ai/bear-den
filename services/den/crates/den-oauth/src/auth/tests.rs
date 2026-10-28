use super::*;
use crate::oauth::{test_support::fixture, utils};

fn headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

#[sqlx::test(migrations = "../../migrations")]
async fn active_auth_requires_a_persisted_current_token_and_active_client(pool: PgPool) {
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
    let token = fixture.token(&pool).await;
    let principal = authenticate_bearer_active(&headers(&token), &pool)
        .await
        .unwrap();
    assert_eq!(principal.user_id, fixture.user_id);
    require_scope(&principal, OAuthScope::DataWrite).unwrap();
    db::revoke_access_token(&pool, &token).await.unwrap();
    assert_eq!(
        authenticate_bearer_active(&headers(&token), &pool)
            .await
            .unwrap_err()
            .status,
        StatusCode::UNAUTHORIZED
    );
    let token = fixture.token(&pool).await;
    db::deactivate_oauth_client(&pool, fixture.client.id)
        .await
        .unwrap();
    assert_eq!(
        authenticate_bearer_active(&headers(&token), &pool)
            .await
            .unwrap_err()
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn signed_but_unpersisted_expired_or_mismatched_authority_is_rejected(pool: PgPool) {
    let owner = fixture(
        &pool,
        true,
        vec![OAuthScope::ProfileRead, OAuthScope::DataRead],
    )
    .await;
    let other = fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    let manager = create_jwt_manager();
    let expires = utils::access_token_expiration();
    let unpersisted = manager
        .generate_access_token(
            owner.user_id,
            &owner.client.client_id,
            &owner.scopes,
            expires,
        )
        .unwrap();
    assert!(authenticate_bearer_active(&headers(&unpersisted), &pool)
        .await
        .is_err());
    for (client, user, scopes, expiry) in [
        (
            owner.client.id,
            other.user_id,
            owner.scopes.clone(),
            expires,
        ),
        (
            other.client.id,
            owner.user_id,
            owner.scopes.clone(),
            expires,
        ),
        (
            owner.client.id,
            owner.user_id,
            vec![OAuthScope::ProfileRead],
            expires,
        ),
        (
            owner.client.id,
            owner.user_id,
            owner.scopes.clone(),
            time::OffsetDateTime::now_utc() - time::Duration::seconds(1),
        ),
    ] {
        let token = manager
            .generate_access_token(
                owner.user_id,
                &owner.client.client_id,
                &owner.scopes,
                expires,
            )
            .unwrap();
        db::create_access_token(&pool, &token, client, user, &scopes, expiry)
            .await
            .unwrap();
        assert_eq!(
            authenticate_bearer_active(&headers(&token), &pool)
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    let token = owner.token(&pool).await;
    db::update_oauth_client(
        &pool,
        owner.client.id,
        "Reduced scopes",
        &owner.client.redirect_uris,
        &[OAuthScope::ProfileRead],
    )
    .await
    .unwrap();
    assert!(authenticate_bearer_active(&headers(&token), &pool)
        .await
        .is_err());
}

#[tokio::test]
async fn storage_failure_fails_closed_without_echoing_the_credential() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
        .unwrap();
    pool.close().await;
    let token = create_jwt_manager()
        .generate_access_token(
            1,
            "native",
            &[OAuthScope::ProfileRead],
            utils::access_token_expiration(),
        )
        .unwrap();
    let error = authenticate_bearer_active(&headers(&token), &pool)
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!error.message.contains(&token));
}
