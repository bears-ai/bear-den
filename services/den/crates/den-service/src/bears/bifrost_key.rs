//! Bear-scoped Bifrost virtual-key lifecycle.
//!
//! Bifrost owns the credential and gateway policy. Den owns the encrypted Bear ↔ key
//! mapping and validates that mapping before declaring a Bear ready for inference.

use den_core::{config::Config, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use super::db as bears_db;
use crate::bifrost_governance::BifrostGovernanceClient;

/// Ensure a Bear has a usable Bifrost virtual key without rotating a healthy key.
///
/// Returns `true` when a replacement key was provisioned.
pub async fn ensure_bifrost_virtual_key_for_bear(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    bear_slug: &str,
) -> Result<bool, DenError> {
    let client = BifrostGovernanceClient::new(config);
    if let Some(existing) = bears_db::get_bear_bifrost_virtual_key(pool, bear_id).await? {
        let stored_value = bears_db::bifrost_virtual_key_value_for_bear(
            pool,
            bear_id,
            &config.den_secret_encryption_key,
        )
        .await?;
        let key_exists = match existing
            .virtual_key_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(id) => client.get_virtual_key_details_by_id(id).await?.is_some(),
            None => false,
        };

        if key_exists {
            let value = stored_value.ok_or_else(|| {
                DenError::System(format!(
                    "Bear {bear_id} has Bifrost virtual-key metadata but no stored credential"
                ))
            })?;
            client.validate_virtual_key_value(&value).await?;
            return Ok(false);
        }

        tracing::warn!(
            %bear_id,
            virtual_key_id = existing.virtual_key_id.as_deref().unwrap_or(""),
            "stored Bear Bifrost virtual key no longer exists; provisioning a replacement"
        );
    }

    provision_bifrost_virtual_key_for_bear(pool, config, bear_id, bear_slug).await?;
    Ok(true)
}

/// Provision a fresh Bear-scoped Bifrost virtual key and persist it after validation.
///
/// Returns `true` when an existing Bifrost key was archived or Bifrost reports that
/// usage tracking was reset as part of conflict recovery.
pub async fn provision_bifrost_virtual_key_for_bear(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    bear_slug: &str,
) -> Result<bool, DenError> {
    let client = BifrostGovernanceClient::new(config);
    let archived_existing_key = match bears_db::get_bear_bifrost_virtual_key(pool, bear_id).await? {
        Some(existing) => {
            if let Some(existing_id) = existing
                .virtual_key_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                match client.archive_virtual_key_by_id(existing_id).await? {
                    Some(archived_name) => {
                        tracing::warn!(
                            %bear_id,
                            %existing_id,
                            archived_name,
                            "archived existing Bifrost virtual key before reprovisioning Bear key"
                        );
                        true
                    }
                    None => {
                        tracing::warn!(
                            %bear_id,
                            %existing_id,
                            "stored Bifrost virtual key id was not found while reprovisioning; continuing with replacement"
                        );
                        false
                    }
                }
            } else {
                false
            }
        }
        None => false,
    };

    let key = client.create_bear_virtual_key(bear_id, bear_slug).await?;
    let created_validation = client.validate_virtual_key_value(&key.value).await?;
    tracing::info!(
        %bear_id,
        virtual_key_id = %key.id,
        auth_mode = created_validation.auth_mode.as_str(),
        "validated newly created Bifrost virtual key before storing it in Den"
    );
    bears_db::set_bear_bifrost_virtual_key(
        pool,
        bear_id,
        Some(&key.id),
        Some(&key.name),
        Some(&key.value),
        &config.den_secret_encryption_key,
    )
    .await?;

    let stored_value = bears_db::bifrost_virtual_key_value_for_bear(
        pool,
        bear_id,
        &config.den_secret_encryption_key,
    )
    .await?
    .ok_or_else(|| {
        DenError::System(
            "Bifrost virtual key was saved but could not be read back from Den storage".to_string(),
        )
    })?;
    let validation = client.validate_virtual_key_value(&stored_value).await?;
    tracing::info!(
        %bear_id,
        virtual_key_id = %key.id,
        auth_mode = validation.auth_mode.as_str(),
        "validated provisioned Bifrost virtual key after encrypted Den storage round trip"
    );

    Ok(archived_existing_key || key.reset_usage_tracking)
}
