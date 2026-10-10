//! Server-side, operation-local authentication for every Bear-scoped embedding path.

use den_core::{config::Config, ids::BearId, DenError};
use den_llm::{BearEmbeddingCredential, EmbeddingClient};
use sqlx::PgPool;

#[cfg(test)]
mod tests;

/// Resolve from canonical Bear state each time; never borrow a global/admin key or cache it.
/// `None` means embeddings are unconfigured. Missing/invalid credentials fail before HTTP.
pub async fn authenticated_embedder(
    pool: &PgPool,
    config: &Config,
    bear_id: BearId,
) -> Result<Option<EmbeddingClient>, DenError> {
    if config.llm_api_url.trim().is_empty() {
        return Ok(None);
    }
    let secret = crate::bears::db::bifrost_virtual_key_for_inference(
        pool,
        bear_id.as_uuid(),
        &config.den_secret_encryption_key,
    )
    .await
    // Database/decryption errors are not suitable for recall logs or persisted run errors.
    .map_err(|_| DenError::System("embeddings Bear credential lookup failed".into()))?
    .ok_or_else(|| DenError::System("embeddings Bear credential is missing".into()))?;
    let credential = BearEmbeddingCredential::from_server_secret(secret)?;
    Ok(Some(EmbeddingClient::new(config, credential)))
}
