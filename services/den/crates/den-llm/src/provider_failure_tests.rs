use super::*;
use serde_json::json;

fn classify(body: &[u8]) -> ProviderFailure {
    classify_http_failure(ProviderHttpStatus::new(400).unwrap(), body)
}

#[test]
fn known_structured_codes_reduce_to_closed_kinds() {
    for (code, kind) in [
        (
            "context_length_exceeded",
            ProviderFailureKind::ContextOverflow,
        ),
        (
            "context_window_exceeded",
            ProviderFailureKind::ContextOverflow,
        ),
        (
            "virtual_key_not_found",
            ProviderFailureKind::VirtualKeyNotFound,
        ),
        ("key_selection_error", ProviderFailureKind::KeySelection),
        ("rate_limit_error", ProviderFailureKind::RateLimited),
        ("overloaded_error", ProviderFailureKind::Unavailable),
        ("request_timeout", ProviderFailureKind::Timeout),
    ] {
        for field in ["code", "type"] {
            let mut error = json!({ "message": "PRIVATE_CANARY" });
            error[field] = json!(code);
            let failure = classify_provider_error(&error);
            assert_eq!(failure.kind(), kind);
            let body = serde_json::to_vec(&json!({ "error": error })).unwrap();
            assert_eq!(classify(&body).kind(), kind);
            assert!(!format!("{failure:?} {failure}").contains("PRIVATE_CANARY"));
        }
    }
}

#[test]
fn legacy_overflow_phrases_work_only_in_error_message_or_plain_body() {
    for message in [
        "context length exceeded",
        "maximum context exceeded",
        "max context exceeded",
        "token limit reached",
        "too many tokens in request",
        "context_window exceeded",
        "context window exceeded",
        "max_tokens exceeded",
        "context_length_exceeded",
        "context_length rejected",
        "maximum number of tokens exceeded",
        "prompt is too long",
        "request too large",
        "input is too long",
    ] {
        assert!(classify(message.as_bytes()).is_context_overflow());
        let error = json!({ "message": message });
        assert!(classify_provider_error(&error).is_context_overflow());
        let body = serde_json::to_vec(&json!({ "error": error })).unwrap();
        assert!(classify(&body).is_context_overflow());
    }
}

#[test]
fn legacy_bifrost_errors_keep_their_routing_classification() {
    for (message, kind) in [
        (
            "no keys found that support model: gpt-5.5",
            ProviderFailureKind::KeySelection,
        ),
        (
            "key-selection error: no keys available",
            ProviderFailureKind::KeySelection,
        ),
        (
            "Virtual key not found",
            ProviderFailureKind::VirtualKeyNotFound,
        ),
        (
            "virtual_key_not_found",
            ProviderFailureKind::VirtualKeyNotFound,
        ),
    ] {
        let body = serde_json::to_vec(&json!({
            "is_bifrost_error": false,
            "error": { "error": "ignored PRIVATE_CANARY", "message": message },
            "extra_fields": { "request": "PRIVATE_CANARY" }
        }))
        .unwrap();
        assert_eq!(classify(&body).kind(), kind);
        assert_eq!(classify(message.as_bytes()).kind(), kind);
    }
}

#[test]
fn nested_request_echoes_and_unknown_types_are_not_control_data() {
    for error in [
        json!({"code": "PRIVATE_CANARY", "type": "context_length_exceeded PRIVATE_CANARY"}),
        json!({"request": {"message": "context_length_exceeded", "error": {"code": "key_selection_error"}}}),
        json!({"message": {"message": "context length exceeded"}}),
        json!({"code": ["virtual_key_not_found"], "type": {"message": "no keys found that support model"}}),
        json!({"error": "context_length_exceeded"}),
        json!("context_length_exceeded"),
        Value::Null,
    ] {
        let failure = classify_provider_error(&error);
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert!(!failure.is_context_overflow());
        assert!(!failure.continuation_retryable());
        assert!(!format!("{failure:?} {failure}").contains("PRIVATE_CANARY"));
    }
    for body in [
        json!({"request":{"error":{"code":"context_length_exceeded"}}}),
        json!({"error":{"message":"PRIVATE_CANARY"},"extra_fields":{"message":"context_length_exceeded"}}),
        json!({"message":"context_length_exceeded"}),
    ] {
        assert_eq!(
            classify(&serde_json::to_vec(&body).unwrap()).kind(),
            ProviderFailureKind::Unknown
        );
    }
}

