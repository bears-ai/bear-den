//! Row shapes aligned with `migrations/*_phase1_*.sql`.
//! Use with `sqlx::query_as` when adding queries (keeps `SQLX_OFFLINE` builds predictable).

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearTemplate {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub default_model: Option<String>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Bear {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub letta_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    pub system_prompt: String,
    pub source_template_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}
