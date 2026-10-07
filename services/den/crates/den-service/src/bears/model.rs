//! Row shapes aligned with `migrations/*_phase1_*.sql`.
//! Use with `sqlx::query_as` when adding queries (keeps `SQLX_OFFLINE` builds predictable).

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::DenError;

/// Bear plus `user_bear.role` for the current user (`list_bears_for_user`).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearWithMembership {
    #[sqlx(flatten)]
    pub bear: Bear,
    pub membership_role: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Bear {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// Database projection of the selected named configuration. Legacy SQL
    /// writes are routing inputs, not independent model state.
    /// Use `model_configurations::resolve_primary` for executable model/effort.
    pub default_model: Option<String>,
    pub default_tool_budget_multiplier: Option<f64>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    pub work_enabled: bool,
    /// Enables Cabinet shared-knowledge tools and facade access for this Bear.
    #[serde(default = "default_cabinet_enabled")]
    pub cabinet_enabled: bool,
    /// Optional runtime plan JSON.
    pub runtime_plan: Option<Json<serde_json::Value>>,
    /// Optional profile-aware context composition profile.
    pub context_profile: Option<Json<serde_json::Value>>,
    /// Coarse canonical config version for profile provisioning/reconciliation.
    #[serde(default = "default_provisioning_version")]
    pub provisioning_version: i32,
    pub system_prompt: String,
    pub birthday: Option<time::Date>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    #[serde(default = "default_live_reflection_enabled")]
    pub live_reflection_enabled: bool,
    #[serde(default = "default_live_reflection_stale_after_minutes")]
    pub live_reflection_stale_after_minutes: i32,
    #[serde(default = "default_live_reflection_activity_threshold")]
    pub live_reflection_activity_threshold: i32,
    #[serde(default = "default_live_reflection_sweep_limit")]
    pub live_reflection_sweep_limit: i32,
}

fn default_live_reflection_enabled() -> bool {
    true
}

fn default_live_reflection_stale_after_minutes() -> i32 {
    30
}

fn default_live_reflection_activity_threshold() -> i32 {
    20
}

fn default_live_reflection_sweep_limit() -> i32 {
    25
}

fn default_provisioning_version() -> i32 {
    1
}

fn default_cabinet_enabled() -> bool {
    true
}

// Shared origin-derived metadata; historical profile records retain their schema.
pub use den_core::RuntimeContextLabel;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearProfileBinding {
    pub bear_id: Uuid,
    pub profile: String,
    pub binding_id: String,
    pub provisioning_status: String,
    pub last_provisioned_version: i32,
    pub last_synced_at: Option<OffsetDateTime>,
    pub last_provisioning_error: Option<String>,
    pub config_hash: Option<Json<serde_json::Value>>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl BearProfileBinding {
    pub fn parsed_profile(&self) -> Result<RuntimeContextLabel, DenError> {
        self.profile.parse().map_err(DenError::ValidationError)
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearSkillManifestEntry {
    pub bear_id: Uuid,
    pub skill_name: String,
    pub skill_version: String,
    pub source: String,
    pub content_hash: String,
    pub applies_to_profiles: Vec<String>,
    pub installed_at: Option<OffsetDateTime>,
    pub last_verified_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearSkillProposal {
    pub bear_id: Uuid,
    pub id: Uuid,
    pub proposed_by_agent_id: String,
    pub proposed_at: OffsetDateTime,
    pub skill_payload: Json<serde_json::Value>,
    pub status: String,
    pub reviewed_at: Option<OffsetDateTime>,
    pub rejection_reason: Option<String>,
    pub resulting_manifest_bear_id: Option<Uuid>,
    pub resulting_manifest_skill_name: Option<String>,
    pub resulting_manifest_skill_version: Option<String>,
    pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests;
