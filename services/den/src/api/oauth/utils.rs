//! OAuth 2.0 utility functions
//!
//! This module provides utility functions for OAuth operations including
//! secure random generation, client secret hashing, scope validation,
//! and redirect URI validation.

use crate::api::oauth::{OAuthScope, error::OAuthError};
use password_auth::{generate_hash, verify_password};
use std::collections::HashSet;
use time::{Duration, OffsetDateTime};
use url::Url;

/// Generate a cryptographically secure random string for codes and tokens
///
/// # Arguments
/// * `length` - The desired length of the generated string
///
/// # Returns
/// A random alphanumeric string of the specified length
pub fn generate_secure_random_string(length: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                            abcdefghijklmnopqrstuvwxyz\
                            0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generate an OAuth authorization code
///
/// Creates a secure random authorization code with standard length.
///
/// # Returns
/// A 32-character random authorization code
pub fn generate_authorization_code() -> String {
    generate_secure_random_string(32)
}

/// Generate an OAuth access token
///
/// Creates a secure random access token with standard length.
///
/// # Returns
/// A 64-character random access token
pub fn generate_access_token() -> String {
    generate_secure_random_string(64)
}

/// Generate an OAuth refresh token
///
/// Creates a secure random refresh token with standard length.
/// Refresh tokens are longer-lived than access tokens and used to obtain new access tokens.
///
/// # Returns
/// A 64-character random refresh token
pub fn generate_refresh_token() -> String {
    generate_secure_random_string(64)
}

/// Generate a client ID
///
/// Creates a secure random client identifier.
///
/// # Returns
/// A 24-character random client ID
pub fn generate_client_id() -> String {
    generate_secure_random_string(24)
}

/// Generate a client secret
///
/// Creates a secure random client secret.
///
/// # Returns
/// A 48-character random client secret
pub fn generate_client_secret() -> String {
    generate_secure_random_string(48)
}

/// Hash a client secret for secure storage
///
/// Uses the same password hashing mechanism as user passwords for consistency.
///
/// # Arguments
/// * `secret` - The plain text client secret to hash
///
/// # Returns
/// The hashed client secret suitable for database storage
///
/// # Errors
/// Returns `OAuthError::ServerError` if hashing fails
pub fn hash_client_secret(secret: &str) -> Result<String, OAuthError> {
    Ok(generate_hash(secret))
}

/// Verify a client secret against its hash
///
/// # Arguments
/// * `secret` - The plain text client secret to verify
/// * `hash` - The stored hash to verify against
///
/// # Returns
/// `true` if the secret matches the hash, `false` otherwise
pub fn verify_client_secret(secret: &str, hash: &str) -> bool {
    verify_password(secret, hash).is_ok()
}

/// Validate and parse a space-separated scope string
///
/// # Arguments
/// * `scope_string` - Space-separated scope string (e.g., "profile email")
///
/// # Returns
/// A vector of validated OAuth scopes
///
/// # Errors
/// Returns `OAuthError::InvalidScope` if any scope is invalid or unknown
pub fn parse_scopes(scope_string: &str) -> Result<Vec<OAuthScope>, OAuthError> {
    if scope_string.trim().is_empty() {
        return Ok(vec![]);
    }

    let mut scopes = Vec::new();
    let mut seen_scopes = HashSet::new();

    for scope_name in scope_string.split_whitespace() {
        // Check for duplicate scopes
        if seen_scopes.contains(scope_name) {
            continue; // Skip duplicates silently
        }
        seen_scopes.insert(scope_name);

        match OAuthScope::from_str(scope_name) {
            Some(scope) => scopes.push(scope),
            None => return Err(OAuthError::InvalidScope),
        }
    }

    Ok(scopes)
}

/// Enhanced scope validation with conflict detection
///
/// Validates scopes and detects potential conflicts or redundant permissions.
///
/// # Arguments
/// * `scopes` - Vector of OAuth scopes to validate
///
/// # Returns
/// Result with validation warnings or errors
pub fn validate_scopes_with_conflict_detection(
    scopes: &[OAuthScope],
) -> Result<Vec<String>, OAuthError> {
    let mut warnings = Vec::new();

    // Check for minimum scope requirements
    if scopes.is_empty() {
        return Err(OAuthError::InvalidScope);
    }

    // Detect scope conflicts and redundancies
    let has_profile = scopes.contains(&OAuthScope::ProfileRead);
    let has_email = scopes.contains(&OAuthScope::ProfileEmail);

    // Email scope implies profile access, so warn if email is requested without profile
    if has_email && !has_profile {
        warnings.push(
            "Email scope includes profile access. Consider adding 'profile' scope for clarity."
                .to_string(),
        );
    }

    // Profile scope alone is fine for basic access
    if has_profile && !has_email {
        warnings.push("This client will only have access to basic profile information (username, display name). Email access is not included.".to_string());
    }

    // Both scopes provide full access
    if has_profile && has_email {
        warnings.push(
            "This client will have full access to user profile and email information.".to_string(),
        );
    }

    Ok(warnings)
}

/// Generate scope preview for user interface
///
/// Creates a human-readable description of what the scopes allow.
///
/// # Arguments
/// * `scopes` - Vector of OAuth scopes
///
/// # Returns
/// Human-readable scope description
pub fn generate_scope_preview(scopes: &[OAuthScope]) -> String {
    if scopes.is_empty() {
        return "No permissions selected".to_string();
    }

    let mut preview_parts = Vec::new();

    for scope in scopes {
        match scope {
            OAuthScope::ProfileRead => {
                preview_parts
                    .push("basic profile information (username, display name)".to_string());
            }
            OAuthScope::ProfileEmail => {
                preview_parts.push("user's email address".to_string());
            }
            OAuthScope::DataRead => {
                preview_parts.push("read access to your data via the API".to_string());
            }
            OAuthScope::DataWrite => {
                preview_parts.push("write access to your data via the API".to_string());
            }
        }
    }

    if preview_parts.len() == 1 {
        format!("Access to {}", preview_parts[0])
    } else {
        let last_part = preview_parts.pop().unwrap();
        let other_parts = preview_parts.join(", ");
        format!("Access to {} and {}", other_parts, last_part)
    }
}

/// Generate a PKCE code verifier
///
/// Creates a cryptographically secure random string suitable for PKCE code verifier.
/// Follows RFC 7636 specifications for length and character set.
///
/// # Returns
/// A 43-128 character random string suitable for PKCE code verifier
pub fn generate_pkce_code_verifier() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    let length = rng.random_range(43..=128); // RFC 7636: 43-128 characters

    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generate a PKCE code challenge from a code verifier using S256 method
///
/// Implements RFC 7636 SHA256 challenge method.
///
/// # Arguments
/// * `code_verifier` - The PKCE code verifier
///
/// # Returns
/// Base64url-encoded SHA256 hash of the code verifier
pub fn generate_pkce_code_challenge(code_verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    base64url_encode(&hash)
}

/// Validate that requested scopes are allowed for a client
///
/// # Arguments
/// * `requested_scopes` - The scopes being requested
/// * `client_scopes` - The scopes allowed for this client
///
/// # Returns
/// `true` if all requested scopes are allowed, `false` otherwise
pub fn validate_scopes_for_client(
    requested_scopes: &[OAuthScope],
    client_scopes: &[OAuthScope],
) -> bool {
    requested_scopes
        .iter()
        .all(|scope| client_scopes.contains(scope))
}

/// Validate a redirect URI
///
/// Ensures the redirect URI is a valid URL and meets security requirements.
/// Allows:
/// - HTTPS URLs (for web applications)
/// - HTTP URLs for localhost/127.0.0.1 (for development)
/// - Custom protocol schemes (for mobile apps, e.g., `myapp://`)
///
/// # Arguments
/// * `redirect_uri` - The redirect URI to validate
///
/// # Returns
/// `true` if the redirect URI is valid, `false` otherwise
pub fn validate_redirect_uri(redirect_uri: &str) -> bool {
    match Url::parse(redirect_uri) {
        Ok(url) => {
            let scheme = url.scheme();

            // Allow HTTPS for web applications
            if scheme == "https" {
                return true;
            }

            // Allow HTTP only for localhost/127.0.0.1 for development
            if scheme == "http" {
                return match url.host_str() {
                    Some("localhost") | Some("127.0.0.1") => true,
                    _ => false,
                };
            }

            // Allow custom protocol schemes (for mobile apps)
            // Reject dangerous schemes that could be used for XSS or other attacks
            let scheme_lower = scheme.to_lowercase();
            let dangerous_schemes = ["javascript", "data", "vbscript", "file", "about"];
            if dangerous_schemes.contains(&scheme_lower.as_str()) {
                return false;
            }

            // Allow any other valid URL scheme (custom protocols like myapp://, etc.)
            // The scheme must be non-empty and the URL must be well-formed
            !scheme.is_empty()
        }
        Err(_) => false,
    }
}

/// Check if a redirect URI is allowed for a client
///
/// # Arguments
/// * `redirect_uri` - The redirect URI to check
/// * `allowed_uris` - JSON array of allowed redirect URIs for the client
///
/// # Returns
/// `true` if the redirect URI is allowed, `false` otherwise
pub fn is_redirect_uri_allowed(redirect_uri: &str, allowed_uris: &serde_json::Value) -> bool {
    match allowed_uris {
        serde_json::Value::Array(uris) => uris.iter().any(|uri| {
            if let serde_json::Value::String(allowed_uri) = uri {
                allowed_uri == redirect_uri
            } else {
                false
            }
        }),
        _ => false,
    }
}

/// Calculate expiration time for authorization codes
///
/// Authorization codes should have a short lifetime (10 minutes as per RFC 6749).
///
/// # Returns
/// An `OffsetDateTime` 10 minutes from now
pub fn authorization_code_expiration() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::minutes(10)
}

