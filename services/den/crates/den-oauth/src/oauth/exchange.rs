//! One serializable transaction owns code consumption and every issued token.

use sqlx::{PgConnection, PgPool};

use super::{
    db, error::OAuthError, jwt::JwtManager, pkce, scopes_from_json, scopes_to_string, utils,
    OAuthClient, TokenRequest, TokenResponse,
};

fn storage_error(_: impl std::fmt::Display) -> OAuthError {
    // Database diagnostics can include the token value in a uniqueness violation.
    OAuthError::ServerError("OAuth token storage is unavailable".into())
}

fn transaction_error(error: sqlx::Error) -> OAuthError {
    if let sqlx::Error::Database(database) = &error {
        if database.code().as_deref() == Some("40001") {
            return OAuthError::InvalidGrant;
        }
    }
    storage_error(error)
}

pub(super) async fn exchange_code(
    pool: &PgPool,
    client: &OAuthClient,
    request: &TokenRequest,
    manager: &JwtManager,
) -> Result<TokenResponse, OAuthError> {
    // The cached read and update queries are intentionally kept unchanged. At
    // SERIALIZABLE isolation two readers cannot both commit the used-bit write;
    // a losing transaction rolls back all its token inserts as well.
    let mut transaction = pool
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .map_err(storage_error)?;
    let response = issue_in_transaction(&mut transaction, client, request, manager).await?;
    transaction.commit().await.map_err(transaction_error)?;
    Ok(response)
}

async fn issue_in_transaction(
    connection: &mut PgConnection,
    client: &OAuthClient,
    request: &TokenRequest,
    manager: &JwtManager,
) -> Result<TokenResponse, OAuthError> {
    let code = db::get_and_validate_authorization_code(
        &mut *connection,
        &request.code,
        client.id,
        &request.redirect_uri,
    )
    .await
    .map_err(|error| match error {
        OAuthError::InvalidGrant => OAuthError::InvalidGrant,
        _ => storage_error(error),
    })?;
    pkce::validate_verifier(
        client.public,
        code.code_challenge.as_deref(),
        code.code_challenge_method.as_deref(),
        request.code_verifier.as_deref(),
    )?;
    let scopes = scopes_from_json(&code.scopes)?;
    let client_scopes = scopes_from_json(&client.scopes)?;
    if !utils::validate_scopes_for_client(&scopes, &client_scopes) {
        return Err(OAuthError::InvalidScope);
    }
    db::mark_authorization_code_used_checked(&mut *connection, code.id)
        .await
        .map_err(transaction_error)?;
    let expires_at = utils::access_token_expiration();
    let access_token = manager
        .generate_access_token(code.user_id, &client.client_id, &scopes, expires_at)
        .map_err(|_| OAuthError::ServerError("OAuth token signing is unavailable".into()))?;
    db::create_access_token(
        &mut *connection,
        &access_token,
        client.id,
        code.user_id,
        &scopes,
        expires_at,
    )
    .await
    .map_err(storage_error)?;
    // Preserve existing confidential-client issuance, without adding a refresh
    // grant or granting refresh credentials to native/public clients.
    let refresh_token = if client.public {
        None
    } else {
        let token = utils::generate_refresh_token();
        db::create_refresh_token(
            &mut *connection,
            &token,
            client.id,
            code.user_id,
            &scopes,
            utils::refresh_token_expiration(),
        )
        .await
        .map_err(storage_error)?;
        Some(token)
    };
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".into(),
        expires_in: utils::seconds_until_expiration(expires_at),
        scope: scopes_to_string(&scopes),
        refresh_token,
    })
}

#[cfg(test)]
#[path = "tests/exchange.rs"]
mod tests;
