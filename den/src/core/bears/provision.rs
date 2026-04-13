//! Create/update Letta agents after Den bear rows exist.

use sqlx::PgPool;
use uuid::Uuid;

use crate::core::letta::LettaClient;

use super::db as bears_db;
use crate::errors::CustomError;

/// When Letta is configured, create a Letta agent and store `letta_agent_id`. No-op if Letta disabled.
pub async fn provision_bear_if_configured(
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
        .filter(|s| !s.is_empty());
    let tool_ids: &[String] = &bear.letta_tool_ids.0;

    let agent_id = letta
        .create_agent(
            bear.name.as_str(),
            bear.system_prompt.as_str(),
            Some(model),
            agent_type,
            tool_ids,
        )
        .await?;

    bears_db::set_letta_agent_id(pool, bear_id, &agent_id).await?;
    tracing::info!(%bear_id, %agent_id, "Letta agent provisioned for bear");
    Ok(())
}
