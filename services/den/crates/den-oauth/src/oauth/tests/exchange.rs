use super::*;
use crate::oauth::{
    jwt::create_jwt_manager,
    test_support::{fixture, CALLBACK},
    OAuthScope,
};

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_redemption_has_one_winner_and_one_persisted_token(pool: PgPool) {
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
    let request = fixture.code(&pool).await;
    let manager = create_jwt_manager();
    let (first, second) = tokio::join!(
        exchange_code(&pool, &fixture.client, &request, &manager),
        exchange_code(&pool, &fixture.client, &request, &manager),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let token = first.or(second).unwrap();
    assert!(token.refresh_token.is_none());
    assert!(crate::auth::authenticate_bearer_active(
        &{
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", token.access_token).parse().unwrap(),
            );
            headers
        },
        &pool
    )
    .await
    .is_ok());
    assert_eq!(
        db::list_user_access_tokens(&pool, fixture.user_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        exchange_code(&pool, &fixture.client, &request, &manager).await,
        Err(OAuthError::InvalidGrant)
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_pkce_callback_or_client_never_consumes_a_valid_code(pool: PgPool) {
    let fixture = fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    let mut request = fixture.code(&pool).await;
    let manager = create_jwt_manager();
    request.code_verifier = Some("x".repeat(43));
    assert!(exchange_code(&pool, &fixture.client, &request, &manager)
        .await
        .is_err());
    request.code_verifier = Some(crate::oauth::test_support::VERIFIER.into());
    request.redirect_uri = "com.attacker:/callback".into();
    assert!(exchange_code(&pool, &fixture.client, &request, &manager)
        .await
        .is_err());
    request.redirect_uri = CALLBACK.into();
    let other =
        crate::oauth::test_support::fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    assert!(exchange_code(&pool, &other.client, &request, &manager)
        .await
        .is_err());
    assert!(db::list_user_access_tokens(&pool, fixture.user_id)
        .await
        .unwrap()
        .is_empty());
    assert!(exchange_code(&pool, &fixture.client, &request, &manager)
        .await
        .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn overlapping_unused_code_snapshots_cannot_both_commit(pool: PgPool) {
    let fixture = fixture(&pool, true, vec![OAuthScope::ProfileRead]).await;
    let request = fixture.code(&pool).await;
    let manager = create_jwt_manager();
    let mut first = pool
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .unwrap();
    let mut second = pool
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .unwrap();
    for transaction in [&mut first, &mut second] {
        db::get_and_validate_authorization_code(
            &mut **transaction,
            &request.code,
            fixture.client.id,
            CALLBACK,
        )
        .await
        .unwrap();
    }
    let (first, second): (
        Result<TokenResponse, OAuthError>,
        Result<TokenResponse, OAuthError>,
    ) = tokio::join!(
        async {
            let response =
                issue_in_transaction(&mut first, &fixture.client, &request, &manager).await?;
            first.commit().await.map_err(transaction_error)?;
            Ok(response)
        },
        async {
            let response =
                issue_in_transaction(&mut second, &fixture.client, &request, &manager).await?;
            second.commit().await.map_err(transaction_error)?;
            Ok(response)
        },
    );
    assert!(matches!(
        (&first, &second),
        (Ok(_), Err(OAuthError::InvalidGrant)) | (Err(OAuthError::InvalidGrant), Ok(_))
    ));
    assert_eq!(
        db::list_user_access_tokens(&pool, fixture.user_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rollback_removes_code_consumption_and_both_confidential_token_inserts(pool: PgPool) {
    let fixture = fixture(&pool, false, vec![OAuthScope::ProfileRead]).await;
    let request = fixture.code(&pool).await;
    let mut transaction = pool
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .unwrap();
    let response = issue_in_transaction(
        &mut transaction,
        &fixture.client,
        &request,
        &create_jwt_manager(),
    )
    .await
    .unwrap();
    let refresh = response.refresh_token.as_deref().unwrap();
    transaction.rollback().await.unwrap();
    assert!(db::list_user_access_tokens(&pool, fixture.user_id)
        .await
        .unwrap()
        .is_empty());
    assert!(
        db::get_and_validate_access_token(&pool, &response.access_token)
            .await
            .is_err()
    );
    assert!(db::get_and_validate_refresh_token(&pool, refresh)
        .await
        .is_err());
    assert!(db::get_and_validate_authorization_code(
        &pool,
        &request.code,
        fixture.client.id,
        CALLBACK
    )
    .await
    .is_ok());
    assert!(
        exchange_code(&pool, &fixture.client, &request, &create_jwt_manager())
            .await
            .is_ok()
    );
}
