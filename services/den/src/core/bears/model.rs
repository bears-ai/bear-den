//! Row shapes aligned with `migrations/*_phase1_*.sql`.
//! Use with `sqlx::query_as` when adding queries (keeps `SQLX_OFFLINE` builds predictable).

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use std::{fmt, str::FromStr};
use time::OffsetDateTime;
use uuid::Uuid;

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
    pub letta_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    /// Letta `agent_type` on create/patch when set (e.g. `memgpt_agent`, `letta_v1_agent`).
    pub letta_agent_type: Option<String>,
    /// Letta `tool_ids` on create/patch (JSON array of strings in Postgres).
    pub letta_tool_ids: Json<Vec<String>>,
    /// Optional BearRuntimePlan v1 JSON for codepool (memory git remote, seeds; extensible).
    pub runtime_plan: Option<Json<serde_json::Value>>,
    /// Optional path to the Bear's bare MemFS repository.
    #[serde(default)]
    pub memfs_repo_path: Option<String>,
    /// Coarse canonical config version for role provisioning/reconciliation.
    #[serde(default = "default_provisioning_version")]
    pub provisioning_version: i32,
    pub system_prompt: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

fn default_provisioning_version() -> i32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearAgentRole {
    Talk,
    Pair,
    Curate,
    Work,
    Watch,
}

impl BearAgentRole {
    pub const ALL: [Self; 5] = [Self::Talk, Self::Pair, Self::Curate, Self::Work, Self::Watch];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Talk => "talk",
            Self::Pair => "pair",
            Self::Curate => "curate",
            Self::Work => "work",
            Self::Watch => "watch",
        }
    }

    pub fn is_harness_backed(self) -> bool {
        matches!(self, Self::Talk | Self::Work)
    }

    pub fn runtime_family(self) -> &'static str {
        if self.is_harness_backed() {
            "letta_code_harness"
        } else {
            "letta_api_direct"
        }
    }

    pub fn tags_for_bear(self, bear_id: Uuid) -> Vec<String> {
        vec![
            format!("bear:{bear_id}"),
            format!("role:{}", self.as_str()),
            format!("bear:{bear_id}:role:{}", self.as_str()),
            "git-memory-enabled".to_string(),
        ]
    }
}

impl fmt::Display for BearAgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BearAgentRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "talk" => Ok(Self::Talk),
            "pair" => Ok(Self::Pair),
            "curate" => Ok(Self::Curate),
            "work" => Ok(Self::Work),
            "watch" => Ok(Self::Watch),
            other => Err(format!("unknown bear agent role: {other}")),
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearAgent {
    pub bear_id: Uuid,
    pub role: String,
    pub letta_agent_id: Option<String>,
    pub provisioning_status: String,
    pub last_provisioned_version: i32,
    pub last_synced_at: Option<OffsetDateTime>,
    pub last_provisioning_error: Option<String>,
    pub config_hash: Option<Json<serde_json::Value>>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl BearAgent {
    pub fn parsed_role(&self) -> Result<BearAgentRole, String> {
        self.role.parse()
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearSkillManifestEntry {
    pub bear_id: Uuid,
    pub skill_name: String,
    pub skill_version: String,
    pub source: String,
    pub content_hash: String,
    pub applies_to_roles: Vec<String>,
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
mod tests {
    use super::*;

    #[test]
    fn role_parse_and_display_round_trip() {
        for role in BearAgentRole::ALL {
            let parsed: BearAgentRole = role.as_str().parse().expect("role parses");
            assert_eq!(parsed, role);
            assert_eq!(role.to_string(), role.as_str());
        }
        assert!("unknown".parse::<BearAgentRole>().is_err());
    }

    #[test]
    fn role_runtime_family_matches_harness_design() {
        assert!(BearAgentRole::Talk.is_harness_backed());
        assert!(BearAgentRole::Work.is_harness_backed());
        assert!(!BearAgentRole::Pair.is_harness_backed());
        assert!(!BearAgentRole::Curate.is_harness_backed());
        assert!(!BearAgentRole::Watch.is_harness_backed());
        assert_eq!(BearAgentRole::Talk.runtime_family(), "letta_code_harness");
        assert_eq!(BearAgentRole::Work.runtime_family(), "letta_code_harness");
        assert_eq!(BearAgentRole::Pair.runtime_family(), "letta_api_direct");
    }

    #[test]
    fn role_tags_include_bear_role_and_git_memory() {
        let bear_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
        let tags = BearAgentRole::Work.tags_for_bear(bear_id);
        assert!(tags.contains(&format!("bear:{bear_id}")));
        assert!(tags.contains(&"role:work".to_string()));
        assert!(tags.contains(&format!("bear:{bear_id}:role:work")));
        assert!(tags.contains(&"git-memory-enabled".to_string()));
    }
}
