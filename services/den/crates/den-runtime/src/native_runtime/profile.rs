//! Native execution defaults selected by a verified ordinary turn origin.

use crate::agent_loop::{
    PostMutationVerificationWindow, StrategyProfile, ToolCallBudgetLimits, TurnBudgetPolicy,
};
use den_core::{DenError, EffectivePolicy, Governance, RuntimeContextLabel, TurnExecutionOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTurnDefaults {
    /// Origin-derived audit label, never an authority input.
    pub context_label: RuntimeContextLabel,
    pub turn_budget: TurnBudgetPolicy,
    pub include_prompt_memory: bool,
    pub strategy: StrategyProfile,
}

impl NativeTurnDefaults {
    pub fn with_tool_budget_multiplier(mut self, multiplier: f64) -> Self {
        self.turn_budget = scaled_turn_budget(self.turn_budget, multiplier);
        self
    }

    pub fn for_origin(
        origin: TurnExecutionOrigin,
        governance: Governance,
    ) -> Result<Self, DenError> {
        origin.require_ordinary_session()?;
        let turn_budget = match origin {
            TurnExecutionOrigin::BrowserTaskSession
            | TurnExecutionOrigin::ArmatureConversation(_) => TurnBudgetPolicy {
                max_wall_clock_ms: 360_000,
                emergency_hard_steps: 80,
                tool_call_limits: ToolCallBudgetLimits {
                    total: 112,
                    read: 32,
                    search: 20,
                    fetch: 12,
                    execute: 16,
                    write: 24,
                    destructive: 2,
                    other: 16,
                },
                max_consecutive_tool_failures: 3,
                max_same_tool_signature_repeats: 2,
                post_mutation_verification_window: Some(PostMutationVerificationWindow {
                    replenish_read: 4,
                    replenish_search: 2,
                }),
            },
            TurnExecutionOrigin::AuthorizedWorkRun(_) => TurnBudgetPolicy {
                max_wall_clock_ms: 900_000,
                emergency_hard_steps: 128,
                tool_call_limits: ToolCallBudgetLimits {
                    total: 160,
                    read: 48,
                    search: 32,
                    fetch: 20,
                    execute: 24,
                    write: 24,
                    destructive: 6,
                    other: 24,
                },
                max_consecutive_tool_failures: 4,
                max_same_tool_signature_repeats: 2,
                post_mutation_verification_window: Some(PostMutationVerificationWindow {
                    replenish_read: 8,
                    replenish_search: 4,
                }),
            },
            TurnExecutionOrigin::ChannelConversation => TurnBudgetPolicy {
                max_wall_clock_ms: 180_000,
                emergency_hard_steps: 40,
                tool_call_limits: ToolCallBudgetLimits {
                    total: 72,
                    read: 20,
                    search: 12,
                    fetch: 8,
                    execute: 6,
                    write: 12,
                    destructive: 2,
                    other: 12,
                },
                max_consecutive_tool_failures: 3,
                max_same_tool_signature_repeats: 2,
                post_mutation_verification_window: Some(PostMutationVerificationWindow {
                    replenish_read: 4,
                    replenish_search: 2,
                }),
            },
            TurnExecutionOrigin::InternalCuration | TurnExecutionOrigin::InboundObservation => {
                unreachable!("ordinary origin checked above")
            }
        };
        Ok(Self {
            context_label: EffectivePolicy::compile_for_origin(origin, governance).context_label,
            turn_budget,
            include_prompt_memory: true,
            strategy: StrategyProfile::plain_react(),
        })
    }
}

fn scaled_count(value: u32, multiplier: f64) -> u32 {
    if !multiplier.is_finite() || multiplier <= 0.0 {
        return value;
    }
    (f64::from(value) * multiplier)
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn scaled_turn_budget(mut budget: TurnBudgetPolicy, multiplier: f64) -> TurnBudgetPolicy {
    if !multiplier.is_finite() || (multiplier - 1.0).abs() < f64::EPSILON {
        return budget;
    }
    budget.emergency_hard_steps = scaled_count(budget.emergency_hard_steps, multiplier);
    budget.tool_call_limits = ToolCallBudgetLimits {
        total: scaled_count(budget.tool_call_limits.total, multiplier),
        read: scaled_count(budget.tool_call_limits.read, multiplier),
        search: scaled_count(budget.tool_call_limits.search, multiplier),
        fetch: scaled_count(budget.tool_call_limits.fetch, multiplier),
        execute: scaled_count(budget.tool_call_limits.execute, multiplier),
        write: scaled_count(budget.tool_call_limits.write, multiplier),
        destructive: scaled_count(budget.tool_call_limits.destructive, multiplier),
        other: scaled_count(budget.tool_call_limits.other, multiplier),
    };
    if let Some(window) = budget.post_mutation_verification_window.as_mut() {
        window.replenish_read = scaled_count(window.replenish_read, multiplier);
        window.replenish_search = scaled_count(window.replenish_search, multiplier);
    }
    budget
}

#[cfg(test)]
mod tests;
