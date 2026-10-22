//! Inference error boundary. Unknown upstream data is inspected only here and
//! reduced to content-free `den_core::ProviderFailure` values.

use std::{error::Error, fmt, io, marker::PhantomData};

use den_core::{ProviderFailure, ProviderFailureKind, ProviderHttpStatus, ProviderTransportKind};
use serde::{
    de::{value::MapAccessDeserializer, IgnoredAny, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;

/// Oversized or incomplete bodies are not classified from a potentially misleading prefix.
pub const MAX_PROVIDER_ERROR_BYTES: usize = 64 * 1024;
const MAX_ERROR_FIELD_BYTES: usize = 8 * 1024;

// Derived struct deserializers also accept positional sequences. Require a map
// at every envelope/object boundary without buffering or reparsing JSON.
struct MapOnly<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for MapOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for MapVisitor<T> {
            type Value = MapOnly<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a provider error object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(MapOnly)
            }
        }

        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

#[derive(Default, Deserialize)]
struct ErrorEnvelope {
    error: Option<MapOnly<ErrorObject>>,
    response: Option<MapOnly<ResponseErrorEnvelope>>,
}

#[derive(Deserialize)]
struct ResponseErrorEnvelope {
    error: Option<MapOnly<ErrorObject>>,
}

#[derive(Default, Deserialize)]
struct ErrorObject {
    code: Option<BoundaryText>,
    #[serde(rename = "type")]
    error_type: Option<BoundaryText>,
    message: Option<BoundaryText>,
}

// Numeric codes and arbitrary objects are tolerated but never propagated. All
// allocations are local to this capped HTTP-boundary parse.
#[derive(Deserialize)]
#[serde(untagged)]
enum BoundaryText {
    Text(String),
    Other(IgnoredAny),
}

impl BoundaryText {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Other(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
enum KnownErrorCode {
    ContextOverflow,
    KeySelection,
    VirtualKeyNotFound,
    RateLimited,
    Unavailable,
    Timeout,
}

impl KnownErrorCode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "context_length_exceeded"
            | "context_window_exceeded"
            | "context_length"
            | "context_window"
            | "max_tokens"
            | "token_limit"
            | "max_tokens_exceeded"
            | "token_limit_exceeded"
            | "prompt_too_long"
            | "input_too_long" => Some(Self::ContextOverflow),
            "key_selection_error"
            | "key-selection_error"
            | "key-selection error"
            | "no_keys_found" => Some(Self::KeySelection),
            "virtual_key_not_found" => Some(Self::VirtualKeyNotFound),
            "rate_limit_exceeded" | "rate_limit_error" | "rate_limit" | "too_many_requests" => {
                Some(Self::RateLimited)
            }
            "overloaded_error" | "overloaded" | "server_overloaded" | "service_unavailable" => {
                Some(Self::Unavailable)
            }
            "timeout" | "timeout_error" | "request_timeout" | "gateway_timeout" => {
                Some(Self::Timeout)
            }
            _ => None,
        }
    }

    const fn kind(self) -> ProviderFailureKind {
        match self {
            Self::ContextOverflow => ProviderFailureKind::ContextOverflow,
            Self::KeySelection => ProviderFailureKind::KeySelection,
            Self::VirtualKeyNotFound => ProviderFailureKind::VirtualKeyNotFound,
            Self::RateLimited => ProviderFailureKind::RateLimited,
            Self::Unavailable => ProviderFailureKind::Unavailable,
            Self::Timeout => ProviderFailureKind::Timeout,
        }
    }
}

fn bounded_field(value: Option<&str>) -> Option<&str> {
    value.filter(|value| value.len() <= MAX_ERROR_FIELD_BYTES)
}

fn classify_parts(
    code: Option<&str>,
    error_type: Option<&str>,
    message: Option<&str>,
) -> ProviderFailure {
    let kind = bounded_field(code)
        .and_then(KnownErrorCode::parse)
        .or_else(|| bounded_field(error_type).and_then(KnownErrorCode::parse))
        .map(KnownErrorCode::kind)
        .unwrap_or_else(|| {
            bounded_field(message).map_or(ProviderFailureKind::Unknown, classify_legacy_message)
        });
    ProviderFailure::new(kind)
}

/// Classify a *selected error object* from an already-decoded SSE frame. Pass
/// `frame["error"]` or `frame["response"]["error"]`, not the whole frame.
/// No JSON serialization/reparse or search through request/extra fields occurs.
pub fn classify_provider_error(error: &Value) -> ProviderFailure {
    classify_parts(
        error.get("code").and_then(Value::as_str),
        error.get("type").and_then(Value::as_str),
        error.get("message").and_then(Value::as_str),
    )
}

