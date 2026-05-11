//! Bear registry and membership (Phase 1).
//! Admin HTTP routes and Letta provisioning wire up in later milestones.

pub mod context_composition;
pub mod db;
pub mod letta_code_harness;
pub mod letta_drift;
pub mod managed_blocks;
pub mod model;
pub mod provision;
pub mod rollout;
pub mod runtime_plan;
pub mod sync;
pub mod templates;

pub use context_composition::{
    compose_role_context, context_profile_from_json, context_profile_to_json,
    default_role_contracts_for_bear, render_managed_role_prompt, BearContextProfile,
    ComposedRoleContext, RoleContracts,
};
pub use letta_drift::{
    compute_letta_drift, compute_letta_drift_with_expected_tool_ids, LettaDriftFlags,
};
pub use managed_blocks::{
    compile_and_store_managed_config_for_bear, compile_managed_config_for_bear,
    get_compiled_bear_config, list_bear_block_bindings, list_system_block_versions,
    list_system_blocks, managed_space_block_key, resolve_managed_blocks_for_bear,
    resolved_blocks_json, seed_system_blocks, upsert_bear_block_binding,
    upsert_compiled_bear_config, BearBlockBindingMode, BearBlockBindingRow,
    BearCompiledConfigRow, CompiledBearConfig, ResolvedManagedBlock, ResolvedManagedBlockSet,
    SeedSystemBlock, SystemBlockKind, SystemBlockRow, SystemBlockScope, SystemBlockVersionRow,
};
pub use model::{
    Bear, BearAgent, BearAgentRole, BearSkillManifestEntry, BearSkillProposal, BearWithMembership,
};
pub use runtime_plan::{default_runtime_plan, effective_runtime_plan};
