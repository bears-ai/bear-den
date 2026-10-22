use std::error::Error;

use super::*;
use crate::DenError;

fn status(value: u16) -> ProviderHttpStatus {
    ProviderHttpStatus::new(value).unwrap()
}

#[test]
fn http_status_is_checked_and_never_has_a_reason_phrase() {
    for value in [0, 99, 1000, u16::MAX] {
        assert!(ProviderHttpStatus::new(value).is_none());
    }
    for value in [100, 400, 503, 999] {
        assert_eq!(status(value).as_u16(), value);
        assert_eq!(status(value).to_string(), value.to_string());
    }
}

#[test]
fn request_retry_status_policy_matches_existing_budget() {
    for value in 100..=999 {
        assert_eq!(
            status(value).request_retryable(),
            matches!(value, 408 | 429 | 500..=599)
        );
    }
}

#[test]
fn retry_metadata_does_not_erase_overflow_or_outer_retry_policy() {
    let failure = ProviderFailure::new(ProviderFailureKind::ContextOverflow)
        .with_http_status(status(503))
        .with_retry_exhausted(4);
    assert!(failure.is_context_overflow());
    assert!(failure.retry_exhausted());
    assert_eq!(failure.retry_attempts(), 4);
    assert!(failure.continuation_retryable());
    assert_eq!(failure.http_status(), Some(status(503)));
    assert_eq!(
        ProviderFailure::new(ProviderFailureKind::Unknown)
            .with_retry_attempts(0)
            .retry_attempts(),
        1
    );
}

#[test]
fn fallback_keeps_original_status_and_final_classification() {
    let failure = ProviderFailure::new(ProviderFailureKind::ContextOverflow)
        .with_http_status(status(413))
        .with_key_selection_fallback(status(400))
        .with_retry_attempts(2);
    assert!(failure.is_context_overflow());
    assert_eq!(failure.http_status(), Some(status(413)));
    assert_eq!(failure.key_selection_fallback_status(), Some(status(400)));
    assert_eq!(failure.retry_attempts(), 2);
    assert!(!failure.retry_exhausted());
}

#[test]
fn timeout_and_idle_classification_are_typed() {
    for kind in [
        ProviderFailureKind::Timeout,
        ProviderFailureKind::IdleTimeout,
        ProviderFailureKind::Transport(ProviderTransportKind::Timeout),
        ProviderFailureKind::StreamRead(ProviderTransportKind::Timeout),
    ] {
        let failure = ProviderFailure::new(kind);
        assert!(failure.is_timeout());
        assert!(failure.continuation_retryable());
        assert_eq!(
            failure.is_idle_timeout(),
            kind == ProviderFailureKind::IdleTimeout
        );
    }
    assert!(ProviderFailure::new(ProviderFailureKind::Unknown)
        .with_http_status(status(504))
        .is_timeout());
    assert!(!ProviderFailure::new(ProviderFailureKind::Unknown).is_timeout());
}

#[test]
fn continuation_retryability_uses_only_typed_status_and_kind() {
    for kind in [
        ProviderTransportKind::Timeout,
        ProviderTransportKind::ConnectionReset,
        ProviderTransportKind::ConnectionClosed,
        ProviderTransportKind::ConnectionAborted,
        ProviderTransportKind::BrokenPipe,
        ProviderTransportKind::UnexpectedEof,
    ] {
        assert!(
            ProviderFailure::new(ProviderFailureKind::StreamRead(kind)).continuation_retryable()
        );
    }
    assert!(!ProviderFailure::new(ProviderFailureKind::Unknown)
        .with_http_status(status(501))
        .continuation_retryable());
    assert!(!ProviderFailure::new(ProviderFailureKind::Unknown)
        .with_http_status(status(599))
        .continuation_retryable());
    for kind in [
        ProviderFailureKind::ContextOverflow,
        ProviderFailureKind::KeySelection,
        ProviderFailureKind::VirtualKeyNotFound,
        ProviderFailureKind::Transport(ProviderTransportKind::Connect),
        ProviderFailureKind::Unknown,
    ] {
        assert!(!ProviderFailure::new(kind).continuation_retryable());
    }
}

#[test]
fn formatting_serialization_and_application_error_have_no_source() {
    let failure = ProviderFailure::new(ProviderFailureKind::StreamRead(
        ProviderTransportKind::UnexpectedEof,
    ))
    .with_http_status(status(502))
    .with_retry_exhausted(4)
    .with_key_selection_fallback(status(400));
    assert_eq!(failure.source().map(ToString::to_string), None);
    let encoded = serde_json::to_string(&failure).unwrap();
    assert!(encoded.contains("unexpected_eof"));
    assert!(encoded.contains("502"));
    assert!(!encoded.contains("message"));
    let error = DenError::Provider(failure);
    assert_eq!(error.to_string(), failure.to_string());
    assert!(format!("{error:?}").contains("Provider"));
    assert!(failure.to_string().contains("HTTP 502"));
    assert!(failure.to_string().contains("attempts=4"));
}
