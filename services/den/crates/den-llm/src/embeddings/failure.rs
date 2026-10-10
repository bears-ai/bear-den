use den_core::DenError;
use reqwest::StatusCode;

/// Classify only trusted transport metadata, never rendered errors or provider bodies.
pub(super) enum EmbeddingFailure {
    NotConfigured,
    MissingCredential,
    InvalidCredential,
    Transport,
    Timeout,
    Http(StatusCode),
    ResponseRead,
    ResponseParse,
}

impl From<EmbeddingFailure> for DenError {
    fn from(failure: EmbeddingFailure) -> Self {
        let message = match failure {
            EmbeddingFailure::NotConfigured => "embeddings API is not configured".into(),
            EmbeddingFailure::MissingCredential => "embeddings Bear credential is missing".into(),
            EmbeddingFailure::InvalidCredential => "embeddings Bear credential is invalid".into(),
            EmbeddingFailure::Transport => "embeddings transport failed".into(),
            EmbeddingFailure::Timeout => "embeddings request timed out".into(),
            EmbeddingFailure::Http(status) => {
                let reason = match status {
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "credential denied",
                    StatusCode::TOO_MANY_REQUESTS => "rate limited",
                    _ => "provider request failed",
                };
                format!("embeddings HTTP {}: {reason}", status.as_u16())
            }
            EmbeddingFailure::ResponseRead => "embeddings response read failed".into(),
            EmbeddingFailure::ResponseParse => "embeddings response is invalid JSON".into(),
        };
        DenError::System(message)
    }
}
