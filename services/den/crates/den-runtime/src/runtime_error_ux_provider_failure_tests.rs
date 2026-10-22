use super::*;
use den_core::{ProviderFailureKind, ProviderHttpStatus, ProviderTransportKind};
use den_llm::provider_failure::classify_provider_error;

const CANARY: &str = "PRIVATE_PROVIDER_BODY_CANARY";

#[test]
fn provider_failure_projection_contains_only_safe_diagnostics_and_existing_recovery_prose() {
    let failure = classify_provider_error(&json!({
        "code": "overloaded_error",
        "message": CANARY,
        "type": CANARY,
        "request": {"prompt": CANARY},
    }))
    .with_http_status(ProviderHttpStatus::new(503).unwrap())
    .with_key_selection_fallback(ProviderHttpStatus::new(400).unwrap())
    .with_retry_exhausted(4);
    let input = RuntimeFailureInput::from(DenError::Provider(failure));
    let projection = input.projection("runtime_internal", "run-1", "Builder Bear", None);
    let user = projection.user_message.as_deref().unwrap();
    assert!(user.contains("waited 2 seconds, then 4 seconds, then 54 seconds"));
    assert!(user.contains("send another message to retry"));
    assert_eq!(projection.history_marker.as_deref(), Some(user));
    let context = projection.diagnostic_context.as_ref().unwrap();
    assert_eq!(
        context["diagnostic"]["class"],
        "llm_provider_retry_exhausted"
    );
    assert_eq!(context["diagnostic"]["provider_failure"], json!(failure));
    assert_eq!(
        context["diagnostic"]["retry_pauses_seconds"],
        json!([2, 4, 54])
    );
    assert!(context["diagnostic"].get("raw_message").is_none());
    assert_eq!(projection.content["detail"], input.message());
    let persisted = json!({
        "model_summary": projection.model_summary,
        "operational_outcome": projection.content,
        "user_message": projection.user_message,
        "history_marker": projection.history_marker,
        "diagnostic_context": projection.diagnostic_context,
        "message": input.message(),
    });
    assert!(!persisted.to_string().contains(CANARY));
}

#[test]
fn typed_idle_timeout_is_recognized_on_initial_and_continuation_failures() {
    let failure = ProviderFailure::new(ProviderFailureKind::IdleTimeout);
    for reason in ["stream_error", "continuation_stream_error"] {
        let input = RuntimeFailureInput::from(DenError::Provider(failure));
        let projection = input.projection(reason, "run-1", "Builder Bear", None);
        let user = projection.user_message.as_deref().unwrap();
        assert!(user.contains("produced no data for 30 seconds"));
        assert!(user.contains("Recent tool results were preserved"));
        assert_eq!(projection.history_marker.as_deref(), Some(user));
        let diagnostic = &projection.diagnostic_context.as_ref().unwrap()["diagnostic"];
        assert_eq!(diagnostic["class"], "llm_stream_idle_timeout");
        assert_eq!(diagnostic["idle_timeout_seconds"], 30);
        assert_eq!(diagnostic["provider_failure"], json!(failure));
    }
}

#[test]
fn provider_kinds_do_not_accidentally_activate_legacy_message_classifiers() {
    for kind in [
        ProviderFailureKind::Unknown,
        ProviderFailureKind::ContextOverflow,
        ProviderFailureKind::KeySelection,
        ProviderFailureKind::VirtualKeyNotFound,
        ProviderFailureKind::RateLimited,
        ProviderFailureKind::Unavailable,
        ProviderFailureKind::Timeout,
        ProviderFailureKind::Transport(ProviderTransportKind::Timeout),
        ProviderFailureKind::StreamRead(ProviderTransportKind::UnexpectedEof),
    ] {
        let failure = ProviderFailure::new(kind);
        let projection = RuntimeFailureInput::from(DenError::Provider(failure)).projection(
            "runtime_internal",
            "run-1",
            "Builder Bear",
            Some(json!([1, 2])),
        );
        assert!(projection.user_message.is_none(), "{kind:?}");
        assert!(projection.history_marker.is_none());
        let context = projection.diagnostic_context.unwrap();
        assert_eq!(context["diagnostic"]["class"], kind.code());
        assert_eq!(context["diagnostic"]["provider_failure"], json!(failure));
        assert_eq!(context["previous_context"], json!([1, 2]));
    }
}

#[test]
fn exhausted_idle_timeout_keeps_exhaustion_precedence_and_both_typed_facts() {
    let failure = ProviderFailure::new(ProviderFailureKind::IdleTimeout).with_retry_exhausted(4);
    let projection = RuntimeFailureInput::from(DenError::Provider(failure)).projection(
        "continuation_stream_error",
        "run-1",
        "Builder Bear",
        None,
    );
    assert!(projection
        .user_message
        .unwrap()
        .contains("could not reach the LLM provider after retrying"));
    let context = projection.diagnostic_context.unwrap();
    assert_eq!(
        context["diagnostic"]["class"],
        "llm_provider_retry_exhausted"
    );
    assert_eq!(context["diagnostic"]["provider_failure"], json!(failure));
}

#[test]
fn legacy_error_input_matches_existing_projection() {
    let error = DenError::System("wall-clock budget (elapsed=10ms/limit=5ms)".into());
    let message = error.to_string();
    let expected =
        run_failure_projection("runtime_internal", &message, "run-1", "Builder Bear", None);
    let actual = RuntimeFailureInput::from(error).projection(
        "runtime_internal",
        "run-1",
        "Builder Bear",
        None,
    );
    assert_eq!(actual, expected);
}
