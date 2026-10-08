//! Creation from an acknowledged, immutable bundle; compensate incomplete imports.
use super::import_outcome::{compensate, CreationFailure, CreationStage};
use super::{
    memory_sqlite_path, portable_hats, portable_models, rewrite_imported_memory_bear_id,
    unique_import_slug, BearBundleIdentity, BearBundleManifest, BearBundlePrompts,
};
use crate::{errors::CustomError, web::AppState};
use den_core::ids::{BearId, UserId};
use den_service::bears::{db as bears_db, db::BEAR_ROLE_ADMIN, provision};
use std::{io::Write, os::unix::fs::OpenOptionsExt};

pub(super) async fn create(
    state: &AppState,
    actor: UserId,
    manifest: BearBundleManifest,
    memory_sqlite: Vec<u8>,
) -> Result<String, CreationFailure> {
    let portable_models = manifest.model_configurations.clone();
    let imported_model_default = manifest.default_model_configuration_id;
    let portable_skills = manifest.skills.clone();
    let portable = manifest.hats.clone();
    let imported_default = manifest.ide_default_hat;
    let BearBundleManifest {
        bear:
            BearBundleIdentity {
                slug: imported_slug,
                name,
                description,
                birthdate,
                default_model,
                tools_enabled,
            },
        prompts:
            BearBundlePrompts {
                system_prompt,
                context_profile,
            },
        ..
    } = manifest;
    let slug = unique_import_slug(state.sqlx_pool(), &imported_slug)
        .await
        .map_err(|_| CreationFailure::unconfirmed(CreationStage::Handle))?;

    let bear_id = bears_db::create_bear_with_context_profile(
        state.sqlx_pool(),
        bears_db::BearParams {
            slug: &slug,
            name: &name,
            description: &description,
            system_prompt: &system_prompt,
            default_model: if portable_models.is_none() {
                default_model.as_deref()
            } else {
                None
            },
            tools_enabled: tools_enabled.map(sqlx::types::Json),
            context_profile: context_profile.map(sqlx::types::Json),
        },
    )
    .await
    .map_err(|_| CreationFailure::unconfirmed(CreationStage::Creation))?;

    let mut stage = CreationStage::Birthday;
    let setup: Result<(), CustomError> = async {
    let birthdate = birthdate.trim();
    if !birthdate.is_empty() {
        sqlx::query!(
            "UPDATE bears SET birthday = $1::text::date, updated_at = NOW() WHERE id = $2",
            birthdate,
            bear_id
        )
        .execute(state.sqlx_pool())
        .await
        .map_err(|err| CustomError::ValidationError(format!("invalid Bear birthday: {err}")))?;
    }

    stage = CreationStage::Memory;
    let memory_path = memory_sqlite_path(state.config.as_ref(), bear_id);
    if let Some(parent) = memory_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            CustomError::System(format!("create Bear memory directory failed: {err}"))
        })?;
    }
    let mut memory_file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&memory_path)?;
    memory_file.write_all(&memory_sqlite)?;
    memory_file.sync_all()?;
    drop(memory_file);
    rewrite_imported_memory_bear_id(state, bear_id).await?;
    stage = CreationStage::Hats;
    let mapping = portable_hats::import(
        state,
        BearId::new(bear_id),
        actor,
        &portable,
        imported_default,
    )
    .await?;
    stage = CreationStage::Models;
    if let Some(configurations) = portable_models.as_deref() {
        portable_models::import(state.sqlx_pool(), BearId::new(bear_id), configurations, imported_model_default, &portable, &mapping).await?;
    }
    stage = CreationStage::Receipt;
    let intent = portable.iter().map(|hat| portable_hats::ReconnectionIntent { imported_hat_id: mapping[&hat.original_id], intent: hat.clone() }).collect::<Vec<_>>();
    sqlx::query!("INSERT INTO bear_import_receipts(bear_id,imported_by_user_id,hat_intent) VALUES($1,$2,$3)", bear_id, actor.get(), sqlx::types::Json(&intent) as _).execute(state.sqlx_pool()).await?;
    stage = CreationStage::KnowledgeMapping;
    let store = state.memory_stores.store_for_bear(bear_id).await?;
    let mut tx =
        store.pool().begin().await.map_err(|error| {
            CustomError::System(format!("begin memory import mapping: {error}"))
        })?;
    for (original, imported) in mapping {
        for (table, column) in [
            ("memory_records", "scope_hat_id"),
            ("memory_proposals", "target_hat_id"),
        ] {
            // sqlx-dynamic: import identifiers are drawn exclusively from this fixed schema whitelist.
            sqlx::query(&format!(
                "UPDATE {table} SET {column} = ? WHERE {column} = ? AND bear_id = ?"
            ))
            .bind(imported.to_string())
            .bind(original.to_string())
            .bind(bear_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| CustomError::System(format!("map imported hat knowledge: {error}")))?;
        }
    }
    tx.commit().await.map_err(|error| CustomError::System(format!("commit imported hat knowledge: {error}")))?;
        stage = CreationStage::Entities;
        let entities = den_memory::list_entities(&store, None, 10_001).await?;
        if entities.len() > 10_000 { return Err(CustomError::ValidationError("bundle contains too many entity bindings".into())); }
        for entity in entities {
            den_memory::set_resolution(&store, &entity.entity_id, den_memory::ResolutionState::Provisional, None).await?;
            den_memory::set_canonical_ref(&store, &entity.entity_id, None).await?;
            for handle in den_memory::list_handles(&store, &entity.entity_id).await? { den_memory::detach_handle(&store, &handle.handle_id).await?; }
        }

    stage = CreationStage::Initialization;
    provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, bear_id).await?;

    stage = CreationStage::Membership;
    bears_db::grant_membership(state.sqlx_pool(), actor.get(), bear_id, Some(BEAR_ROLE_ADMIN)).await?;
    stage = CreationStage::Procedures;
    den_service::skills::stage_import(state.sqlx_pool(), BearId::new(bear_id), actor, &portable_skills).await?;
    Ok(())
    }.await;
    if setup.is_err() {
        tracing::warn!(%bear_id, ?stage, "Import setup failed; compensating incomplete Bear");
        let failure = compensate(
            bear_id,
            slug,
            stage,
            bears_db::delete_bear(state.sqlx_pool(), bear_id),
            || async {
                // This closure is never called if the Bear survives deletion.
                if let Ok(store) = state.memory_stores.store_for_bear(bear_id).await {
                    store.pool().close().await;
                }
                let path = memory_sqlite_path(state.config.as_ref(), bear_id);
                let mut failed = false;
                for suffix in ["", "-wal", "-shm"] {
                    if let Err(error) = std::fs::remove_file(format!("{}{suffix}", path.display()))
                    {
                        failed |= error.kind() != std::io::ErrorKind::NotFound;
                    }
                }
                if failed {
                    return Err(CustomError::System(
                        "Private memory cleanup needs operator repair.".into(),
                    ));
                }
                Ok(())
            },
        )
        .await;
        return Err(failure);
    }
    Ok(slug)
}
