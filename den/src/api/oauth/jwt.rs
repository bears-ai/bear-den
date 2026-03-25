//! JWT token generation and validation for OAuth 2.0
//!
//! This module provides JWT (JSON Web Token) functionality for OAuth access tokens,
//! replacing the simple random string tokens with self-contained JWT tokens that
//! include claims and can be validated without database lookups.

use crate::api::oauth::{OAuthScope, error::OAuthError};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// JWT signing algorithm - using HS256 for simplicity
const JWT_ALGORITHM: Algorithm = Algorithm::HS256;

/// JWT issuer claim
const JWT_ISSUER: &str = "newapp-oauth";

/// JWT audience claim
const JWT_AUDIENCE: &str = "newapp-api";

/// JWT claims structure for OAuth access tokens
///
/// This structure contains the standard JWT claims plus OAuth-specific claims
/// following RFC 7519 (JWT) and RFC 6749 (OAuth 2.0) specifications.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JwtClaims {
    /// Subject (user ID)
    pub sub: String,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: String,
    /// Expiration time (Unix timestamp)
    pub exp: i64,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Not before (Unix timestamp)
    pub nbf: i64,
    /// JWT ID (unique token identifier)
    pub jti: String,
    /// Client ID that requested this token
    pub client_id: String,
    /// Granted scopes (space-separated string)
    pub scope: String,
    /// Token type (always "access_token")
    pub token_type: String,
}

impl JwtClaims {
    /// Create new JWT claims for an access token
    ///
    /// # Arguments
    /// * `user_id` - The user ID (subject)
    /// * `client_id` - The OAuth client ID
    /// * `scopes` - Granted OAuth scopes
    /// * `expires_at` - When the token expires
    ///
    /// # Returns
    /// JWT claims ready for encoding
    pub fn new_access_token(
        user_id: i32,
        client_id: &str,
        scopes: &[OAuthScope],
        expires_at: OffsetDateTime,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        let scope_string = scopes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        Self {
            sub: user_id.to_string(),
            iss: JWT_ISSUER.to_string(),
            aud: JWT_AUDIENCE.to_string(),
            exp: expires_at.unix_timestamp(),
            iat: now.unix_timestamp(),
            nbf: now.unix_timestamp(),
            jti: generate_jti(),
            client_id: client_id.to_string(),
            scope: scope_string,
            token_type: "access_token".to_string(),
        }
    }

    /// Parse scopes from the scope claim
    ///
    /// # Returns
    /// Vector of OAuth scopes or error if invalid
    pub fn parse_scopes(&self) -> Result<Vec<OAuthScope>, OAuthError> {
        if self.scope.trim().is_empty() {
            return Ok(vec![]);
        }

        let mut scopes = Vec::new();
        for scope_name in self.scope.split_whitespace() {
            match OAuthScope::from_str(scope_name) {
                Some(scope) => scopes.push(scope),
                None => return Err(OAuthError::InvalidScope),
            }
        }

        Ok(scopes)
    }

    /// Check if the token has expired
    ///
    /// # Returns
    /// `true` if the token has expired, `false` otherwise
    pub fn is_expired(&self) -> bool {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        now >= self.exp
    }

    /// Check if the token is valid for use (not before time)
    ///
    /// # Returns
    /// `true` if the token is valid for use, `false` otherwise
    pub fn is_valid_for_use(&self) -> bool {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        now >= self.nbf
    }

    /// Get the user ID as an integer
    ///
    /// # Returns
    /// User ID or error if invalid
    pub fn user_id(&self) -> Result<i32, OAuthError> {
        self.sub
            .parse::<i32>()
            .map_err(|_| OAuthError::InvalidToken)
    }
}

