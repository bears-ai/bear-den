use std::fmt;

use den_core::{
    ids::{BearId, ModelConfigurationId},
    DenError, ThinkingEffort,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Den catalog handle, not a provider model id. Membership/capabilities must be
/// checked at use time; deserialization alone does not confer selectability.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelHandle(String);

impl ModelHandle {
    pub(super) fn from_catalog(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ModelHandle {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ModelHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfiguration {
    pub id: ModelConfigurationId,
    pub bear_id: BearId,
    pub name: String,
    pub model_handle: ModelHandle,
    pub thinking_effort: Option<ThinkingEffort>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryModelSource {
    ConversationPin,
    HatOverride,
    BearDefault,
    DeploymentDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPrimaryModel {
    pub configuration_id: Option<ModelConfigurationId>,
    pub configuration_name: Option<String>,
    pub model_handle: String,
    pub thinking_effort: Option<ThinkingEffort>,
    pub source: PrimaryModelSource,
}

pub(super) struct ModelConfigurationRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub name: String,
    pub model_handle: String,
    pub thinking_effort: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl TryFrom<ModelConfigurationRow> for ModelConfiguration {
    type Error = DenError;

    fn try_from(row: ModelConfigurationRow) -> Result<Self, Self::Error> {
        let thinking_effort = row
            .thinking_effort
            .as_deref()
            .map(|raw| match raw {
                "low" => Ok(ThinkingEffort::Low),
                "medium" => Ok(ThinkingEffort::Medium),
                "high" => Ok(ThinkingEffort::High),
                _ => Err(DenError::Parsing("invalid stored thinking effort".into())),
            })
            .transpose()?;
        Ok(Self {
            id: row.id.into(),
            bear_id: row.bear_id.into(),
            name: row.name,
            model_handle: ModelHandle(row.model_handle),
            thinking_effort,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

pub(super) fn write_error(error: sqlx::Error) -> DenError {
    if let sqlx::Error::Database(ref database) = error {
        if database.is_unique_violation() {
            return DenError::ValidationError(
                "a model configuration with that name already exists for this Bear".into(),
            );
        }
        if database.is_foreign_key_violation() {
            return DenError::ValidationError(
                "model configuration must belong to this Bear and cannot be deleted while referenced"
                    .into(),
            );
        }
    }
    error.into()
}
