use super::{
    catalog_state::{CredentialFingerprint, RefreshFailure, RefreshPermit},
    BifrostCatalogEntry, BifrostCatalogSnapshot, BifrostClient, BEAR_CATALOG_REFRESH_RETRY_DELAYS,
};
use crate::bears::model_configurations::PrimaryModelSource;
use den_core::{
    DenError, ModelAvailabilityFailure, ModelAvailabilityFailureKind, SafeModelReference,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(super) fn catalog_failure(kind: ModelAvailabilityFailureKind) -> DenError {
    DenError::ModelAvailability(ModelAvailabilityFailure::new(kind, None))
}

fn for_model(error: DenError, model: &str) -> DenError {
    match error {
        DenError::ModelAvailability(failure) => {
            DenError::ModelAvailability(failure.with_model(model))
        }
        other => other,
    }
}

async fn read_virtual_key(
    pool: &PgPool,
    bear_id: Uuid,
    secret_key: &str,
) -> Result<String, DenError> {
    crate::bears::db::bifrost_virtual_key_for_inference(pool, bear_id, secret_key)
        .await
        .map_err(|error| match error {
            DenError::Database(_) | DenError::DatabaseUnavailable(_) => error,
            _ => catalog_failure(ModelAvailabilityFailureKind::VirtualKeyRejected),
        })?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| catalog_failure(ModelAvailabilityFailureKind::VirtualKeyMissing))
}

impl BifrostClient {
    /// Selection writes require a fresh authenticated catalog, without continuity.
    pub async fn validate_bear_model_selection(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        model: &str,
        secret_key: &str,
    ) -> Result<BifrostCatalogEntry, DenError> {
        let snapshot = self
            .refresh_bear_catalog_snapshot(pool, bear_id, secret_key)
            .await
            .map_err(|error| for_model(error, model))?;
        snapshot.require_available_model(model).cloned()
    }

    /// Preflight a canonical resolved primary for a new turn. Only an explicit
    /// conversation pin may continue through a genuine catalog outage. `None`
    /// retains that same pin without claiming verified catalog availability.
    pub async fn validate_bear_model_execution(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        model: &str,
        secret_key: &str,
        source: PrimaryModelSource,
    ) -> Result<Option<BifrostCatalogEntry>, DenError> {
        match self
            .read_bear_catalog(pool, bear_id, secret_key, false)
            .await
        {
            Ok(snapshot) => snapshot.require_available_model(model).cloned().map(Some),
            Err(RefreshFailure::Outage(cached))
                if source == PrimaryModelSource::ConversationPin =>
            {
                // This cache was captured atomically by the current refresh after
                // rechecking its canonical credential, not read after an arbitrary error.
                let entry = cached
                    .as_ref()
                    .map(|snapshot| snapshot.require_available_model(model).cloned())
                    .transpose()?;
                tracing::warn!(
                    bear_id = %bear_id, model = ?SafeModelReference::checked(model),
                    reason = ModelAvailabilityFailureKind::CatalogUnavailable.descriptor().code,
                    cached_transport = entry.is_some(),
                    "Bifrost catalog unavailable; retaining the canonical explicit pin"
                );
                Ok(entry)
            }
            Err(failure) => Err(for_model(failure.into_den(), model)),
        }
    }

    async fn credential_for_refresh(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        secret_key: &str,
        permit: RefreshPermit,
    ) -> Result<String, RefreshFailure> {
        self.require_current_refresh(permit)?;
        let result = read_virtual_key(pool, bear_id, secret_key).await;
        self.require_current_refresh(permit)?;
        match result {
            Ok(key) => Ok(key),
            Err(error) => {
                self.invalidate_catalog(permit)?;
                Err(RefreshFailure::Failed(error))
            }
        }
    }

    async fn recheck_catalog_credential(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        secret_key: &str,
        permit: RefreshPermit,
        credential: CredentialFingerprint,
    ) -> Result<(), RefreshFailure> {
        let current = self
            .credential_for_refresh(pool, bear_id, secret_key, permit)
            .await?;
        if CredentialFingerprint::for_key(&current) != credential {
            self.invalidate_catalog(permit)?;
            return Err(RefreshFailure::Failed(catalog_failure(
                ModelAvailabilityFailureKind::VirtualKeyRejected,
            )));
        }
        Ok(())
    }

    async fn read_bear_catalog(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        secret_key: &str,
        allow_cached: bool,
    ) -> Result<BifrostCatalogSnapshot, RefreshFailure> {
        // Do not turn legitimate concurrent checks into artificial outages.
        // Each queued call still reads its own credential and performs a fresh
        // request when required; different credentials never share a result.
        let (_refresh_guard, queued) = self.enter_catalog_refresh(bear_id).await?;
        let permit = self.begin_catalog_refresh(bear_id)?;
        let virtual_key = self
            .credential_for_refresh(pool, bear_id, secret_key, permit)
            .await?;
        let credential = CredentialFingerprint::for_key(&virtual_key);
        self.bind_catalog_credential(permit, credential)?;
        if allow_cached && !queued {
            if let Some(snapshot) = self.finish_cached_catalog(permit, credential)? {
                return Ok(snapshot);
            }
        }
        let mut attempt = 0usize;
        let outcome = loop {
            self.require_current_refresh(permit)?;
            let outcome = self
                .list_available_models_with_virtual_key(Some(&virtual_key))
                .await;
            self.require_current_refresh(permit)?;
            match outcome {
                Err(DenError::ModelAvailability(failure))
                    if failure.kind == ModelAvailabilityFailureKind::CatalogUnavailable
                        && attempt < BEAR_CATALOG_REFRESH_RETRY_DELAYS.len() =>
                {
                    let delay = BEAR_CATALOG_REFRESH_RETRY_DELAYS[attempt];
                    attempt += 1;
                    tracing::warn!(
                        bear_id = %bear_id, attempt, retry_after_ms = delay.as_millis(),
                        reason = failure.descriptor().code,
                        "Bear-scoped Bifrost catalog unavailable; retrying preflight"
                    );
                    tokio::time::sleep(delay).await;
                    self.recheck_catalog_credential(pool, bear_id, secret_key, permit, credential)
                        .await?;
                }
                other => break other,
            }
        };
        // A key can rotate while HTTP is pending even without another client
        // refresh. Check canonical storage again before publishing or fallback.
        self.recheck_catalog_credential(pool, bear_id, secret_key, permit, credential)
            .await?;
        match outcome {
            Ok(models) => self.publish_catalog(
                permit,
                credential,
                BifrostCatalogSnapshot::from_available_models(models),
            ),
            Err(DenError::ModelAvailability(failure))
                if failure.kind == ModelAvailabilityFailureKind::CatalogUnavailable =>
            {
                Err(self.finish_catalog_outage(permit, credential)?)
            }
            Err(error) => {
                self.invalidate_catalog(permit)?;
                Err(RefreshFailure::Failed(error))
            }
        }
    }

    pub async fn refresh_bear_catalog_snapshot(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        secret_encryption_key: &str,
    ) -> Result<BifrostCatalogSnapshot, DenError> {
        self.read_bear_catalog(pool, bear_id, secret_encryption_key, false)
            .await
            .map_err(RefreshFailure::into_den)
    }

    pub async fn bear_catalog_snapshot(
        &self,
        pool: &PgPool,
        bear_id: Uuid,
        secret_encryption_key: &str,
    ) -> Result<BifrostCatalogSnapshot, DenError> {
        self.read_bear_catalog(pool, bear_id, secret_encryption_key, true)
            .await
            .map_err(RefreshFailure::into_den)
    }
}
