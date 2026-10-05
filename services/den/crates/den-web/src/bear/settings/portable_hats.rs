//! Portable hat identity and grant intent. Import never restores execution authority.

use std::collections::{BTreeSet, HashMap};

use den_core::ids::{BearId, HatId, UserId};
use den_service::bears::hats;
use serde::{Deserialize, Serialize};

use crate::{errors::CustomError, AppState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortableHat {
    pub original_id: HatId,
    pub name: String,
    pub purpose: String,
    pub short_summary: Option<String>,
    pub identity_prompt: String,
    pub work_requested: bool,
    pub automatic_sharing_requested: bool,
    #[serde(default)]
    pub repository_names: Vec<String>,
    #[serde(default)]
    pub https_hosts: Vec<String>,
    #[serde(default)]
    pub web_fetch_requested: bool,
    #[serde(default)]
    pub web_search_requested: bool,
}

pub(super) fn validate(hats: &[PortableHat], default: Option<HatId>) -> Result<(), CustomError> {
    if hats.len() > 100 {
        return Err(CustomError::ValidationError(
            "a bundle may contain at most 100 hats".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for hat in hats {
        if !ids.insert(hat.original_id.to_string()) || !names.insert(hat.name.trim().to_lowercase())
        {
            return Err(CustomError::ValidationError(
                "duplicate hat identity or name in bundle".into(),
            ));
        }
        if hat.name.trim().is_empty()
            || hat.name.chars().count() > 120
            || hat.purpose.trim().is_empty()
            || hat.purpose.len() > 4000
            || hat.identity_prompt.trim().is_empty()
            || hat.identity_prompt.chars().count() > 4000
            || hat.short_summary.as_ref().is_some_and(|text| {
                text.trim().is_empty()
                    || text.chars().count() > 160
                    || text.chars().any(char::is_control)
            })
            || hat.repository_names.len() > 100
            || hat.https_hosts.len() > 100
        {
            return Err(CustomError::ValidationError(
                "invalid or oversized portable hat".into(),
            ));
        }
        for host in &hat.https_hosts {
            hats::access::HttpsHost::parse(host)?;
        }
    }
    if default.is_some_and(|id| !hats.iter().any(|hat| hat.original_id == id)) {
        return Err(CustomError::ValidationError(
            "IDE default must name a hat in this bundle".into(),
        ));
    }
    Ok(())
}

pub(super) async fn export(
    state: &AppState,
    bear_id: BearId,
) -> Result<Vec<PortableHat>, CustomError> {
    let repositories = den_service::work_surfaces::list_surfaces_for_bears(
        state.sqlx_pool(),
        &[bear_id.as_uuid()],
    )
    .await?;
    let mut result = Vec::new();
    for hat in hats::list_hats(state.sqlx_pool(), bear_id).await? {
        let surfaces = hats::manage::allowed_surfaces(state.sqlx_pool(), bear_id, hat.id).await?;
        let grants = hats::access::web_grants_for_hat(state.sqlx_pool(), bear_id, hat.id).await?;
        result.push(PortableHat {
            original_id: hat.id,
            name: hat.name,
            purpose: hat.purpose,
            short_summary: hat.short_summary,
            identity_prompt: hat.identity_prompt,
            work_requested: hat.work_enabled,
            automatic_sharing_requested: hat.auto_curate_enabled,
            repository_names: repositories
                .iter()
                .filter(|repository| surfaces.contains(&repository.id))
                .map(|repository| repository.name.clone())
                .collect(),
            https_hosts: grants.hosts.into_iter().map(|grant| grant.host).collect(),
            web_fetch_requested: grants.fetch_tool_grant_id.is_some(),
            web_search_requested: grants.search_tool_grant_id.is_some(),
        });
    }
    Ok(result)
}

pub(super) async fn import(
    state: &AppState,
    bear_id: BearId,
    actor: UserId,
    portable: &[PortableHat],
    default: Option<HatId>,
) -> Result<HashMap<HatId, HatId>, CustomError> {
    validate(portable, default)?;
    let mut mapping = HashMap::new();
    for item in portable {
        let hat = hats::create_hat_with_summary(
            state.sqlx_pool(),
            bear_id,
            actor,
            &item.name,
            &item.purpose,
            item.short_summary.as_deref(),
        )
        .await?;
        hats::manage::update_hat(
            state.sqlx_pool(),
            bear_id,
            hat.id,
            &item.name,
            &item.purpose,
            &item.identity_prompt,
            false,
        )
        .await?;
        mapping.insert(item.original_id, hat.id);
    }
    if let Some(default) = default {
        let imported = mapping
            .get(&default)
            .ok_or_else(|| CustomError::ValidationError("IDE default is not imported".into()))?;
        hats::set_ide_default_hat(state.sqlx_pool(), bear_id, *imported).await?;
    }
    Ok(mapping)
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ReconnectionIntent {
    pub imported_hat_id: HatId,
    pub intent: PortableHat,
}

pub(crate) async fn receipt(
    state: &AppState,
    bear_id: BearId,
) -> Result<Vec<ReconnectionIntent>, CustomError> {
    let row = sqlx::query_scalar!(r#"SELECT hat_intent AS "hat_intent: sqlx::types::Json<Vec<ReconnectionIntent>>" FROM bear_import_receipts WHERE bear_id=$1"#, bear_id.as_uuid()).fetch_optional(state.sqlx_pool()).await?;
    Ok(row.map(|value| value.0).unwrap_or_default())
}

#[cfg(test)]
mod tests;
