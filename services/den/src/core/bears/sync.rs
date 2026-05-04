//! Push Den bear registry fields to role-specific Letta agents, then recompile them so Letta
//! refreshes compiled system prompts.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    bears::{
        db as bears_db,
        model::{Bear, BearAgent, BearAgentRole},
        provision::{desired_role_tool_ids, render_role_prompt, role_config_hash},
        runtime_plan::default_runtime_plan,
    },
    bifrost::BifrostClient,
    letta::LettaClient,
};
use crate::errors::CustomError;

#[derive(Debug, Clone, Serialize)]
pub struct BearRoleSyncOutcome {
    pub role: String,
    pub letta_agent_id: Option<String>,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BearSyncSummary {
    pub bear_id: Uuid,
    pub outcomes: Vec<BearRoleSyncOutcome>,
}

impl BearSyncSummary {
    pub fn failed_roles(&self) -> Vec<&BearRoleSyncOutcome> {
        self.outcomes
            .iter()
            .filter(|o| o.status == "failed")
            .collect()
    }

    pub fn skipped_roles(&self) -> Vec<&BearRoleSyncOutcome> {
        self.outcomes
            .iter()
            .filter(|o| o.status == "skipped_missing_agent")
            .collect()
    }

    pub fn synced_count(&self) -> usize {
        self.outcomes.iter().filter(|o| o.status == "synced").count()
    }

    pub fn diagnostic_message(&self) -> Option<String> {
        let failed = self.failed_roles();
        if failed.is_empty() {
            return None;
        }
        let parts = failed
            .into_iter()
            .map(|o| {
                format!(
                    "{} ({})",
                    o.role,
                    o.message.as_deref().unwrap_or("unknown error")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!("Letta role sync failed for: {parts}"))
    }
}

fn skipped_missing(role: BearAgentRole) -> BearRoleSyncOutcome {
    BearRoleSyncOutcome {
        role: role.as_str().to_string(),
        letta_agent_id: None,
        status: "skipped_missing_agent".to_string(),
        message: Some("No Letta agent id is recorded for this role.".to_string()),
    }
}

async fn sync_one_role(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear: &Bear,
    agent: &BearAgent,
    role: BearAgentRole,
) -> BearRoleSyncOutcome {
    let Some(agent_id) = agent
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return skipped_missing(role);
    };

    let model = bear
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let agent_type = bear
        .letta_agent_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("letta_v1_agent");

    let desired_tool_ids = desired_role_tool_ids(bear, role);
    let tool_ids = match letta.filtered_tool_ids(&desired_tool_ids).await {
        Ok(ids) => ids,
        Err(err) => {
            let msg = format!("tool roster resolution failed before patching Letta: {err}");
            let _ = bears_db::mark_bear_agent_drifted(pool, bear.id, role, &msg).await;
            tracing::warn!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, error = %err, "role sync failed resolving tool roster");
            return BearRoleSyncOutcome {
                role: role.as_str().to_string(),
                letta_agent_id: Some(agent_id),
                status: "failed".to_string(),
                message: Some(msg),
            };
        }
    };

    let context_window = if let (Some(model), true) = (model, bifrost.is_enabled()) {
        match bifrost.get_model(model).await {
            Ok(model) => model.map(|m| m.context_window),
            Err(err) => {
                let msg = format!("model metadata lookup failed before patching Letta: {err}");
                let _ = bears_db::mark_bear_agent_drifted(pool, bear.id, role, &msg).await;
                tracing::warn!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, error = %err, "role sync failed resolving model metadata");
                return BearRoleSyncOutcome {
                    role: role.as_str().to_string(),
                    letta_agent_id: Some(agent_id),
                    status: "failed".to_string(),
                    message: Some(msg),
                };
            }
        }
    } else {
        None
    };

    let prompt = render_role_prompt(bear, role);
    let role_name = format!("{} ({})", bear.name, role.as_str());

