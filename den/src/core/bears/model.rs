//! Row shapes aligned with `migrations/*_phase1_*.sql`.
//! Use with `sqlx::query_as` when adding queries (keeps `SQLX_OFFLINE` builds predictable).

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Bear {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub letta_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    /// Letta `agent_type` on create/patch when set (e.g. `memgpt_agent`, `letta_v1_agent`).
    pub letta_agent_type: Option<String>,
    /// Letta `tool_ids` on create/patch (JSON array of strings in Postgres).
    pub letta_tool_ids: Json<Vec<String>>,
    pub system_prompt: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}
