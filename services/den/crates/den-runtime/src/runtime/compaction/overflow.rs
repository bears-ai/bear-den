//! Classifiers for LLM/provider context-length overflow errors.

use den_core::DenError;

/// Returns true when `message` looks like a provider context-window / token-limit rejection.
pub fn is_context_length_overflow_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "context length",
        "maximum context",
        "max context",
        "token limit",
        "too many tokens",
        "context_window",
        "context window",
        "max_tokens",
        "context_length_exceeded",
        "context_length",
        "maximum number of tokens",
        "prompt is too long",
        "request too large",
        "input is too long",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Returns true when a [`DenError`] likely indicates context-length overflow from the LLM path.
pub fn den_error_indicates_context_overflow(err: &DenError) -> bool {
    match err {
        DenError::Provider(failure) => failure.is_context_overflow(),
        DenError::System(msg) | DenError::ValidationError(msg) => {
            is_context_length_overflow_message(msg)
        }
        DenError::Anyhow(cause) => is_context_length_overflow_message(&cause.to_string()),
        other => is_context_length_overflow_message(&other.to_string()),
    }
}

#[cfg(test)]
#[path = "overflow_provider_failure_tests.rs"]
mod provider_failure_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_overflow_phrases() {
        assert!(is_context_length_overflow_message(
            "LLM chat/completions HTTP 400: context_length_exceeded"
        ));
        assert!(is_context_length_overflow_message(
            "maximum context length exceeded for model"
        ));
        assert!(is_context_length_overflow_message(
            "too many tokens in request"
        ));
        assert!(is_context_length_overflow_message("context window is 128k"));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!is_context_length_overflow_message("connection refused"));
        assert!(!is_context_length_overflow_message("authentication failed"));
    }

    #[test]
    fn den_error_classifier_matches_system_errors() {
        let err = DenError::System(
            "LLM chat/completions HTTP 400: {\"error\":{\"code\":\"context_length_exceeded\"}}"
                .into(),
        );
        assert!(den_error_indicates_context_overflow(&err));
        assert!(!den_error_indicates_context_overflow(&DenError::NotFound(
            "x".into()
        )));
    }
}
