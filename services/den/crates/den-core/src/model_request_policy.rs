use crate::ThinkingEffort;
use serde::{Deserialize, Serialize};

/// Symbolic classification for a foreground agent-loop model call.
/// Independent of provider model identifiers and routing authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPrimaryStep {
    OrdinaryTurn,
    Planning,
    TaskSelection,
    Execution,
    Checkpoint,
    PreRiskReview,
    Summarization,
    CheapProbe,
}

impl AgentPrimaryStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OrdinaryTurn => "ordinary_turn",
            Self::Planning => "planning",
            Self::TaskSelection => "task_selection",
            Self::Execution => "execution",
            Self::Checkpoint => "checkpoint",
            Self::PreRiskReview => "pre_risk_review",
            Self::Summarization => "summarization",
            Self::CheapProbe => "cheap_probe",
        }
    }
}

/// Provider-neutral request settings. Execution validates explicit configuration
/// effort against the live catalog; transports report any incompatible omission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequestProfile {
    /// Canonical Den model handle, not a provider id or routing authority grant.
    pub approved_model_ref: String,
    pub agent_primary_step: AgentPrimaryStep,
    /// Catalog-authoritative support; unknown is not support.
    pub supports_reasoning_effort: Option<bool>,
    pub thinking_effort: Option<ThinkingEffort>,
}

impl Default for ModelRequestProfile {
    fn default() -> Self {
        Self {
            approved_model_ref: String::new(),
            agent_primary_step: AgentPrimaryStep::OrdinaryTurn,
            supports_reasoning_effort: None,
            thinking_effort: None,
        }
    }
}

pub fn resolve_agent_primary_request_profile(
    approved_model_ref: impl Into<String>,
    agent_primary_step: AgentPrimaryStep,
    supports_reasoning_effort: Option<bool>,
    checkpoint_thinking_effort: Option<ThinkingEffort>,
) -> ModelRequestProfile {
    resolve_agent_primary_request_profile_with_configuration(
        approved_model_ref,
        agent_primary_step,
        supports_reasoning_effort,
        None,
        checkpoint_thinking_effort,
    )
}

/// Explicit configuration effort applies to every primary step and wins over
/// checkpoint policy. `None` means model default, retaining legacy checkpoints.
/// Execution must validate explicit effort against the live catalog before use.
pub fn resolve_agent_primary_request_profile_with_configuration(
    approved_model_ref: impl Into<String>,
    agent_primary_step: AgentPrimaryStep,
    supports_reasoning_effort: Option<bool>,
    configuration_thinking_effort: Option<ThinkingEffort>,
    checkpoint_thinking_effort: Option<ThinkingEffort>,
) -> ModelRequestProfile {
    let checkpoint_effort = checkpoint_thinking_effort.filter(|_| {
        matches!(
            agent_primary_step,
            AgentPrimaryStep::Checkpoint | AgentPrimaryStep::PreRiskReview
        )
    });
    ModelRequestProfile {
        approved_model_ref: approved_model_ref.into(),
        agent_primary_step,
        supports_reasoning_effort,
        thinking_effort: (supports_reasoning_effort == Some(true))
            .then_some(configuration_thinking_effort.or(checkpoint_effort))
            .flatten(),
    }
}

#[cfg(test)]
#[path = "model_request_policy_tests.rs"]
mod tests;