#[test]
fn malformed_json_html_binary_and_rendered_errors_are_unknown() {
    for body in [
        br#"{"error":{"message":"context_length_exceeded"}"#.as_slice(),
        br#"["context_length_exceeded"]"#.as_slice(),
        br#"[{"message":"context_length_exceeded"}]"#.as_slice(),
        br#""context_length_exceeded""#.as_slice(),
        b"<html>context_length_exceeded PRIVATE_CANARY</html>".as_slice(),
        b"Server Error: HTTP 400: {\"error\":{\"message\":\"no keys found that support model\"}}"
            .as_slice(),
        b"\xffcontext_length_exceeded".as_slice(),
        b"".as_slice(),
    ] {
        let failure = classify(body);
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert_eq!(failure.http_status().unwrap().as_u16(), 400);
    }
}

#[test]
fn nested_http_envelopes_require_objects_not_positional_arrays() {
    for positional in [
        json!([null, null, "no keys found that support model"]),
        json!(["context_length_exceeded", null, null]),
    ] {
        assert_eq!(
            classify_provider_error(&positional).kind(),
            ProviderFailureKind::Unknown
        );
        for envelope in [
            json!({"error": positional.clone()}),
            json!({"response": {"error": positional.clone()}}),
            json!({"response": [positional]}),
        ] {
            let failure = classify(&serde_json::to_vec(&envelope).unwrap());
            assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
            assert_eq!(failure.http_status().unwrap().as_u16(), 400);
            assert!(!failure.is_context_overflow());
        }
    }
    for envelope in [
        json!({"response": [{"code": "context_length_exceeded"}]}),
        json!({"response": [{"message": "no keys found that support model"}]}),
        json!({"error": {"code": "context_length_exceeded"}, "response": []}),
        json!({"error": [], "response": {"error": {"code": "context_length_exceeded"}}}),
    ] {
        let failure = classify(&serde_json::to_vec(&envelope).unwrap());
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert_eq!(failure.http_status().unwrap().as_u16(), 400);
    }
    for (error, expected) in [
        (
            json!({"code": "context_length_exceeded"}),
            ProviderFailureKind::ContextOverflow,
        ),
        (
            json!({"message": "no keys found that support model"}),
            ProviderFailureKind::KeySelection,
        ),
    ] {
        for envelope in [
            json!({"error": error.clone()}),
            json!({"response": {"error": error}}),
        ] {
            assert_eq!(
                classify(&serde_json::to_vec(&envelope).unwrap()).kind(),
                expected
            );
        }
    }
}

#[test]
fn numeric_codes_and_escaped_messages_are_tolerated_at_json_boundary() {
    assert!(
        classify(br#"{"error":{"code":400,"type":null,"message":"prompt\u0020is too long"}}"#)
            .is_context_overflow()
    );
    assert_eq!(
        classify(br#"{"response":{"error":{"code":"virtual_key_not_found"}}}"#).kind(),
        ProviderFailureKind::VirtualKeyNotFound
    );
}

#[test]
fn known_code_takes_precedence_over_incompatible_message() {
    let failure = classify_provider_error(&json!({
        "code": "virtual_key_not_found",
        "message": "context_length_exceeded PRIVATE_CANARY"
    }));
    assert_eq!(failure.kind(), ProviderFailureKind::VirtualKeyNotFound);
    assert!(!failure.is_context_overflow());
}

#[test]
fn body_and_field_caps_do_not_classify_prefixes() {
    let mut body = br#"{"error":{"code":"context_length_exceeded","message":""#.to_vec();
    body.extend(std::iter::repeat_n(b'x', MAX_PROVIDER_ERROR_BYTES));
    body.extend_from_slice(br#""}}"#);
    assert_eq!(classify(&body).kind(), ProviderFailureKind::Unknown);
    let oversized = format!(
        "context_length_exceeded {}",
        "x".repeat(MAX_ERROR_FIELD_BYTES)
    );
    assert_eq!(
        classify_provider_error(&json!({"message": oversized})).kind(),
        ProviderFailureKind::Unknown
    );
    assert_eq!(
        classify(oversized.as_bytes()).kind(),
        ProviderFailureKind::Unknown
    );
}

#[test]
fn transient_legacy_messages_preserve_typed_continuation_hints() {
    for message in [
        "too many requests",
        "rate limit",
        "rate_limit",
        "request timeout",
        "request timed out",
        "operation timed out",
        "gateway timeout",
        "upstream timeout",
        "temporarily unavailable",
        "overloaded",
    ] {
        assert!(classify_provider_error(&json!({"message": message})).continuation_retryable());
    }
}

#[test]
fn http_status_retains_retryability_when_body_is_unknown() {
    let failure = classify_http_failure(ProviderHttpStatus::new(503).unwrap(), b"PRIVATE_CANARY");
    assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
    assert!(failure.continuation_retryable());
    assert_eq!(failure.http_status().unwrap().as_u16(), 503);
    assert!(!format!("{failure:?} {failure}").contains("PRIVATE_CANARY"));
}
