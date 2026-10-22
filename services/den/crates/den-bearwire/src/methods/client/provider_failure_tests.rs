use super::*;
use den_core::{ProviderFailure, ProviderFailureKind, ProviderHttpStatus, ProviderTransportKind};

#[test]
fn typed_continuation_eligibility_keeps_existing_budget_and_schedule() {
    assert_eq!(continuation_retry_pauses_seconds(), vec![2, 4, 54]);
    for kind in [
        ProviderFailureKind::RateLimited,
        ProviderFailureKind::Unavailable,
        ProviderFailureKind::Timeout,
        ProviderFailureKind::IdleTimeout,
        ProviderFailureKind::Transport(ProviderTransportKind::Timeout),
        ProviderFailureKind::Transport(ProviderTransportKind::ConnectionReset),
        ProviderFailureKind::StreamRead(ProviderTransportKind::ConnectionClosed),
        ProviderFailureKind::StreamRead(ProviderTransportKind::ConnectionAborted),
        ProviderFailureKind::StreamRead(ProviderTransportKind::BrokenPipe),
        ProviderFailureKind::StreamRead(ProviderTransportKind::UnexpectedEof),
    ] {
        // The inner HTTP budget does not consume the continuation budget.
        let error = DenError::Provider(ProviderFailure::new(kind).with_retry_exhausted(4));
        assert!(continuation_error_retryable(&error), "{kind:?}");
        for index in 0..3 {
            assert!(should_retry_continuation_error(&error, index));
        }
        assert!(!should_retry_continuation_error(&error, 3));
    }
    for kind in [
        ProviderFailureKind::Unknown,
        ProviderFailureKind::ContextOverflow,
        ProviderFailureKind::KeySelection,
        ProviderFailureKind::VirtualKeyNotFound,
        ProviderFailureKind::Transport(ProviderTransportKind::Connect),
        ProviderFailureKind::StreamRead(ProviderTransportKind::Body),
        ProviderFailureKind::StreamRead(ProviderTransportKind::Decode),
    ] {
        let error = DenError::Provider(ProviderFailure::new(kind).with_retry_exhausted(4));
        assert!(!continuation_error_retryable(&error), "{kind:?}");
        assert!(!should_retry_continuation_error(&error, 0));
    }
}

#[test]
fn typed_continuation_http_statuses_do_not_broaden_retry_policy() {
    for status in [400, 401, 403, 408, 413, 429, 500, 501, 502, 503, 504, 599] {
        let failure = ProviderFailure::new(ProviderFailureKind::Unknown)
            .with_http_status(ProviderHttpStatus::new(status).unwrap())
            .with_key_selection_fallback(ProviderHttpStatus::new(429).unwrap());
        assert_eq!(
            continuation_error_retryable(&DenError::Provider(failure)),
            matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
            "{status}"
        );
    }
}
