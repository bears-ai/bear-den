//! Create/update Letta agents after Den bear rows exist.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    bifrost::BifrostClient,
    letta::LettaClient,
    memory_manager_head::register_memfs_role_view,
};

use super::db as bears_db;
use super::model::{Bear, BearAgentRole};
use super::runtime_plan::default_runtime_plan;
use crate::errors::CustomError;

fn memfs_sidecar_url_from_env() -> String {
    std::env::var("LETTA_MEMFS_SERVICE_URL")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

async fn register_role_view_if_configured(
    letta: &LettaClient,
    bear_id: Uuid,
    role: BearAgentRole,
    agent_id: &str,
) -> Result<(), CustomError> {
    let base = memfs_sidecar_url_from_env();
    if base.trim().is_empty() {
        return Ok(());
    }
    match register_memfs_role_view(letta.http(), &base, agent_id, bear_id, role.as_str()).await {
        Ok(Some(view)) => {
            tracing::info!(
                %bear_id,
                role = %role,
                %agent_id,
                state = %view.state,
                canonical_tip = view.canonical_tip.as_deref(),
                view_tip = view.view_tip.as_deref(),
                "MemFS role view registered"
            );
        }
        Ok(None) => {
            tracing::debug!(%bear_id, role = %role, %agent_id, "MemFS sidecar not configured; skipped role view registration");
        }
        Err(err) => {
            tracing::warn!(%bear_id, role = %role, %agent_id, error = %err, "MemFS role view registration failed");
            return Err(err);
        }
    }
    Ok(())
}

/// When Letta is configured, create role-specific Letta agents and mirror the talk role to
/// legacy `bears.letta_agent_id`. No-op if Letta disabled.
pub async fn provision_bear_if_configured(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear_id: Uuid,
) -> Result<(), CustomError> {
    if !letta.is_enabled() {
        return Ok(());
    }

    let bear = bears_db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    provision_bear_roles(pool, letta, bifrost, &bear).await?;
    bears_db::backfill_default_letta_agent_type(pool, bear_id, "letta_v1_agent").await?;
    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan()).await?;
    bears_db::mirror_talk_agent_to_legacy_letta_agent_id(pool, bear_id).await?;
    Ok(())
}

async fn provision_bear_roles(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear: &Bear,
) -> Result<(), CustomError> {
    bears_db::ensure_bear_agent_rows(pool, bear.id).await?;

    for role in BearAgentRole::ALL {
        provision_bear_role(pool, letta, bifrost, bear, role).await?;
    }

    Ok(())
}

pub async fn provision_missing_bear_roles(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear_id: Uuid,
) -> Result<usize, CustomError> {
    if !letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL).".to_string(),
        ));
    }

    let bear = bears_db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    bears_db::ensure_bear_agent_rows(pool, bear.id).await?;

    let mut provisioned = 0usize;
    for role in BearAgentRole::ALL {
        let existing = bears_db::get_bear_agent(pool, bear.id, role).await?;
        let has_agent = existing
            .as_ref()
            .and_then(|row| row.letta_agent_id.as_deref())
            .is_some_and(|id| !id.trim().is_empty());
        if has_agent {
            continue;
        }
        provision_bear_role(pool, letta, bifrost, &bear, role).await?;
        provisioned += 1;
    }

    bears_db::mirror_talk_agent_to_legacy_letta_agent_id(pool, bear_id).await?;
    Ok(provisioned)
}

pub async fn reconcile_bear_if_configured(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear_id: Uuid,
) -> Result<crate::core::bears::sync::BearSyncSummary, CustomError> {
    if !letta.is_enabled() {
        return Ok(crate::core::bears::sync::BearSyncSummary {
            bear_id,
            outcomes: BearAgentRole::ALL
                .iter()
                .map(|role| crate::core::bears::sync::BearRoleSyncOutcome {
                    role: role.as_str().to_string(),
                    letta_agent_id: None,
                    status: "skipped_letta_disabled".to_string(),
                    message: Some("Letta is not configured (set LETTA_BASE_URL).".to_string()),
                })
                .collect(),
        });
    }
    crate::core::bears::sync::sync_all_bear_roles_to_letta(pool, letta, bifrost, bear_id).await
}