/// JWT token manager for OAuth operations
#[derive(Clone)]
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtManager {
    /// Create a new JWT manager with the given secret
    ///
    /// # Arguments
    /// * `secret` - The secret key for signing and verifying JWTs
    ///
    /// # Returns
    /// A new JWT manager instance
    pub fn new(secret: &[u8]) -> Self {
        let encoding_key = EncodingKey::from_secret(secret);
        let decoding_key = DecodingKey::from_secret(secret);

        let mut validation = Validation::new(JWT_ALGORITHM);
        validation.set_issuer(&[JWT_ISSUER]);
        validation.set_audience(&[JWT_AUDIENCE]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        Self {
            encoding_key,
            decoding_key,
            validation,
        }
    }

    /// Generate a JWT access token
    ///
    /// # Arguments
    /// * `user_id` - The user ID
    /// * `client_id` - The OAuth client ID
    /// * `scopes` - Granted OAuth scopes
    /// * `expires_at` - When the token expires
    ///
    /// # Returns
    /// A signed JWT token string or error
    pub fn generate_access_token(
        &self,
        user_id: i32,
        client_id: &str,
        scopes: &[OAuthScope],
        expires_at: OffsetDateTime,
    ) -> Result<String, OAuthError> {
        let claims = JwtClaims::new_access_token(user_id, client_id, scopes, expires_at);

        let header = Header::new(JWT_ALGORITHM);

        encode(&header, &claims, &self.encoding_key)
            .map_err(|e| OAuthError::ServerError(format!("Failed to generate JWT: {e}")))
    }

    /// Validate and decode a JWT access token
    ///
    /// # Arguments
    /// * `token` - The JWT token string to validate
    ///
    /// # Returns
    /// Decoded JWT claims or error if invalid
    pub fn validate_access_token(&self, token: &str) -> Result<JwtClaims, OAuthError> {
        let token_data = decode::<JwtClaims>(token, &self.decoding_key, &self.validation).map_err(
            |e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => OAuthError::InvalidToken,
                jsonwebtoken::errors::ErrorKind::InvalidToken => OAuthError::InvalidToken,
                jsonwebtoken::errors::ErrorKind::InvalidSignature => OAuthError::InvalidToken,
                jsonwebtoken::errors::ErrorKind::InvalidIssuer => OAuthError::InvalidToken,
                jsonwebtoken::errors::ErrorKind::InvalidAudience => OAuthError::InvalidToken,
                jsonwebtoken::errors::ErrorKind::ImmatureSignature => OAuthError::InvalidToken,
                _ => OAuthError::ServerError(format!("JWT validation error: {e}")),
            },
        )?;

        let claims = token_data.claims;

        // Additional validation
        if claims.token_type != "access_token" {
            return Err(OAuthError::InvalidToken);
        }

        // Validate that the token is not expired (double-check)
        if claims.is_expired() {
            return Err(OAuthError::InvalidToken);
        }

        // Validate that the token is valid for use
        if !claims.is_valid_for_use() {
            return Err(OAuthError::InvalidToken);
        }

        Ok(claims)
    }

    /// Extract claims from a JWT token without full validation
    ///
    /// This is useful for debugging or when you need to inspect a token
    /// without validating its signature or expiration.
    ///
    /// # Arguments
    /// * `token` - The JWT token string
    ///
    /// # Returns
    /// Decoded JWT claims or error if malformed
    pub fn _decode_claims_unsafe(&self, token: &str) -> Result<JwtClaims, OAuthError> {
        let mut validation = Validation::new(JWT_ALGORITHM);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.insecure_disable_signature_validation();

        let token_data = decode::<JwtClaims>(token, &self.decoding_key, &validation)
            .map_err(|e| OAuthError::ServerError(format!("Failed to decode JWT: {e}")))?;

        Ok(token_data.claims)
    }
}

/// Generate a unique JWT ID (jti claim)
///
/// # Returns
/// A unique identifier for the JWT token
fn generate_jti() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                            abcdefghijklmnopqrstuvwxyz\
                            0123456789";
    let mut rng = rand::rng();
    (0..16)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Get JWT secret from environment or generate a default one
