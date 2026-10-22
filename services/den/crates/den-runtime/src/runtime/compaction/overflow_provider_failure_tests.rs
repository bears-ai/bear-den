use super::*;
use den_core::{ProviderFailure, ProviderFailureKind, ProviderHttpStatus, ProviderTransportKind};

#[test]
fn typed_overflow_does_not_depend_on_display_or_retry_metadata() {
    let overflow = ProviderFailure::new(ProviderFailureKind::ContextOverflow)
        .with_http_status(ProviderHttpStatus::new(400).unwrap())
        .with_retry_exhausted(4);
    assert!(den_error_indicates_context_overflow(&DenError::Provider(
        overflow
    )));
    for kind in [
        ProviderFailureKind::Unknown,
        ProviderFailureKind::KeySelection,
        ProviderFailureKind::VirtualKeyNotFound,
        ProviderFailureKind::RateLimited,
        ProviderFailureKind::Unavailable,
        ProviderFailureKind::Timeout,
        ProviderFailureKind::IdleTimeout,
        ProviderFailureKind::Transport(ProviderTransportKind::Timeout),
        ProviderFailureKind::StreamRead(ProviderTransportKind::UnexpectedEof),
    ] {
        assert!(
            !den_error_indicates_context_overflow(&DenError::Provider(
                ProviderFailure::new(kind).with_http_status(ProviderHttpStatus::new(413).unwrap())
            )),
            "{kind:?}"
        );
    }
}

#[test]
fn legacy_overflow_classification_remains_compatible() {
    for error in [
        DenError::System("maximum context length exceeded".into()),
        DenError::ValidationError("context_length_exceeded".into()),
        DenError::Anyhow(anyhow::anyhow!("too many tokens")),
        DenError::Parsing("prompt is too long".into()),
    ] {
        assert!(den_error_indicates_context_overflow(&error));
    }
}
