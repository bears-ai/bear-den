//! Create/update Letta agents after Den bear rows exist.

use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{bifrost::BifrostClient, letta::LettaClient};

use super::db as bears_db;
use super::runtime_plan::default_runtime_plan;
use crate::errors::CustomError;

/// When Letta is configured, create a Letta agent and store `letta_agent_id`. No-op if Letta disabled.
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

    if bear.letta_agent_id.is_some() {
        return Ok(());
    }

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

    let tool_ids = letta.filtered_tool_ids(&bear.letta_tool_ids.0).await?;
    let context_window = if bifrost.is_enabled() {
        bifrost.get_model(model).await?.map(|m| m.context_window)
    } else {
        None
    };

    let agent_id = letta
        .create_agent(
            bear.name.as_str(),
            bear.system_prompt.as_str(),
            Some(model),
            context_window,
            Some(agent_type),
            &tool_ids,
        )
        .await?;

    bears_db::set_letta_agent_id(pool, bear_id, &agent_id).await?;
    bears_db::backfill_default_letta_agent_type(pool, bear_id, "letta_v1_agent").await?;
    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan()).await?;
    tracing::info!(%bear_id, %agent_id, "Letta agent provisioned for bear");
    Ok(())
}
