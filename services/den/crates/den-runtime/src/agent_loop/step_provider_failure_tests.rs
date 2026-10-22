use super::*;
use den_core::{ProviderFailure, ProviderFailureKind, ProviderHttpStatus};

#[tokio::test]
async fn typed_handshake_failure_is_emitted_once_without_string_conversion() {
    let failure = ProviderFailure::new(ProviderFailureKind::ContextOverflow)
        .with_http_status(ProviderHttpStatus::new(400).unwrap())
        .with_retry_attempts(2);
    let mut stream = LazyAgentStepStream {
        state: Some(LazyAgentStepState::Init {
            fut: Box::pin(async move { Err(DenError::Provider(failure)) }),
        }),
    };
    assert!(
        matches!(stream.next().await, Some(Err(DenError::Provider(actual))) if actual == failure)
    );
    assert!(stream.next().await.is_none());
    assert!(stream.next().await.is_none());
}

#[test]
fn overflow_recovery_rejections_preserve_provider_classification_and_diagnostics() {
    let failure = ProviderFailure::new(ProviderFailureKind::ContextOverflow)
        .with_http_status(ProviderHttpStatus::new(413).unwrap())
        .with_retry_exhausted(4);
    for message in [
        "LLM context overflow persists after emergency compaction retry",
        "LLM context overflow; enable COMPACTION_MODE=active for emergency recovery",
        "LLM context overflow; emergency compaction did not shrink prompt",
    ] {
        let error = overflow_recovery_failure(DenError::Provider(failure), message);
        assert!(matches!(error, DenError::Provider(actual) if actual == failure));
        assert!(den_error_indicates_context_overflow(&error));
        assert!(matches!(
            overflow_recovery_failure(DenError::System("context length exceeded".into()), message),
            DenError::System(actual) if actual == message
        ));
    }
}
