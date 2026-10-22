//! Content-free inference failures shared by provider boundaries and runtime policy.
//!
//! Raw upstream bodies, messages, URLs, headers and error sources must be discarded
//! before constructing these values. Neither formatting nor serialization can recover them.

use std::{fmt, num::NonZeroU32};

use serde::Serialize;

/// A checked HTTP status, without a provider-controlled reason phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProviderHttpStatus(u16);

impl ProviderHttpStatus {
    pub const fn new(status: u16) -> Option<Self> {
        if status >= 100 && status <= 999 {
            Some(Self(status))
        } else {
            None
        }
    }

    pub const fn as_u16(self) -> u16 {
        self.0
    }

    pub const fn request_retryable(self) -> bool {
        matches!(self.0, 408 | 429 | 500..=599)
    }
}

impl fmt::Display for ProviderHttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransportKind {
    Timeout,
    Connect,
    ConnectionReset,
    ConnectionClosed,
    ConnectionAborted,
    BrokenPipe,
    UnexpectedEof,
    Body,
    Decode,
    Request,
    Builder,
    Redirect,
    Other,
}

impl ProviderTransportKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::ConnectionReset => "connection_reset",
            Self::ConnectionClosed => "connection_closed",
            Self::ConnectionAborted => "connection_aborted",
            Self::BrokenPipe => "broken_pipe",
            Self::UnexpectedEof => "unexpected_eof",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Request => "request",
            Self::Builder => "builder",
            Self::Redirect => "redirect",
            Self::Other => "other",
        }
    }

    pub const fn continuation_retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::ConnectionReset
                | Self::ConnectionClosed
                | Self::ConnectionAborted
                | Self::BrokenPipe
                | Self::UnexpectedEof
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    Unknown,
    ContextOverflow,
    KeySelection,
    VirtualKeyNotFound,
    RateLimited,
    Unavailable,
    Timeout,
    Transport(ProviderTransportKind),
    StreamRead(ProviderTransportKind),
    IdleTimeout,
}

impl ProviderFailureKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unknown => "provider_error",
            Self::ContextOverflow => "context_overflow",
            Self::KeySelection => "key_selection",
            Self::VirtualKeyNotFound => "virtual_key_not_found",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "provider_unavailable",
            Self::Timeout => "provider_timeout",
            Self::Transport(_) => "provider_transport",
            Self::StreamRead(_) => "provider_stream_read",
            Self::IdleTimeout => "provider_idle_timeout",
        }
    }
}

/// Contains only Den-owned labels and numeric diagnostics; no raw cause is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderFailure {
    kind: ProviderFailureKind,
    http_status: Option<ProviderHttpStatus>,
    retry_attempts: NonZeroU32,
    retry_exhausted: bool,
    key_selection_fallback_status: Option<ProviderHttpStatus>,
}

impl ProviderFailure {
    pub const fn new(kind: ProviderFailureKind) -> Self {
        Self {
            kind,
            http_status: None,
            retry_attempts: NonZeroU32::MIN,
            retry_exhausted: false,
            key_selection_fallback_status: None,
        }
    }

    pub const fn with_http_status(mut self, status: ProviderHttpStatus) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Counts sends, including the initial attempt. Zero is normalized to one.
    pub const fn with_retry_attempts(mut self, attempts: u32) -> Self {
        self.retry_attempts = match NonZeroU32::new(attempts) {
            Some(attempts) => attempts,
            None => NonZeroU32::MIN,
        };
        self
    }

    pub const fn with_retry_exhausted(mut self, attempts: u32) -> Self {
        self = self.with_retry_attempts(attempts);
        self.retry_exhausted = true;
        self
    }

    /// Records the original rejection while keeping the final failure's classification.
    pub const fn with_key_selection_fallback(
        mut self,
        original_status: ProviderHttpStatus,
    ) -> Self {
        self.key_selection_fallback_status = Some(original_status);
        self
    }

    pub const fn kind(&self) -> ProviderFailureKind {
        self.kind
    }

    pub const fn http_status(&self) -> Option<ProviderHttpStatus> {
        self.http_status
    }

    pub const fn key_selection_fallback_status(&self) -> Option<ProviderHttpStatus> {
        self.key_selection_fallback_status
    }

    pub const fn retry_attempts(&self) -> u32 {
        self.retry_attempts.get()
    }

    pub const fn retry_exhausted(&self) -> bool {
        self.retry_exhausted
    }

    pub const fn is_context_overflow(&self) -> bool {
        matches!(self.kind, ProviderFailureKind::ContextOverflow)
    }

    pub const fn transport_kind(&self) -> Option<ProviderTransportKind> {
        match self.kind {
            ProviderFailureKind::Transport(kind) | ProviderFailureKind::StreamRead(kind) => {
                Some(kind)
            }
            _ => None,
        }
    }

    pub const fn is_timeout(&self) -> bool {
        matches!(
            self.kind,
            ProviderFailureKind::Timeout
                | ProviderFailureKind::IdleTimeout
                | ProviderFailureKind::Transport(ProviderTransportKind::Timeout)
                | ProviderFailureKind::StreamRead(ProviderTransportKind::Timeout)
        ) || matches!(self.http_status, Some(status) if status.as_u16() == 408 || status.as_u16() == 504)
    }

    pub const fn is_idle_timeout(&self) -> bool {
        matches!(self.kind, ProviderFailureKind::IdleTimeout)
    }

    /// Separate from the request retry budget: exhaustion does not suppress the
    /// existing outer continuation retry policy.
    pub const fn continuation_retryable(&self) -> bool {
        if matches!(self.http_status, Some(status) if matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504))
        {
            return true;
        }
        match self.kind {
            ProviderFailureKind::RateLimited
            | ProviderFailureKind::Unavailable
            | ProviderFailureKind::Timeout
            | ProviderFailureKind::IdleTimeout => true,
            ProviderFailureKind::Transport(kind) | ProviderFailureKind::StreamRead(kind) => {
                kind.continuation_retryable()
            }
            _ => false,
        }
    }
}

impl fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LLM provider failure: {}", self.kind.code())?;
        if let Some(status) = self.http_status {
            write!(f, "; HTTP {status}")?;
        }
        if let Some(kind) = self.transport_kind() {
            write!(f, "; transport={}", kind.code())?;
        }
        write!(
            f,
            "; attempts={}; retry_exhausted={}",
            self.retry_attempts, self.retry_exhausted
        )?;
        if let Some(status) = self.key_selection_fallback_status {
            write!(f, "; key_selection_fallback_from_http={status}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderFailure {}

#[cfg(test)]
#[path = "provider_failure_tests.rs"]
mod tests;
