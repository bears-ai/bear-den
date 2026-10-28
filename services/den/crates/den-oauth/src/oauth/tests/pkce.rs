use super::*;
use crate::oauth::test_support::VERIFIER;

#[test]
fn public_clients_require_complete_canonical_s256_challenges() {
    let challenge = utils::generate_pkce_code_challenge(VERIFIER);
    assert!(validate_challenge(true, Some(&challenge), Some("S256")).is_ok());
    for (challenge, method) in [
        (None, None),
        (Some(VERIFIER), Some("plain")),
        (Some(""), Some("S256")),
        (Some(VERIFIER), None),
        (None, Some("S256")),
        (Some("not-a-sha256"), Some("S256")),
    ] {
        assert!(validate_challenge(true, challenge, method).is_err());
    }
    assert!(validate_challenge(true, Some(&format!("{challenge}=")), Some("S256")).is_err());
}

#[test]
fn confidential_legacy_plain_remains_supported_but_partial_pkce_does_not() {
    assert!(validate_challenge(false, None, None).is_ok());
    assert!(validate_verifier(false, Some(VERIFIER), Some("plain"), Some(VERIFIER)).is_ok());
    assert!(validate_challenge(false, Some(VERIFIER), None).is_err());
}

#[test]
fn redemption_rejects_public_legacy_plain_missing_or_wrong_verifiers() {
    let challenge = utils::generate_pkce_code_challenge(VERIFIER);
    assert!(validate_verifier(true, Some(&challenge), Some("S256"), Some(VERIFIER)).is_ok());
    assert!(validate_verifier(true, Some(VERIFIER), Some("plain"), Some(VERIFIER)).is_err());
    assert!(validate_verifier(true, Some(&challenge), Some("S256"), None).is_err());
    assert!(
        validate_verifier(true, Some(&challenge), Some("S256"), Some(&"x".repeat(43))).is_err()
    );
}
