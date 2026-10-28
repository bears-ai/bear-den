//! PKCE policy shared by consent admission and authorization-code redemption.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use super::{error::OAuthError, utils};

pub(super) fn validate_challenge(
    public: bool,
    challenge: Option<&str>,
    method: Option<&str>,
) -> Result<(), OAuthError> {
    let invalid = || {
        OAuthError::InvalidRequest(
            "Public clients require a valid S256 PKCE challenge; PKCE parameters must be complete"
                .into(),
        )
    };
    match (challenge, method) {
        (None, None) if !public => Ok(()),
        (Some(challenge), Some("S256")) => {
            let decoded = URL_SAFE_NO_PAD.decode(challenge).map_err(|_| invalid())?;
            if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != challenge {
                return Err(invalid());
            }
            Ok(())
        }
        (Some(challenge), Some("plain")) if !public => {
            // Confidential clients retain their existing plain-PKCE compatibility.
            utils::validate_pkce(challenge, challenge, "plain")
        }
        _ => Err(invalid()),
    }
}

pub(super) fn validate_verifier(
    public: bool,
    challenge: Option<&str>,
    method: Option<&str>,
    verifier: Option<&str>,
) -> Result<(), OAuthError> {
    validate_challenge(public, challenge, method)?;
    match (challenge, method, verifier) {
        (None, None, None) => Ok(()),
        (Some(challenge), Some(method), Some(verifier)) => {
            utils::validate_pkce(verifier, challenge, method).map_err(|_| OAuthError::InvalidGrant)
        }
        _ => Err(OAuthError::InvalidGrant),
    }
}

#[cfg(test)]
#[path = "tests/pkce.rs"]
mod tests;