/// Decode an HTTP error envelope once. Only error code/type/message fields are
/// considered; echoed requests and arbitrary nested metadata have no authority.
pub fn classify_http_failure(status: ProviderHttpStatus, body: &[u8]) -> ProviderFailure {
    let failure = if body.len() > MAX_PROVIDER_ERROR_BYTES {
        ProviderFailure::new(ProviderFailureKind::Unknown)
    } else if let Some(envelope) = parse_http_envelope(body) {
        envelope
            .error
            .or_else(|| envelope.response.and_then(|response| response.0.error))
            .map_or_else(
                || ProviderFailure::new(ProviderFailureKind::Unknown),
                |MapOnly(error)| {
                    classify_parts(
                        error.code.as_ref().and_then(BoundaryText::as_str),
                        error.error_type.as_ref().and_then(BoundaryText::as_str),
                        error.message.as_ref().and_then(BoundaryText::as_str),
                    )
                },
            )
    } else {
        // Malformed JSON/HTML must not fall back to scanning arbitrary serialized
        // request fields. Legacy compatibility is limited to plain error text.
        let kind = std::str::from_utf8(body)
            .ok()
            .map(str::trim)
            .filter(|text| text.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
            .filter(|text| !text.contains(['{', '}', '[', ']', '<', '>']))
            .map_or(ProviderFailureKind::Unknown, |text| {
                bounded_field(Some(text))
                    .map_or(ProviderFailureKind::Unknown, classify_legacy_message)
            });
        ProviderFailure::new(kind)
    };
    failure.with_http_status(status)
}

fn parse_http_envelope(body: &[u8]) -> Option<ErrorEnvelope> {
    serde_json::from_slice::<MapOnly<ErrorEnvelope>>(body)
        .ok()
        .map(|envelope| envelope.0)
}

fn classify_legacy_message(message: &str) -> ProviderFailureKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("virtual_key_not_found") || lower.contains("virtual key not found") {
        ProviderFailureKind::VirtualKeyNotFound
    } else if lower.contains("no keys found that support model")
        || (lower.contains("key-selection error") && lower.contains("no keys"))
    {
        ProviderFailureKind::KeySelection
    } else if [
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
    {
        ProviderFailureKind::ContextOverflow
    } else if ["too many requests", "rate limit", "rate_limit"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        ProviderFailureKind::RateLimited
    } else if [
        "request timeout",
        "request timed out",
        "operation timed out",
        "gateway timeout",
        "upstream timeout",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        ProviderFailureKind::Timeout
    } else if lower.contains("temporarily unavailable") || lower.contains("overloaded") {
        ProviderFailureKind::Unavailable
    } else {
        ProviderFailureKind::Unknown
    }
}

/// Classify transport flags and typed I/O sources without rendering any source.
pub fn classify_transport_failure(error: &reqwest::Error) -> ProviderFailure {
    ProviderFailure::new(ProviderFailureKind::Transport(transport_kind(error)))
}

pub(super) fn classify_stream_read_failure(error: &reqwest::Error) -> ProviderFailure {
    ProviderFailure::new(ProviderFailureKind::StreamRead(transport_kind(error)))
}

fn transport_kind(error: &reqwest::Error) -> ProviderTransportKind {
    if error.is_timeout() {
        return ProviderTransportKind::Timeout;
    }
    let mut source = error.source();
    // Bound traversal even if a third-party Error implementation has a source cycle.
    for _ in 0..16 {
        let Some(cause) = source else { break };
        if let Some(error) = cause.downcast_ref::<io::Error>() {
            let kind = match error.kind() {
                io::ErrorKind::TimedOut => Some(ProviderTransportKind::Timeout),
                io::ErrorKind::ConnectionReset => Some(ProviderTransportKind::ConnectionReset),
                io::ErrorKind::NotConnected => Some(ProviderTransportKind::ConnectionClosed),
                io::ErrorKind::ConnectionAborted => Some(ProviderTransportKind::ConnectionAborted),
                io::ErrorKind::BrokenPipe => Some(ProviderTransportKind::BrokenPipe),
                io::ErrorKind::UnexpectedEof => Some(ProviderTransportKind::UnexpectedEof),
                _ => None,
            };
            if let Some(kind) = kind {
                return kind;
            }
        }
        source = cause.source();
    }
    if error.is_connect() {
        ProviderTransportKind::Connect
    } else if error.is_builder() {
        ProviderTransportKind::Builder
    } else if error.is_redirect() {
        ProviderTransportKind::Redirect
    } else if error.is_decode() {
        ProviderTransportKind::Decode
    } else if error.is_body() {
        ProviderTransportKind::Body
    } else if error.is_request() {
        ProviderTransportKind::Request
    } else {
        ProviderTransportKind::Other
    }
}

pub(super) async fn read_http_failure(mut response: reqwest::Response) -> ProviderFailure {
    let status = ProviderHttpStatus::new(response.status().as_u16())
        .expect("reqwest supplies a valid HTTP status");
    let unknown = ProviderFailure::new(ProviderFailureKind::Unknown).with_http_status(status);
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > MAX_PROVIDER_ERROR_BYTES.saturating_sub(body.len()) {
                    return unknown;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return classify_http_failure(status, &body),
            // A partial body is not authoritative. Do not retain or render its cause.
            Err(_) => return unknown,
        }
    }
}

#[cfg(test)]
#[path = "provider_failure_tests.rs"]
mod tests;
