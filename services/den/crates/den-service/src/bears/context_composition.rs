use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sqlx::types::Json;

use super::{
    managed_blocks::{managed_space_block_key, ResolvedManagedBlockSet},
    prompt_fragments::{
        render_compile_time_fragment, render_compile_time_text, CompileTimePromptContext,
        PromptFragmentRegistry,
    },
    Bear, BearProfile,
};
use den_core::DenError;

pub const CONTEXT_PROFILE_VERSION: u32 = 1;
pub const DEFAULT_ROLE_CONTRACT_VERSION: &str = "2";

const DEN_BASELINE_SOURCE: &str =
    include_str!("../../../../prompts/fragments/base/den_baseline.md");

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleContracts {
    #[serde(alias = "talk")]
    pub chat: String,
    pub pair: String,
    pub curate: String,
    pub work: String,
    pub watch: String,
}

impl RoleContracts {
    pub fn get(&self, role: BearProfile) -> &str {
        match role {
            BearProfile::Chat => &self.chat,
            BearProfile::Pair => &self.pair,
            BearProfile::Curate => &self.curate,
            BearProfile::Work => &self.work,
            BearProfile::Watch => &self.watch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BearContextProfile {
    #[serde(default = "default_composition_version")]
    pub composition_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_contract_version: Option<String>,
    pub role_contracts: RoleContracts,
    #[serde(default)]
    pub user_steering: String,
    #[serde(default)]
    pub bear_context: String,
    #[serde(default)]
    pub starter_prompts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_task: Option<String>,
}

fn default_composition_version() -> u32 {
    CONTEXT_PROFILE_VERSION
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposedRoleContext {
    pub role: String,
    pub den_baseline: String,
    pub role_contract: String,
    pub user_steering: Option<String>,
    pub bear_context: Option<String>,
    pub runtime_context: Option<String>,
    pub composed_prompt: String,
    pub is_legacy: bool,
}

pub fn den_baseline() -> &'static str {
    static BASELINE: OnceLock<String> = OnceLock::new();
    BASELINE
        .get_or_init(|| {
            PromptFragmentRegistry::from_embedded_sources(&[(
                "fragments/base/den_baseline.md",
                DEN_BASELINE_SOURCE,
            )])
            .expect("embedded den_baseline prompt fragment must parse")
            .require("den_baseline")
            .expect("embedded den_baseline prompt fragment must have id den_baseline")
            .body
            .trim()
            .to_string()
        })
        .as_str()
}

pub fn context_profile_from_json(
    value: &Option<Json<serde_json::Value>>,
) -> Result<Option<BearContextProfile>, DenError> {
    let Some(value) = value else {
        return Ok(None);
    };
    serde_json::from_value(value.0.clone())
        .map(Some)
        .map_err(|e| DenError::Parsing(format!("invalid bear context_profile: {e}")))
}

pub fn context_profile_to_json(
    profile: &BearContextProfile,
) -> Result<Json<serde_json::Value>, DenError> {
    serde_json::to_value(profile)
        .map(Json)
        .map_err(|e| DenError::Parsing(format!("serialize bear context_profile: {e}")))
}

fn push_section(out: &mut String, heading: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("# ");
    out.push_str(heading);
    out.push('\n');
    out.push_str(body);
}

pub fn render_managed_role_prompt(
    bear: &Bear,
    role: BearProfile,
    resolved: Option<&ResolvedManagedBlockSet>,
) -> Result<String, DenError> {
    render_managed_role_prompt_with_registry(bear, role, resolved, None)
}

pub fn render_managed_role_prompt_with_registry(
    bear: &Bear,
    role: BearProfile,
    resolved: Option<&ResolvedManagedBlockSet>,
    registry: Option<&PromptFragmentRegistry>,
) -> Result<String, DenError> {
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        return Ok(bear.system_prompt.trim().to_string());
    };

    let den_baseline_text = resolved
        .and_then(|resolved| {
            resolved
                .blocks
                .iter()
                .find(|block| block.key == "den_baseline")
                .map(|block| block.effective_content.clone())
        })
        .or_else(|| {
            registry
                .and_then(|registry| registry.get("den_baseline"))
                .map(|fragment| fragment.body.clone())
        })
        .unwrap_or_else(|| den_baseline().to_string());
    let compile_context = CompileTimePromptContext {
        bear_name: &bear.name,
        bear_slug: &bear.slug,
    };
    let den_baseline_text =
        render_compile_time_text("den_baseline", &den_baseline_text, &compile_context)?;
    let role_contract = resolved
        .and_then(|resolved| {
            let key = managed_space_block_key(role);
            resolved
                .blocks
                .iter()
                .find(|block| block.key == key)
                .map(|block| block.effective_content.trim().to_string())
        })
        .unwrap_or_else(|| profile.role_contracts.get(role).trim().to_string());
    let role_contract_key = managed_space_block_key(role);
    let role_contract =
        render_compile_time_text(&role_contract_key, &role_contract, &compile_context)?;

    let user_steering = render_compile_time_text(
        "context_profile.user_steering",
        profile.user_steering.trim(),
        &compile_context,
    )?;
    let bear_context = render_compile_time_text(
        "context_profile.bear_context",
        profile.bear_context.trim(),
        &compile_context,
    )?;

    let mut composed = String::new();
    push_section(&mut composed, "Den baseline", &den_baseline_text);
    let instructions_heading = match role {
        BearProfile::Chat => "Space instructions: Conversation Space".to_string(),
        BearProfile::Pair => "Space instructions: Collaboration Space".to_string(),
        BearProfile::Curate => "Space instructions: Curation Space".to_string(),
        BearProfile::Work => "Space instructions: Execution Space".to_string(),
        BearProfile::Watch => "Space instructions: Observation Space".to_string(),
    };
    push_section(&mut composed, &instructions_heading, &role_contract);
    push_section(&mut composed, "User steering", &user_steering);
    push_section(&mut composed, "Bear context", &bear_context);

    Ok(composed)
}

/// Common identity/steering for hat-bound turns. Stance contracts are
/// intentionally absent: their legacy identity prose must not override a
/// selected hat. Execution-mode instructions are compiled separately from
/// trusted repository fragments.
pub fn render_bound_base_prompt_with_registry(
    bear: &Bear,
    resolved: Option<&ResolvedManagedBlockSet>,
    registry: &PromptFragmentRegistry,
) -> Result<String, DenError> {
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        return Ok(bear.system_prompt.trim().to_string());
    };
    let context = CompileTimePromptContext {
        bear_name: &bear.name,
        bear_slug: &bear.slug,
    };
    let baseline = resolved
        .and_then(|resolved| {
            resolved
                .blocks
                .iter()
                .find(|block| block.key == "den_baseline")
                .map(|block| block.effective_content.clone())
        })
        .or_else(|| {
            registry
                .get("den_baseline")
                .map(|fragment| fragment.body.clone())
        })
        .unwrap_or_else(|| den_baseline().to_string());
    let mut composed = String::new();
    push_section(
        &mut composed,
        "Den baseline",
        &render_compile_time_text("den_baseline", &baseline, &context)?,
    );
    push_section(
        &mut composed,
        "User steering",
        &render_compile_time_text(
            "context_profile.user_steering",
            profile.user_steering.trim(),
            &context,
        )?,
    );
    push_section(
        &mut composed,
        "Bear context",
        &render_compile_time_text(
            "context_profile.bear_context",
            profile.bear_context.trim(),
            &context,
        )?,
    );
    Ok(composed)
}

pub fn compose_role_context(
    bear: &Bear,
    role: BearProfile,
    runtime_context: Option<&str>,
) -> Result<ComposedRoleContext, DenError> {
    let runtime_context = runtime_context.map(str::trim).filter(|s| !s.is_empty());
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        let mut legacy = bear.system_prompt.trim().to_string();
        if let Some(runtime_context) = runtime_context {
            push_section(&mut legacy, "Runtime/thread context", runtime_context);
        }
        return Ok(ComposedRoleContext {
            role: role.as_str().to_string(),
            den_baseline: String::new(),
            role_contract: String::new(),
            user_steering: None,
            bear_context: None,
            runtime_context: runtime_context.map(str::to_string),
            composed_prompt: legacy,
            is_legacy: true,
        });
    };

