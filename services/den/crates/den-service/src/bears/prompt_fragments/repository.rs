use den_core::DenError;
use sha2::{Digest, Sha256};

use super::{PromptBundleRegistry, PromptFragmentRegistry};

const REPOSITORY_PROMPT_SOURCES: &[(&str, &str)] = &[
    (
        "fragments/base/den_baseline.md",
        include_str!("../../../../../prompts/fragments/base/den_baseline.md"),
    ),
    (
        "fragments/base/bound_hat_identity.md",
        include_str!("../../../../../prompts/fragments/base/bound_hat_identity.md"),
    ),
    (
        "fragments/stances/bound_chat_mode.md",
        include_str!("../../../../../prompts/fragments/stances/bound_chat_mode.md"),
    ),
    (
        "fragments/stances/bound_pair_mode.md",
        include_str!("../../../../../prompts/fragments/stances/bound_pair_mode.md"),
    ),
    (
        "fragments/stances/bound_work_mode.md",
        include_str!("../../../../../prompts/fragments/stances/bound_work_mode.md"),
    ),
    (
        "fragments/stances/pair.md",
        include_str!("../../../../../prompts/fragments/stances/pair.md"),
    ),
    (
        "fragments/stances/work.md",
        include_str!("../../../../../prompts/fragments/stances/work.md"),
    ),
    (
        "fragments/stances/docket_coordination.md",
        include_str!("../../../../../prompts/fragments/stances/docket_coordination.md"),
    ),
    (
        "fragments/stances/docket_execution.md",
        include_str!("../../../../../prompts/fragments/stances/docket_execution.md"),
    ),
    (
        "fragments/runtime/read_only_authority.md",
        include_str!("../../../../../prompts/fragments/runtime/read_only_authority.md"),
    ),
    (
        "fragments/runtime/capability_discovery.md",
        include_str!("../../../../../prompts/fragments/runtime/capability_discovery.md"),
    ),
    (
        "fragments/runtime/work_checkout.md",
        include_str!("../../../../../prompts/fragments/runtime/work_checkout.md"),
    ),
    (
        "fragments/runtime/docket_execution_active.md",
        include_str!("../../../../../prompts/fragments/runtime/docket_execution_active.md"),
    ),
    (
        "fragments/runtime/objective_freeform.md",
        include_str!("../../../../../prompts/fragments/runtime/objective_freeform.md"),
    ),
    (
        "fragments/runtime/objective_oriented.md",
        include_str!("../../../../../prompts/fragments/runtime/objective_oriented.md"),
    ),
    (
        "fragments/runtime/objective_docket_execution.md",
        include_str!("../../../../../prompts/fragments/runtime/objective_docket_execution.md"),
    ),
    (
        "fragments/runtime/budget_warning.md",
        include_str!("../../../../../prompts/fragments/runtime/budget_warning.md"),
    ),
    (
        "fragments/runtime/run_recovery.md",
        include_str!("../../../../../prompts/fragments/runtime/run_recovery.md"),
    ),
    (
        "fragments/runtime/operational_outcome_summary.md",
        include_str!("../../../../../prompts/fragments/runtime/operational_outcome_summary.md"),
    ),
    (
        "fragments/runtime/task_list_final_gate_continuation.md",
        include_str!(
            "../../../../../prompts/fragments/runtime/task_list_final_gate_continuation.md"
        ),
    ),
];

const REPOSITORY_BUNDLE_SOURCES: &[(&str, &str)] = &[(
    "bundles/pair.yaml",
    include_str!("../../../../../prompts/bundles/pair.yaml"),
)];

pub fn repository_prompt_fragment_registry() -> Result<PromptFragmentRegistry, DenError> {
    PromptFragmentRegistry::from_embedded_sources(REPOSITORY_PROMPT_SOURCES)
}

pub fn repository_prompt_bundle_registry(
    fragments: &PromptFragmentRegistry,
) -> Result<PromptBundleRegistry, DenError> {
    PromptBundleRegistry::from_embedded_sources(REPOSITORY_BUNDLE_SOURCES, fragments)
}

pub fn repository_prompt_source_version() -> String {
    let mut hasher = Sha256::new();
    for (name, source) in REPOSITORY_PROMPT_SOURCES {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.update([0]);
    }
    for (name, source) in REPOSITORY_BUNDLE_SOURCES {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}
