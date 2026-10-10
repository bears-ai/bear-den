use std::fmt;

use den_core::DenError;
use reqwest::header::HeaderValue;

use super::failure::EmbeddingFailure;

/// A server-resolved Bear virtual-key secret, never a tool argument or model ID.
/// No serialization or secret accessor is exposed; HTTP debug output is redacted.
#[derive(Clone)]
pub struct BearEmbeddingCredential(HeaderValue);

impl BearEmbeddingCredential {
    pub fn from_server_secret(secret: String) -> Result<Self, DenError> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(EmbeddingFailure::MissingCredential.into());
        }
        let mut header = HeaderValue::from_str(secret)
            .map_err(|_| DenError::from(EmbeddingFailure::InvalidCredential))?;
        header.set_sensitive(true);
        Ok(Self(header))
    }

    pub(super) fn header(&self) -> HeaderValue {
        self.0.clone()
    }
}

impl fmt::Debug for BearEmbeddingCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearEmbeddingCredential([REDACTED])")
    }
}