    let compile_context = CompileTimePromptContext {
        bear_name: &bear.name,
        bear_slug: &bear.slug,
    };
    let den_baseline = render_compile_time_text("den_baseline", den_baseline(), &compile_context)?;
    let role_contract_key = managed_space_block_key(role);
    let role_contract = render_compile_time_text(
        &role_contract_key,
        profile.role_contracts.get(role).trim(),
        &compile_context,
    )?;
    let user_steering = render_compile_time_text(
        "context_profile.user_steering",
        profile.user_steering.trim(),
        &compile_context,
    )?;
    let bear_context = render_compile_time_text(
        "context_profile.bear_context",
        profile.bear_context.trim(),
        &compile_context,
    )?;

    let mut composed = render_managed_role_prompt(bear, role, None)?;
    if let Some(runtime_context) = runtime_context {
        push_section(&mut composed, "Runtime/thread context", runtime_context);
    }

    Ok(ComposedRoleContext {
        role: role.as_str().to_string(),
        den_baseline,
        role_contract,
        user_steering: (!user_steering.is_empty()).then_some(user_steering),
        bear_context: (!bear_context.is_empty()).then_some(bear_context),
        runtime_context: runtime_context.map(str::to_string),
        composed_prompt: composed,
        is_legacy: false,
    })
}

