use super::*;
use den_core::{ProviderFailure, ProviderFailureKind, ProviderHttpStatus, ProviderTransportKind};

#[test]
fn typed_provider_categories_use_semantics_not_display_substrings() {
    for kind in [
        ProviderFailureKind::Timeout,
        ProviderFailureKind::IdleTimeout,
        ProviderFailureKind::Transport(ProviderTransportKind::Timeout),
        ProviderFailureKind::StreamRead(ProviderTransportKind::Timeout),
    ] {
        let error = DenError::Provider(ProviderFailure::new(kind).with_retry_exhausted(4));
        assert_eq!(
            classify_runtime_error(&error),
            RuntimeErrorCategory::Timeout
        );
        assert!(!runtime_error_is_conflict_pending_approval(&error));
        assert!(!runtime_error_is_no_active_runs_cancel(&error));
    }
    for (status, category) in [
        (400, RuntimeErrorCategory::Internal),
        (408, RuntimeErrorCategory::Timeout),
        (503, RuntimeErrorCategory::Unavailable),
        (504, RuntimeErrorCategory::Timeout),
    ] {
        let failure = ProviderFailure::new(ProviderFailureKind::Unknown)
            .with_http_status(ProviderHttpStatus::new(status).unwrap())
            .with_key_selection_fallback(ProviderHttpStatus::new(504).unwrap());
        assert_eq!(
            classify_runtime_error(&DenError::Provider(failure)),
            category
        );
    }
    assert_eq!(
        classify_runtime_error(&DenError::Provider(ProviderFailure::new(
            ProviderFailureKind::Unavailable
        ))),
        RuntimeErrorCategory::Unavailable
    );
    for kind in [
        ProviderFailureKind::Unknown,
        ProviderFailureKind::ContextOverflow,
        ProviderFailureKind::KeySelection,
        ProviderFailureKind::VirtualKeyNotFound,
        ProviderFailureKind::RateLimited,
        ProviderFailureKind::Transport(ProviderTransportKind::Connect),
        ProviderFailureKind::StreamRead(ProviderTransportKind::UnexpectedEof),
    ] {
        assert_eq!(
            classify_runtime_error(&DenError::Provider(ProviderFailure::new(kind))),
            RuntimeErrorCategory::Internal
        );
    }
}

#[test]
fn non_provider_classifier_order_is_unchanged() {
    for (error, category) in [
        (
            DenError::System("waiting for approval; timeout".into()),
            RuntimeErrorCategory::ConflictPendingApproval,
        ),
        (
            DenError::System("no active runs to cancel".into()),
            RuntimeErrorCategory::Cancelled,
        ),
        (
            DenError::System("runtime is not configured".into()),
            RuntimeErrorCategory::Misconfigured,
        ),
        (
            DenError::NotFound("not found for this bear".into()),
            RuntimeErrorCategory::InvalidIdentity,
        ),
        (
            DenError::Authorization("timed out".into()),
            RuntimeErrorCategory::PermissionDenied,
        ),
        (
            DenError::System("request timed out".into()),
            RuntimeErrorCategory::Timeout,
        ),
        (
            DenError::System("temporarily unavailable".into()),
            RuntimeErrorCategory::Unavailable,
        ),
        (
            DenError::System("other error".into()),
            RuntimeErrorCategory::Internal,
        ),
    ] {
        assert_eq!(classify_runtime_error(&error), category);
    }
}
