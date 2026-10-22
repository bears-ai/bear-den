use std::{future::ready, sync::Mutex};

use futures::StreamExt;

use super::*;

#[path = "client_failure_test_support.rs"]
mod support;
use support::*;

const STYLES: [LlmApiStyle; 2] = [
    LlmApiStyle::ChatCompletionsStream,
    LlmApiStyle::ResponsesStream,
];

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_contain_http_echoes_and_boundary_logs() {
    for style in STYLES {
        let server = Server::start(vec![Reply::http(400, canary_body("rejected"))]);
        let client = client(&server.url);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let failure = expect_provider(call(&client, style, &request()).await.unwrap_err());
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert_eq!(failure.http_status().unwrap().as_u16(), 400);
        assert_eq!(failure.retry_attempts(), 1);
        assert!(!failure.retry_exhausted());
        let logs = capture.output();
        assert!(logs.contains("LLM provider request failed"));
        assert!(logs.contains("http_status=400"));
        assert_safe(&logs);
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            header(&requests[0], "x-bf-vk"),
            Some("HEADER_SECRET_CANARY")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn positional_error_arrays_never_trigger_key_selection_fallback() {
    for style in STYLES {
        let server = Server::start(vec![Reply::http(
            400,
            r#"{"error":[null,null,"no keys found that support model"]}"#,
        )]);
        let client = client(&server.url);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let failure = expect_provider(call(&client, style, &request()).await.unwrap_err());
        let requests = server.finish();
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert_eq!(failure.http_status().unwrap().as_u16(), 400);
        // Counting sends also detects an attempted fallback after the fixture closes.
        assert_eq!(failure.retry_attempts(), 1);
        assert_eq!(failure.key_selection_fallback_status(), None);
        assert_eq!(requests.len(), 1);
        for name in [
            "x-bf-session-id",
            "x-bf-cache-key",
            "x-bf-cache-type",
            "x-bf-vk",
        ] {
            assert!(header(&requests[0], name).is_some());
        }
        assert!(!capture
            .output()
            .contains("without Bifrost session/cache headers"));
        assert_safe(&capture.output());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_preserve_success_body_and_request_serialization() {
    for style in STYLES {
        let server = Server::start(vec![Reply::http(
            200,
            "successful PERSONAL_CANARY response",
        )]);
        let client = client(&server.url);
        let request = request();
        let response = call(&client, style, &request).await.unwrap();
        assert_eq!(
            response.text().await.unwrap(),
            "successful PERSONAL_CANARY response"
        );
        let requests = server.finish();
        let (headers, body) = requests[0].split_once("\r\n\r\n").unwrap();
        let (path, expected) = match style {
            LlmApiStyle::ChatCompletionsStream => ("chat/completions", request.to_body()),
            LlmApiStyle::ResponsesStream => ("responses", request.to_responses_body()),
        };
        assert!(headers.starts_with(&format!("POST /v1/{path} HTTP/1.1")));
        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), expected);
        assert_eq!(
            header(&requests[0], "x-bears-operation"),
            Some("agent_turn")
        );
        assert_eq!(
            header(&requests[0], "x-bf-session-id"),
            Some("conversation-fixture")
        );
        assert_eq!(
            header(&requests[0], "x-bf-cache-key"),
            Some("conversation-fixture")
        );
        assert_eq!(header(&requests[0], "x-bf-cache-type"), Some("direct"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_preserve_overflow_and_virtual_key_classifications() {
    for style in STYLES {
        for (message, expected) in [
            (
                "context_length_exceeded",
                ProviderFailureKind::ContextOverflow,
            ),
            (
                "virtual key not found",
                ProviderFailureKind::VirtualKeyNotFound,
            ),
        ] {
            let server = Server::start(vec![Reply::http(400, canary_body(message))]);
            let client = client(&server.url);
            let capture = Capture::default();
            let _guard = tracing::subscriber::set_default(capture.clone());
            let failure = expect_provider(call(&client, style, &request()).await.unwrap_err());
            assert_eq!(failure.kind(), expected);
            assert_eq!(
                failure.is_context_overflow(),
                expected == ProviderFailureKind::ContextOverflow
            );
            assert_safe(&capture.output());
            assert_eq!(server.finish().len(), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_key_selection_fallback_preserves_wire_headers_body_and_success() {
    for style in STYLES {
        let server = Server::start(vec![
            Reply::http(400, canary_body("no keys found that support model")),
            Reply::http(200, "fallback PERSONAL_CANARY success"),
        ]);
        let client = client(&server.url);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let response = call(&client, style, &request()).await.unwrap();
        assert_eq!(
            response.text().await.unwrap(),
            "fallback PERSONAL_CANARY success"
        );
        assert_safe(&capture.output());
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].split_once("\r\n\r\n").unwrap().1,
            requests[1].split_once("\r\n\r\n").unwrap().1
        );
        for name in [
            "x-bf-vk",
            "x-bears-request-id",
            "x-bears-conversation-id",
            "x-bears-operation",
        ] {
            assert!(header(&requests[0], name).is_some());
            assert_eq!(header(&requests[0], name), header(&requests[1], name));
        }
        for name in ["x-bf-session-id", "x-bf-cache-key", "x-bf-cache-type"] {
            assert!(header(&requests[0], name).is_some());
            assert!(header(&requests[1], name).is_none());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_contain_both_fallback_bodies_and_keep_final_failure() {
    for style in STYLES {
        for (status, message, kind) in [
            (
                400,
                "context_length_exceeded",
                ProviderFailureKind::ContextOverflow,
            ),
            (503, "overloaded", ProviderFailureKind::Unavailable),
            (
                401,
                "virtual_key_not_found",
                ProviderFailureKind::VirtualKeyNotFound,
            ),
            (
                400,
                "no keys found that support model",
                ProviderFailureKind::KeySelection,
            ),
        ] {
            let server = Server::start(vec![
                Reply::http(400, canary_body("no keys found that support model")),
                Reply::http(status, canary_body(message)),
            ]);
            let client = client(&server.url);
            let capture = Capture::default();
            let _guard = tracing::subscriber::set_default(capture.clone());
            let failure = expect_provider(call(&client, style, &request()).await.unwrap_err());
            assert_eq!(failure.kind(), kind);
            assert_eq!(failure.http_status().unwrap().as_u16(), status);
            assert_eq!(
                failure.key_selection_fallback_status().unwrap().as_u16(),
                400
            );
            assert_eq!(failure.retry_attempts(), 2);
            // The special fallback remains a single send, even when it returns 503.
            assert!(!failure.retry_exhausted());
            assert_safe(&capture.output());
            assert_eq!(server.finish().len(), 2);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_endpoints_contain_key_selection_fallback_transport_failures() {
    for style in STYLES {
        let server = Server::start(vec![
            Reply::http(400, canary_body("no keys found that support model")),
            Reply::Disconnect,
        ]);
        let client = client(&server.url);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let failure = expect_provider(call(&client, style, &request()).await.unwrap_err());
        assert!(matches!(failure.kind(), ProviderFailureKind::Transport(_)));
        assert_eq!(
            failure.key_selection_fallback_status().unwrap().as_u16(),
            400
        );
        assert_eq!(failure.retry_attempts(), 2);
        assert_eq!(failure.http_status(), None);
        assert!(!failure.retry_exhausted());
        assert_safe(&capture.output());
        assert_eq!(server.finish().len(), 2);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retry_loop_preserves_four_sends_and_exact_pauses_without_real_waits() {
    for style in STYLES {
        for status in [408, 429, 500, 502, 503, 504, 599] {
            let server = Server::start(
                (0..4)
                    .map(|_| Reply::http(status, canary_body("overloaded")))
                    .collect(),
            );
            let http = reqwest::Client::new();
            let pauses = Mutex::new(Vec::new());
            let capture = Capture::default();
            let _guard = tracing::subscriber::set_default(capture.clone());
            let result = send_llm_provider_request_with_retry_delay(
                || http.post(&server.url).body("PERSONAL_CANARY"),
                style,
                Instant::now(),
                |delay| {
                    pauses.lock().unwrap().push(delay);
                    ready(())
                },
            )
            .await;
            let failure = expect_provider(result.unwrap_err());
            assert!(failure.retry_exhausted());
            assert_eq!(failure.retry_attempts(), 4);
            assert_eq!(failure.http_status().unwrap().as_u16(), status);
            assert!(failure.continuation_retryable());
            assert_eq!(*pauses.lock().unwrap(), LLM_PROVIDER_RETRY_DELAYS);
            assert_safe(&capture.output());
            assert_eq!(server.finish().len(), 4);
        }
    }
    assert_eq!(
        LLM_PROVIDER_RETRY_DELAYS.map(|delay| delay.as_secs()),
        [2, 4, 54]
    );
    for status in 100..=999 {
        let status = StatusCode::from_u16(status).unwrap();
        assert_eq!(
            llm_provider_error_retryable_status(status),
            matches!(status.as_u16(), 408 | 429 | 500..=599)
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retry_loop_stops_on_success_or_nonretry_status_and_counts_prior_sends() {
    for final_status in [200, 400] {
        let server = Server::start(vec![
            Reply::http(503, canary_body("overloaded")),
            Reply::http(final_status, "final body"),
        ]);
        let http = reqwest::Client::new();
        let mut pauses = Vec::new();
        let (response, attempts) = send_llm_provider_request_with_retry_delay(
            || http.post(&server.url),
            LlmApiStyle::ResponsesStream,
            Instant::now(),
            |delay| {
                pauses.push(delay);
                ready(())
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(response.status().as_u16(), final_status);
        assert_eq!(response.text().await.unwrap(), "final body");
        assert_eq!(pauses, [Duration::from_secs(2)]);
        assert_eq!(server.finish().len(), 2);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn all_send_errors_keep_retry_budget_without_retaining_transport_url() {
    let http = reqwest::Client::new();
    let mut sends = 0;
    let mut pauses = Vec::new();
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    let result = send_llm_provider_request_with_retry_delay(
        || {
            sends += 1;
            http.post("http://[URL_SECRET_CANARY")
        },
        LlmApiStyle::ChatCompletionsStream,
        Instant::now(),
        |delay| {
            pauses.push(delay);
            ready(())
        },
    )
    .await;
    let failure = expect_provider(result.unwrap_err());
    assert_eq!(sends, 4);
    assert_eq!(pauses, LLM_PROVIDER_RETRY_DELAYS);
    assert_eq!(
        failure.transport_kind(),
        Some(ProviderTransportKind::Builder)
    );
    assert_eq!(failure.retry_attempts(), 4);
    assert!(failure.retry_exhausted());
    assert_safe(&capture.output());
}

#[tokio::test(flavor = "current_thread")]
async fn both_byte_streams_contain_read_errors_without_touching_received_bytes() {
    for style in STYLES {
        let server = Server::start(vec![Reply::Http {
            status: 200,
            body: "PERSONAL_CANARY".to_string(),
            missing_bytes: 100,
        }]);
        let client = client(&server.url);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let stream = client
            .byte_stream_for_api_style(&request(), style)
            .await
            .unwrap();
        let mut stream = Box::pin(stream);
        let mut received = Vec::new();
        let failure = loop {
            match stream.next().await.expect("truncated stream must fail") {
                Ok(bytes) => received.extend_from_slice(&bytes),
                Err(error) => break expect_provider(error),
            }
        };
        assert!(matches!(failure.kind(), ProviderFailureKind::StreamRead(_)));
        // Whether Hyper yields the short body before detecting EOF is transport-dependent.
        assert!(received.is_empty() || received.as_slice() == b"PERSONAL_CANARY");
        assert_safe(&capture.output());
        assert_eq!(server.finish().len(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn capped_and_partial_http_error_bodies_are_unknown_not_prefix_classified() {
    for reply in [
        Reply::http(
            400,
            format!(
                "{{\"error\":{{\"code\":\"context_length_exceeded\",\"message\":\"{}\"}}}}",
                "x".repeat(provider_failure::MAX_PROVIDER_ERROR_BYTES)
            ),
        ),
        Reply::Http {
            status: 400,
            body: canary_body("context_length_exceeded"),
            missing_bytes: 100,
        },
    ] {
        let server = Server::start(vec![reply]);
        let client = client(&server.url);
        let failure = expect_provider(
            call(&client, LlmApiStyle::ResponsesStream, &request())
                .await
                .unwrap_err(),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Unknown);
        assert_eq!(failure.http_status().unwrap().as_u16(), 400);
        assert_eq!(server.finish().len(), 1);
    }
}

#[test]
fn optional_headers_are_sensitive_without_changing_wire_values() {
    let request = request();
    let telemetry = request.telemetry.as_ref();
    let builder = reqwest::Client::new().post("http://example.invalid/v1/responses");
    let builder = apply_bears_telemetry_headers(builder, telemetry);
    let builder = apply_bifrost_virtual_key_header(builder, telemetry);
    let request = apply_bifrost_session_cache_headers(builder, telemetry)
        .build()
        .unwrap();
    for name in [
        "x-bf-vk",
        "x-bears-request-id",
        "x-bears-conversation-id",
        "x-bears-operation",
        "x-bf-session-id",
        "x-bf-cache-key",
        "x-bf-cache-type",
    ] {
        assert!(request.headers().get(name).unwrap().is_sensitive());
    }
    let key = request.headers().get("x-bf-vk").unwrap();
    assert_eq!(key.to_str().unwrap(), "HEADER_SECRET_CANARY");
    assert_safe(&format!("{:?}", request.headers()));
}

#[test]
fn legacy_key_selection_helper_accepts_raw_errors_not_rendered_json_causes() {
    assert!(bifrost_key_selection_error(
        r#"{"error":{"message":"no keys found that support model: gpt-5.5"}}"#
    ));
    assert!(bifrost_key_selection_error("key-selection error: no keys"));
    assert!(!bifrost_key_selection_error(
        r#"Server Error: HTTP 400: {"error":{"message":"no keys found that support model"}}"#
    ));
    assert!(!bifrost_key_selection_error(
        r#"{"request":{"message":"no keys found that support model"}}"#
    ));
}