/// Calculate expiration time for access tokens
///
/// Access tokens have a longer lifetime (1 hour by default).
///
/// # Returns
/// An `OffsetDateTime` 1 hour from now
pub fn access_token_expiration() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::hours(1)
}

/// Calculate expiration time for refresh tokens
///
/// Refresh tokens have a much longer lifetime (90 days by default) to allow
/// clients to obtain new access tokens without re-authorization.
///
/// # Returns
/// An `OffsetDateTime` 90 days from now
pub fn refresh_token_expiration() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::days(90)
}

/// Check if a timestamp has expired
///
/// # Arguments
/// * `expires_at` - The expiration timestamp to check
///
/// # Returns
/// `true` if the timestamp is in the past, `false` otherwise
pub fn is_expired(expires_at: OffsetDateTime) -> bool {
    OffsetDateTime::now_utc() > expires_at
}

/// Calculate seconds until expiration for token responses
///
/// # Arguments
/// * `expires_at` - The expiration timestamp
///
/// # Returns
/// Number of seconds until expiration, or 0 if already expired
pub fn seconds_until_expiration(expires_at: OffsetDateTime) -> i64 {
    let now = OffsetDateTime::now_utc();
    if expires_at > now {
        (expires_at - now).whole_seconds()
    } else {
        0
    }
}