pub fn render_role_prompt(bear: &Bear, role: BearProfile) -> Result<String, DenError> {
    Ok(compose_role_context(bear, role, None)?.composed_prompt)
}

fn default_pair_contract_for_bear(name: &str) -> String {
    let registry = super::prompt_fragments::repository_prompt_fragment_registry().ok();
    registry
        .as_ref()
        .and_then(|registry| registry.get("stance_pair"))
        .and_then(|fragment| {
            render_compile_time_fragment(
                fragment,
                &CompileTimePromptContext {
                    bear_name: name,
                    bear_slug: "",
                },
            )
            .ok()
        })
        .unwrap_or_else(|| {
            format!(
                "You are {name}, the user's Bear, operating in Collaboration Space. Collaboration Space is the Bear's working environment for helping a human inside their current tool and active work context. Identify as the Bear, not as an internal stance, sub-agent, or implementation component. When a concrete workspace, document set, design surface, plan, log, or other artifact is available, prefer advancing the task through direct inspection and client-mediated tool use rather than stopping at abstract explanation. Bias toward the first useful concrete action that is low-risk and feasible in the current client context: inspect the relevant artifact, trace the behavior, compare expected and actual state, draft the change, gather evidence, or otherwise move the work forward with minimal conversational delay. Treat code, documents, designs, logs, configs, plans, and other workspace materials as first-class work artifacts and primary evidence sources. In practice: inspect an existing codebase before diagnosing or editing it; when creating something new from scratch, create the first useful structure rather than staying abstract; when organizing a large collection of notes, sample the notes before designing a taxonomy; when adding a blog post to a site, inspect existing posts and publishing conventions before creating the new one. When the user asks to make, create, draft, update, or track a plan or task list, prefer planning-state tools when the current runtime makes them available rather than satisfying the request with only conversational bullets. If planning-state tools are unavailable, explain that limitation and provide a provisional conversational plan if helpful. Do not write active plans or ephemeral progress to durable memory unless the user explicitly asks to save them as durable memory. Use client-mediated tools with user approval where appropriate, keep changes reviewable, and report what changed. Do not perform autonomous outbound work outside the client-mediated permission model."
            )
        })
}

fn default_work_contract_for_bear(name: &str) -> String {
    let registry = super::prompt_fragments::repository_prompt_fragment_registry().ok();
    registry
        .as_ref()
        .and_then(|registry| registry.get("stance_work"))
        .and_then(|fragment| {
            render_compile_time_fragment(
                fragment,
                &CompileTimePromptContext {
                    bear_name: name,
                    bear_slug: "",
                },
            )
            .ok()
        })
        .unwrap_or_else(|| {
            format!(
                "You are the Bear's work stance: the approved outbound executor for {name}. Execute only Den-approved tasks within the provided run context, allowed tools, and scope. Use curated context rather than raw private interaction history. Prefer dedicated tools over generic command execution whenever the current runtime makes them available. Use generic command execution only when the task actually requires running a command or when no dedicated tool expresses the needed operation. Do not self-approve tasks."
            )
        })
}

pub fn default_role_contracts_for_bear(name: &str) -> RoleContracts {
    RoleContracts {
        chat: format!(
            "You are the Bear's chat role: the conversational front door for {name}. Hold synchronous conversations in chat-like surfaces, answer directly when appropriate, and capture task intents when the user asks for external or autonomous work. Do not perform arbitrary outbound autonomous work or promote shared memory unilaterally."
        ),
        pair: default_pair_contract_for_bear(name),
        curate: format!(
            "You are the Bear's curate role: the internal integrator for {name}. Review branches, task intents, observations, work results, and skill proposals. Promote durable knowledge into shared core memory through Den-controlled mechanisms. Do not perform outbound external communication."
        ),
        work: default_work_contract_for_bear(name),
        watch: format!(
            "You are the Bear's watch role: the inbound observer for {name}. Parse inbound external events into structured observations for review. Do not take outbound action or directly convert events into external work without curate and Den mediation."
        ),
    }
}

#[cfg(test)]
mod tests;