async fn provision_bear_role(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear: &Bear,
    role: BearAgentRole,
) -> Result<(), CustomError> {
    let existing = bears_db::get_bear_agent(pool, bear.id, role).await?;
    if let Some(existing) = existing.as_ref() {
        if existing
            .letta_agent_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
        {
            let current_hash = role_config_hash(bear, role);
            let stored_hash = existing.config_hash.as_ref().map(|j| j.as_ref());
            if existing.last_provisioned_version >= bear.provisioning_version
                && stored_hash == Some(&current_hash)
            {
                return Ok(());
            }

            // Existing role agents are reconciled via PATCH rather than replaced.
            let summary = crate::core::bears::sync::sync_all_bear_roles_to_letta(
                pool,
                letta,
                bifrost,
                bear.id,
            )
            .await?;
            if let Some(message) = summary.diagnostic_message() {
                return Err(CustomError::System(message));
            }
            return Ok(());
        }
    }

    bears_db::mark_bear_agent_provisioning(pool, bear.id, role).await?;

    let result = create_role_agent(letta, bifrost, bear, role).await;
    match result {
        Ok(agent_id) => {
            let config_hash = role_config_hash(bear, role);
            bears_db::mark_bear_agent_ready(
                pool,
                bear.id,
                role,
                &agent_id,
                bear.provisioning_version,
                &config_hash,
            )
            .await?;
            register_role_view_if_configured(letta, bear.id, role, &agent_id).await?;
            tracing::info!(bear_id = %bear.id, %agent_id, role = %role, "Letta role agent provisioned for bear");
            Ok(())
        }
        Err(err) => {
            let message = err.to_string();
            bears_db::mark_bear_agent_failed(pool, bear.id, role, &message).await?;
            Err(err)
        }
    }
}

async fn create_role_agent(
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear: &Bear,
    role: BearAgentRole,
) -> Result<String, CustomError> {
    let model = bear
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CustomError::ValidationError(
                "default_model is required to provision a Letta agent (pick a model from Letta)."
                    .to_string(),
            )
        })?;

    let agent_type = bear
        .letta_agent_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("letta_v1_agent");

    let desired_tool_ids = desired_role_tool_ids(bear, role);
    let tool_ids = letta.filtered_tool_ids(&desired_tool_ids).await?;
    let context_window = if bifrost.is_enabled() {
        bifrost.get_model(model).await?.map(|m| m.context_window)
    } else {
        None
    };

    let name = format!("{} ({})", bear.name, role.as_str());
    let prompt = render_role_prompt(bear, role);
    let tags = role.tags_for_bear(bear.id);

    letta
        .create_agent_with_tags(
            name.as_str(),
            prompt.as_str(),
            Some(model),
            context_window,
            Some(agent_type),
            &tool_ids,
            &tags,
        )
        .await
}

pub(crate) fn desired_role_tool_ids(bear: &Bear, role: BearAgentRole) -> Vec<String> {
    // Until explicit per-role tool roster configuration lands, be conservative:
    // harness-backed roles keep the operator-selected Letta tools, while API-direct roles receive
    // no broad operator-selected harness tools. Den/ACP tools are exposed through their own
    // controlled paths rather than by attaching every legacy Letta tool to every role.
    match role {
        BearAgentRole::Talk | BearAgentRole::Work => bear.letta_tool_ids.0.clone(),
        BearAgentRole::Pair | BearAgentRole::Curate | BearAgentRole::Watch => Vec::new(),
    }
}

pub(crate) fn render_role_prompt(bear: &Bear, role: BearAgentRole) -> String {
    let mut prompt = bear.system_prompt.trim().to_string();
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }
    prompt.push_str("# BEARS role assignment\n");
    prompt.push_str(&format!(
        "You are the `{}` role agent for the logical Bear `{}`. \
         Preserve the Bear identity while obeying this role boundary.\n",
        role.as_str(), bear.name
    ));
    prompt.push_str(match role {
        BearAgentRole::Talk => {
            "Use conversational channels only. Read core/ and talk/; write only talk/. Use Den tools for task intents and skill proposals."
        }
        BearAgentRole::Pair => {
            "Serve ACP clients. Read core/ and pair/; write only pair/. Client tools are user-gated through ACP. Use Den tools for task intents and skill proposals."
        }
        BearAgentRole::Curate => {
            "Review branches, task intents, observations, results, and skill proposals. Write directly only to curate/ and core/. No external communication tools are allowed."
        }
        BearAgentRole::Work => {
            "Execute approved outbound work through the Letta Code harness. Read only core/, the dispatched task definition, and work/. Write only work/. Obey Den-issued run context, allowed_tools, and scope."
        }
        BearAgentRole::Watch => {
            "Record inbound external observations. Read only core/, delivered event payloads, and watch/. Write only watch/. No outbound action tools are allowed."
        }
    });
    prompt
}

pub(crate) fn role_config_hash(bear: &Bear, role: BearAgentRole) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "role": role.as_str(),
        "runtime_family": role.runtime_family(),
        "bear_provisioning_version": bear.provisioning_version,
        "tool_ids": desired_role_tool_ids(bear, role),
        "prompt_strategy": "base_prompt_plus_role_suffix_v1",
        "skills": {
            "manifest_projection": "pending"
        }
    })
}