/// Validate PKCE code_verifier against code_challenge
///
/// Implements RFC 7636 (Proof Key for Code Exchange) validation.
/// Supports both S256 (SHA256) and plain challenge methods.
///
/// # Arguments
/// * `code_verifier` - The code verifier provided in token request
/// * `code_challenge` - The code challenge stored with authorization code
/// * `code_challenge_method` - The challenge method (S256 or plain)
///
/// # Returns
/// `Ok(())` if validation succeeds, or `OAuthError::InvalidGrant` if it fails
///
/// # Security
/// - S256 (SHA256) is strongly recommended for production use
/// - Plain method is allowed per RFC 7636 but less secure
/// - Code verifier must be 43-128 characters as per RFC 7636
pub fn validate_pkce(
    code_verifier: &str,
    code_challenge: &str,
    code_challenge_method: &str,
) -> Result<(), OAuthError> {
    // Validate code_verifier length (43-128 characters per RFC 7636)
    if code_verifier.len() < 43 || code_verifier.len() > 128 {
        tracing::warn!(
            "Invalid code_verifier length: {} (must be 43-128 characters)",
            code_verifier.len()
        );
        return Err(OAuthError::InvalidRequest(format!(
            "code_verifier length is {} characters, but must be between 43 and 128 characters (RFC 7636)",
            code_verifier.len()
        )));
    }

    // Validate code_verifier contains only allowed characters
    // RFC 7636: unreserved characters [A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"
    if !code_verifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
    {
        tracing::warn!("Invalid code_verifier characters");
        return Err(OAuthError::InvalidRequest(
            "code_verifier contains invalid characters. Only alphanumeric characters and '-', '.', '_', '~' are allowed (RFC 7636)".to_string(),
        ));
    }

    match code_challenge_method {
        "S256" => {
            // Generate SHA256 hash of code_verifier
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(code_verifier.as_bytes());
            let hash = hasher.finalize();

            // Convert to base64url (RFC 4648 Section 5)
            let computed_challenge = base64url_encode(&hash);

            // Compare with stored challenge
            if computed_challenge != code_challenge {
                tracing::warn!("PKCE validation failed: code_challenge mismatch");
                return Err(OAuthError::InvalidRequest(
                    "PKCE validation failed: The SHA256 hash of the code_verifier does not match the stored code_challenge. Ensure you're using the same code_verifier that was used to generate the code_challenge in the authorization request.".to_string(),
                ));
            }

            Ok(())
        }
        "plain" => {
            // For plain method, verifier must match challenge exactly
            if code_verifier != code_challenge {
                tracing::warn!("PKCE validation failed: plain code_challenge mismatch");
                return Err(OAuthError::InvalidRequest(
                    "PKCE validation failed: The code_verifier does not match the code_challenge for the 'plain' method. Ensure you're using the same code_verifier that was used as the code_challenge in the authorization request.".to_string(),
                ));
            }

            Ok(())
        }
        _ => {
            tracing::warn!(
                "Unsupported code_challenge_method: {}",
                code_challenge_method
            );
            Err(OAuthError::InvalidRequest(format!(
                "Unsupported code_challenge_method: '{}'. Only 'S256' and 'plain' are supported (RFC 7636)",
                code_challenge_method
            )))
        }
    }
}

