//! Push Den bear registry fields to an existing Letta agent (`PATCH /v1/agents/{id}`), then
//! `POST /v1/agents/{id}/recompile` so Letta refreshes the compiled system prompt.

use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{
    bears::{db as bears_db, runtime_plan::default_runtime_plan},
    letta::LettaClient,
};
use crate::errors::CustomError;

/// When Letta is configured and the bear has `letta_agent_id`, PATCH Letta to match Den, then
/// recompile the agent. No-op if Letta is disabled or no agent id is stored.
pub async fn sync_bear_to_letta(
    pool: &PgPool,
    letta: &LettaClient,
    bear_id: Uuid,
) -> Result<(), CustomError> {
    if !letta.is_enabled() {
        return Ok(());
    }

    let bear = bears_db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let Some(agent_id) = bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
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

    let tool_ids = letta.filtered_tool_ids(&bear.letta_tool_ids.0).await?;

    letta
        .patch_agent(
            agent_id,
            bear.name.as_str(),
            bear.description.as_str(),
            bear.system_prompt.as_str(),
            model,
            Some(agent_type),
            &tool_ids,
        )
        .await?;

    letta.recompile_agent(agent_id).await?;

    bears_db::backfill_default_letta_agent_type(pool, bear_id, "letta_v1_agent").await?;
    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan()).await?;

    Ok(())
}
