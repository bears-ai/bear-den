use super::*;
use den_core::{DenError, ProviderFailureKind, ProviderHttpStatus};
use den_llm::provider_failure::classify_http_failure;
use den_protocol::{classify_runtime_error, RuntimeErrorCategory};

const CANARY: &str = "PRIVATE_UPSTREAM_PERSISTENCE_CANARY";

#[test]
fn provider_http_to_protocol_and_all_failure_persistence_inputs_are_content_free() {
    for (code, kind, category) in [
        (
            "context_length_exceeded",
            ProviderFailureKind::ContextOverflow,
            RuntimeErrorCategory::Internal,
        ),
        (
            "overloaded_error",
            ProviderFailureKind::Unavailable,
            RuntimeErrorCategory::Unavailable,
        ),
        (
            "timeout_error",
            ProviderFailureKind::Timeout,
            RuntimeErrorCategory::Timeout,
        ),
        (
            CANARY,
            ProviderFailureKind::Unknown,
            RuntimeErrorCategory::Internal,
        ),
    ] {
        let body = json!({
            "error": {
                "code": code,
                "type": CANARY,
                "message": CANARY,
                "request": {"code": "rate_limit_error", "messages": [CANARY]},
                "extra": CANARY,
            },
            "request": {"code": "context_length_exceeded", "prompt": CANARY},
        });
        let failure = classify_http_failure(
            ProviderHttpStatus::new(400).unwrap(),
            body.to_string().as_bytes(),
        )
        .with_key_selection_fallback(ProviderHttpStatus::new(401).unwrap())
        .with_retry_exhausted(4);
        assert_eq!(failure.kind(), kind);
        assert_eq!(
            classify_runtime_error(&DenError::Provider(failure)),
            category
        );
        // The HTTP adapter also preserves the typed failure through startup errors.
        let error = CustomError::from(DenError::Provider(failure)).into_den();
        let input = RuntimeFailureInput::from(error);
        for reason in [
            RunFailureReason::StreamError,
            RunFailureReason::StartFailed,
            RunFailureReason::ContinuationStreamError,
            RunFailureReason::ContinuationStartFailed,
        ] {
            let inputs = run_failure_persistence_inputs(
                "session-1",
                "run-1",
                Uuid::nil(),
                reason,
                "Builder Bear",
                &input,
                Some(json!({"attempt": 4, "retryable": false, "retry_exhausted": true})),
            );
            let context = inputs.projection.diagnostic_context.as_ref().unwrap();
            assert_eq!(context["diagnostic"]["provider_failure"], json!(failure));
            assert_eq!(inputs.terminal["context"], *context);
            assert_eq!(inputs.work_outcome["forensics"], *context);
            assert_eq!(inputs.exception.details, *context);
            assert_eq!(inputs.terminal["message"], input.message());
            assert_eq!(inputs.exception.message, input.message());
            assert_eq!(inputs.work_outcome["message"], input.message());
            let wire = BearWireEvent::persistent_typed("run.failed", inputs.terminal.clone());
            let serialized = json!({
                "terminal": inputs.terminal,
                "work_outcome": inputs.work_outcome,
                "exception": {
                    "message": inputs.exception.message,
                    "details": inputs.exception.details,
                    "component": inputs.exception.component,
                    "code": inputs.exception.event_code,
                },
                "operational_outcome": inputs.projection.content,
                "model_summary": inputs.projection.model_summary,
                "user_message": inputs.projection.user_message,
                "history_marker": inputs.projection.history_marker,
                "diagnostic_context": inputs.projection.diagnostic_context,
                "wire": wire,
            })
            .to_string();
            assert!(!serialized.contains(CANARY), "{serialized}");
            assert!(!format!("{inputs:?}").contains(CANARY));
        }
    }
}