    if let Err(err) = letta
        .patch_agent(
            &agent_id,
            role_name.as_str(),
            bear.description.as_str(),
            prompt.as_str(),
            model,
            context_window,
            Some(agent_type),
            &tool_ids,
        )
        .await
    {
        let msg = format!("Letta PATCH failed: {err}");
        let _ = bears_db::mark_bear_agent_drifted(pool, bear.id, role, &msg).await;
        tracing::warn!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, error = %err, "role sync patch failed");
        return BearRoleSyncOutcome {
            role: role.as_str().to_string(),
            letta_agent_id: Some(agent_id),
            status: "failed".to_string(),
            message: Some(msg),
        };
    }

    if let Err(err) = letta.recompile_agent(&agent_id).await {
        let msg = format!("Letta recompile failed after PATCH: {err}");
        let _ = bears_db::mark_bear_agent_drifted(pool, bear.id, role, &msg).await;
        tracing::warn!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, error = %err, "role sync recompile failed");
        return BearRoleSyncOutcome {
            role: role.as_str().to_string(),
            letta_agent_id: Some(agent_id),
            status: "failed".to_string(),
            message: Some(msg),
        };
    }

    let config_hash = role_config_hash(bear, role);
    if let Err(err) = bears_db::mark_bear_agent_synced(
        pool,
        bear.id,
        role,
        bear.provisioning_version,
        &config_hash,
    )
    .await
    {
        let msg = format!("Den updated Letta but failed to record role sync status: {err}");
        tracing::warn!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, error = %err, "role sync status update failed");
        return BearRoleSyncOutcome {
            role: role.as_str().to_string(),
            letta_agent_id: Some(agent_id),
            status: "failed".to_string(),
            message: Some(msg),
        };
    }

    tracing::info!(bear_id = %bear.id, role = %role, letta_agent_id = %agent_id, "role sync completed");
    BearRoleSyncOutcome {
        role: role.as_str().to_string(),
        letta_agent_id: Some(agent_id),
        status: "synced".to_string(),
        message: None,
    }
}

/// Sync all provisioned Bear role agents to Letta. Missing role ids are skipped and reported; a
/// failure for one role does not prevent other roles from syncing.
pub async fn sync_all_bear_roles_to_letta(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear_id: Uuid,
) -> Result<BearSyncSummary, CustomError> {
    let bear = bears_db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    if !letta.is_enabled() {
        return Ok(BearSyncSummary {
            bear_id,
            outcomes: BearAgentRole::ALL
                .iter()
                .map(|role| BearRoleSyncOutcome {
                    role: role.as_str().to_string(),
                    letta_agent_id: None,
                    status: "skipped_letta_disabled".to_string(),
                    message: Some("Letta is not configured (set LETTA_BASE_URL).".to_string()),
                })
                .collect(),
        });
    }

    bears_db::ensure_bear_agent_rows(pool, bear_id).await?;
    let agents = bears_db::list_bear_agents(pool, bear_id).await?;
    let mut outcomes = Vec::with_capacity(agents.len());
    for agent in agents {
        let role = match agent.parsed_role() {
            Ok(role) => role,
            Err(err) => {
                outcomes.push(BearRoleSyncOutcome {
                    role: agent.role.clone(),
                    letta_agent_id: agent.letta_agent_id.clone(),
                    status: "failed".to_string(),
                    message: Some(format!("invalid role in DB: {err}")),
                });
                continue;
            }
        };
        outcomes.push(sync_one_role(pool, letta, bifrost, &bear, &agent, role).await);
    }

    bears_db::backfill_default_letta_agent_type(pool, bear_id, "letta_v1_agent").await?;
    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan()).await?;
    bears_db::mirror_talk_agent_to_legacy_letta_agent_id(pool, bear_id).await?;

    Ok(BearSyncSummary { bear_id, outcomes })
}

/// Backward-compatible wrapper used by older call sites. Prefer [`sync_all_bear_roles_to_letta`]
/// for new code so per-role failures can be surfaced precisely.
pub async fn sync_bear_to_letta(
    pool: &PgPool,
    letta: &LettaClient,
    bifrost: &BifrostClient,
    bear_id: Uuid,
) -> Result<(), CustomError> {
    let summary = sync_all_bear_roles_to_letta(pool, letta, bifrost, bear_id).await?;
    if let Some(message) = summary.diagnostic_message() {
        return Err(CustomError::System(message));
    }
    Ok(())
}