///
/// In production, this should always come from a secure environment variable.
/// For development, we provide a default secret.
///
/// # Returns
/// JWT secret as bytes
pub fn get_jwt_secret() -> Vec<u8> {
    std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| {
            tracing::warn!("JWT_SECRET not set, using default secret (not secure for production)");
            "newapp-oauth-jwt-secret-change-in-production".to_string()
        })
        .into_bytes()
}

/// Create a global JWT manager instance
///
/// # Returns
/// A JWT manager configured with the appropriate secret
pub fn create_jwt_manager() -> JwtManager {
    let secret = get_jwt_secret();
    JwtManager::new(&secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn create_test_jwt_manager() -> JwtManager {
        let secret = b"test-secret-key-for-jwt-testing";
        JwtManager::new(secret)
    }

    #[test]
    fn test_jwt_claims_creation() {
        let user_id = 123;
        let client_id = "test_client";
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(1);

        let claims = JwtClaims::new_access_token(user_id, client_id, &scopes, expires_at);

        assert_eq!(claims.sub, "123");
        assert_eq!(claims.client_id, "test_client");
        assert_eq!(claims.scope, "profile:read profile:email");
        assert_eq!(claims.token_type, "access_token");
        assert_eq!(claims.iss, JWT_ISSUER);
        assert_eq!(claims.aud, JWT_AUDIENCE);
        assert!(!claims.is_expired());
        assert!(claims.is_valid_for_use());
    }

    #[test]
    fn test_jwt_token_generation_and_validation() {
        let jwt_manager = create_test_jwt_manager();
        let user_id = 123;
        let client_id = "test_client";
        let scopes = vec![OAuthScope::ProfileRead];
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(1);

        // Generate token
        let token = jwt_manager
            .generate_access_token(user_id, client_id, &scopes, expires_at)
            .unwrap();

        assert!(!token.is_empty());
        assert!(token.contains('.')); // JWT format has dots

        // Validate token
        let claims = jwt_manager.validate_access_token(&token).unwrap();

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert_eq!(claims.client_id, client_id);
        assert_eq!(claims.parse_scopes().unwrap(), scopes);
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_expired_token_validation() {
        let jwt_manager = create_test_jwt_manager();
        let user_id = 123;
        let client_id = "test_client";
        let scopes = vec![OAuthScope::ProfileRead];
        let expires_at = OffsetDateTime::now_utc() - Duration::hours(1); // Expired

        let token = jwt_manager
            .generate_access_token(user_id, client_id, &scopes, expires_at)
            .unwrap();

        let result = jwt_manager.validate_access_token(&token);
        assert!(matches!(result, Err(OAuthError::InvalidToken)));
    }

    #[test]
    fn test_invalid_token_validation() {
        let jwt_manager = create_test_jwt_manager();

        // Test completely invalid token
        let result = jwt_manager.validate_access_token("invalid.token.here");
        assert!(result.is_err());
        // The specific error type may vary based on JWT library implementation

        // Test empty token
        let result = jwt_manager.validate_access_token("");
        assert!(result.is_err());

        // Test malformed JWT (not enough parts)
        let result = jwt_manager.validate_access_token("invalid.token");
        assert!(result.is_err());

        // Test token with wrong signature
        let result = jwt_manager.validate_access_token("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        assert!(result.is_err());
    }

    #[test]
    fn test_scope_parsing() {
        let user_id = 123;
        let client_id = "test_client";
        let scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(1);

        let claims = JwtClaims::new_access_token(user_id, client_id, &scopes, expires_at);
        let parsed_scopes = claims.parse_scopes().unwrap();

        assert_eq!(parsed_scopes, scopes);
    }

    #[test]
    fn test_jti_generation() {
        let jti1 = generate_jti();
        let jti2 = generate_jti();

        assert_eq!(jti1.len(), 16);
        assert_eq!(jti2.len(), 16);
        assert_ne!(jti1, jti2); // Should be unique
        assert!(jti1.chars().all(|c| c.is_alphanumeric()));
    }
}