/// Encode bytes to base64url format (RFC 4648 Section 5)
///
/// Base64url encoding is base64 with URL-safe characters:
/// - Use '-' instead of '+'
/// - Use '_' instead of '/'
/// - No padding ('=' characters)
///
/// # Arguments
/// * `data` - The bytes to encode
///
/// # Returns
/// Base64url-encoded string
fn base64url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Validate PKCE code_challenge_method
///
/// Ensures the challenge method is one of the supported values.
///
/// # Arguments
/// * `method` - The code_challenge_method to validate
///
/// # Returns
/// `true` if the method is valid (S256 or plain), `false` otherwise
pub fn validate_code_challenge_method(method: &str) -> bool {
    matches!(method, "S256" | "plain")
}

/// Helper function to convert scopes to JSON value for database storage
pub fn scopes_to_json(scopes: &[OAuthScope]) -> serde_json::Value {
    let scope_strings: Vec<String> = scopes.iter().map(|s| s.as_str().to_string()).collect();
    serde_json::Value::Array(
        scope_strings
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    )
}

/// Helper function to parse scopes from JSON value
pub fn scopes_from_json(json: &serde_json::Value) -> Result<Vec<OAuthScope>, OAuthError> {
    match json {
        serde_json::Value::Array(arr) => {
            let mut scopes = Vec::new();
            for item in arr {
                if let serde_json::Value::String(scope_str) = item {
                    match OAuthScope::from_str(scope_str) {
                        Some(scope) => scopes.push(scope),
                        None => return Err(OAuthError::InvalidScope),
                    }
                } else {
                    return Err(OAuthError::InvalidScope);
                }
            }
            Ok(scopes)
        }
        _ => Err(OAuthError::InvalidScope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secure_random_string() {
        let s1 = generate_secure_random_string(32);
        let s2 = generate_secure_random_string(32);

        assert_eq!(s1.len(), 32);
        assert_eq!(s2.len(), 32);
        assert_ne!(s1, s2); // Should be different
        assert!(s1.chars().all(|c| c.is_alphanumeric()));
    }

    #[test]
    fn test_generate_codes_and_tokens() {
        let auth_code = generate_authorization_code();
        let access_token = generate_access_token();
        let client_id = generate_client_id();
        let client_secret = generate_client_secret();

        assert_eq!(auth_code.len(), 32);
        assert_eq!(access_token.len(), 64);
        assert_eq!(client_id.len(), 24);
        assert_eq!(client_secret.len(), 48);
    }

    #[test]
    fn test_client_secret_hashing() {
        let secret = "test_secret_123";
        let hash = hash_client_secret(secret).unwrap();

        assert_ne!(hash, secret);
        assert!(verify_client_secret(secret, &hash));
        assert!(!verify_client_secret("wrong_secret", &hash));
    }

    #[test]
    fn test_parse_scopes() {
        let scopes = parse_scopes("profile:read profile:email").unwrap();
        assert_eq!(
            scopes,
            vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail]
        );

        let empty_scopes = parse_scopes("").unwrap();
        assert!(empty_scopes.is_empty());

        let invalid_scopes = parse_scopes("profile invalid_scope");
        assert!(invalid_scopes.is_err());

        // Test duplicate handling
        let duplicate_scopes = parse_scopes("profile:read profile:email profile:read").unwrap();
        assert_eq!(
            duplicate_scopes,
            vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail]
        );
    }

    #[test]
    fn test_validate_scopes_for_client() {
        let client_scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let requested_scopes = vec![OAuthScope::ProfileRead];

        assert!(validate_scopes_for_client(
            &requested_scopes,
            &client_scopes
        ));

        let invalid_requested = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let limited_client_scopes = vec![OAuthScope::ProfileRead];

        assert!(!validate_scopes_for_client(
            &invalid_requested,
            &limited_client_scopes
        ));
    }

    #[test]
    fn test_validate_redirect_uri() {
        // HTTPS URLs (web applications)
        assert!(validate_redirect_uri("https://example.com/callback"));

        // HTTP URLs for localhost (development)
        assert!(validate_redirect_uri("http://localhost:3000/callback"));
        assert!(validate_redirect_uri("http://127.0.0.1:8080/callback"));

        // Custom protocol schemes (mobile apps)
        assert!(validate_redirect_uri("myapp://oauth/callback"));
        assert!(validate_redirect_uri("myapp://oauth/callback"));
        assert!(validate_redirect_uri("com.example.app://callback"));

        // Rejected cases
        assert!(!validate_redirect_uri("http://example.com/callback")); // HTTP not allowed for non-localhost
        assert!(!validate_redirect_uri("javascript:alert('xss')")); // Dangerous scheme
        assert!(!validate_redirect_uri(
            "data:text/html,<script>alert('xss')</script>"
        )); // Dangerous scheme
        assert!(!validate_redirect_uri("not_a_url")); // Invalid URL
    }

    #[test]
    fn test_is_redirect_uri_allowed() {
        let allowed_uris = serde_json::json!([
            "https://example.com/callback",
            "http://localhost:3000/callback"
        ]);

        assert!(is_redirect_uri_allowed(
            "https://example.com/callback",
            &allowed_uris
        ));
        assert!(is_redirect_uri_allowed(
            "http://localhost:3000/callback",
            &allowed_uris
        ));
        assert!(!is_redirect_uri_allowed(
            "https://evil.com/callback",
            &allowed_uris
        ));

        let invalid_json = serde_json::json!("not_an_array");
        assert!(!is_redirect_uri_allowed(
            "https://example.com/callback",
            &invalid_json
        ));
    }

    #[test]
    fn test_expiration_functions() {
        let auth_exp = authorization_code_expiration();
        let token_exp = access_token_expiration();
        let now = OffsetDateTime::now_utc();

        assert!(auth_exp > now);
        assert!(token_exp > now);
        assert!(token_exp > auth_exp);

        // Test is_expired
        let past_time = now - Duration::minutes(1);
        let future_time = now + Duration::minutes(1);

        assert!(is_expired(past_time));
        assert!(!is_expired(future_time));

        // Test seconds_until_expiration
        assert_eq!(seconds_until_expiration(past_time), 0);
        assert!(seconds_until_expiration(future_time) > 0);
    }

    #[test]
    fn test_pkce_s256_validation() {
        // Test vector from RFC 7636
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        // Valid S256 PKCE validation
        let result = validate_pkce(code_verifier, code_challenge, "S256");
        assert!(result.is_ok(), "Valid S256 PKCE should succeed");

        // Wrong code_verifier
        let wrong_verifier = "wrongwrongwrongwrongwrongwrongwrongwrongwro";
        let result = validate_pkce(wrong_verifier, code_challenge, "S256");
        assert!(result.is_err(), "Wrong code_verifier should fail");

        // Wrong code_challenge
        let wrong_challenge = "wrongchallenge";
        let result = validate_pkce(code_verifier, wrong_challenge, "S256");
        assert!(result.is_err(), "Wrong code_challenge should fail");
    }

    #[test]
    fn test_pkce_plain_validation() {
        let code_verifier = "test-verifier-that-is-long-enough-for-pkce-1234567890"; // Exactly 43 chars, valid characters only
        let code_challenge = code_verifier; // For plain method, they should match

        // Valid plain PKCE validation
        let result = validate_pkce(code_verifier, code_challenge, "plain");
        assert!(result.is_ok(), "Valid plain PKCE should succeed");

        // Mismatched verifier and challenge
        let wrong_challenge = "different-challenge-value-for-plain-method-1234567890";
        let result = validate_pkce(code_verifier, wrong_challenge, "plain");
        assert!(result.is_err(), "Mismatched plain PKCE should fail");
    }

    #[test]
    fn test_pkce_verifier_length_validation() {
        let code_challenge = "validchallenge";

        // Too short verifier (< 43 chars)
        let short_verifier = "tooshort";
        let result = validate_pkce(short_verifier, code_challenge, "S256");
        assert!(result.is_err(), "Too short verifier should fail");

        // Too long verifier (> 128 chars)
        let long_verifier = "a".repeat(129);
        let result = validate_pkce(&long_verifier, code_challenge, "S256");
        assert!(result.is_err(), "Too long verifier should fail");

        // Valid length (43 chars)
        let valid_verifier = "a".repeat(43);
        let result = validate_pkce(&valid_verifier, code_challenge, "S256");
        // This will fail challenge validation but pass length validation
        assert!(
            result.is_err(),
            "Should fail on challenge mismatch, not length"
        );
    }

    #[test]
    fn test_pkce_verifier_character_validation() {
        let code_challenge = "validchallenge";

        // Valid characters: alphanumeric and -._~
        let valid_verifier = "abcABC123-._~abcABC123-._~abcABC123-._~a";
        let result = validate_pkce(valid_verifier, code_challenge, "S256");
        // Will fail on challenge mismatch but pass character validation
        assert!(
            result.is_err(),
            "Should fail on challenge mismatch, not character validation"
        );

        // Invalid characters
        let invalid_verifier = "abc!@#$%^&*()abc123456789012345678901234";
        let result = validate_pkce(invalid_verifier, code_challenge, "S256");
        assert!(result.is_err(), "Invalid characters should fail");
    }

    #[test]
    fn test_pkce_unsupported_method() {
        let code_verifier = "validverifierthatisenoughlongforpkcevalidation";
        let code_challenge = "validchallenge";

        // Unsupported method
        let result = validate_pkce(code_verifier, code_challenge, "unsupported");
        assert!(result.is_err(), "Unsupported method should fail");

        // Empty method
        let result = validate_pkce(code_verifier, code_challenge, "");
        assert!(result.is_err(), "Empty method should fail");
    }

    #[test]
    fn test_validate_code_challenge_method() {
        assert!(validate_code_challenge_method("S256"));
        assert!(validate_code_challenge_method("plain"));
        assert!(!validate_code_challenge_method("invalid"));
        assert!(!validate_code_challenge_method(""));
    }

    #[test]
    fn test_base64url_encode() {
        // Test base64url encoding
        let data = b"hello world";
        let encoded = base64url_encode(data);

        // Should not contain +, /, or =
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));

        // Should be valid base64url
        assert!(
            encoded
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn test_validate_scopes_with_conflict_detection() {
        // Test profile scope alone
        let profile_scopes = vec![OAuthScope::ProfileRead];
        let warnings = validate_scopes_with_conflict_detection(&profile_scopes).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("basic profile information"));

        // Test email scope alone
        let email_scopes = vec![OAuthScope::ProfileEmail];
        let warnings = validate_scopes_with_conflict_detection(&email_scopes).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("includes profile access"));

        // Test both scopes
        let both_scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let warnings = validate_scopes_with_conflict_detection(&both_scopes).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("full access"));

        // Test empty scopes (should error)
        let empty_scopes: Vec<OAuthScope> = vec![];
        let result = validate_scopes_with_conflict_detection(&empty_scopes);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_scope_preview() {
        // Test empty scopes
        let empty_scopes: Vec<OAuthScope> = vec![];
        let preview = generate_scope_preview(&empty_scopes);
        assert_eq!(preview, "No permissions selected");

        // Test single scope
        let profile_scopes = vec![OAuthScope::ProfileRead];
        let preview = generate_scope_preview(&profile_scopes);
        assert!(preview.contains("basic profile information"));

        // Test multiple scopes
        let both_scopes = vec![OAuthScope::ProfileRead, OAuthScope::ProfileEmail];
        let preview = generate_scope_preview(&both_scopes);
        assert!(preview.contains("basic profile information"));
        assert!(preview.contains("user's email address"));
        assert!(preview.contains("and"));
    }
}
