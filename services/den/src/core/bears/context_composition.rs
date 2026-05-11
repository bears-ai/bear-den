use serde::{Deserialize, Serialize};
use sqlx::types::Json;

use super::{managed_blocks::{managed_space_block_key, ResolvedManagedBlockSet}, Bear, BearAgentRole};
use crate::errors::CustomError;

pub const CONTEXT_PROFILE_VERSION: u32 = 1;
pub const DEFAULT_ROLE_CONTRACT_VERSION: &str = "2";

const DEN_BASELINE: &str = r#"You are operating as a Bear in Den.
A Bear feels like one assistant to the user, but internally it has specialized roles backing different Spaces.
Preserve Space and role boundaries and do not claim tools or authority unavailable in the current runtime.
Ask before destructive or externally visible actions.
Do not intentionally remember secrets or credentials."#;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleContracts {
    pub talk: String,
    pub pair: String,
    pub curate: String,
    pub work: String,
    pub watch: String,
}

impl RoleContracts {
    pub fn get(&self, role: BearAgentRole) -> &str {
        match role {
            BearAgentRole::Talk => &self.talk,
            BearAgentRole::Pair => &self.pair,
            BearAgentRole::Curate => &self.curate,
            BearAgentRole::Work => &self.work,
            BearAgentRole::Watch => &self.watch,
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
    DEN_BASELINE
}

pub fn context_profile_from_json(
    value: &Option<Json<serde_json::Value>>,
) -> Result<Option<BearContextProfile>, CustomError> {
    let Some(value) = value else {
        return Ok(None);
    };
    serde_json::from_value(value.0.clone())
        .map(Some)
        .map_err(|e| CustomError::Parsing(format!("invalid bear context_profile: {e}")))
}

pub fn context_profile_to_json(
    profile: &BearContextProfile,
) -> Result<Json<serde_json::Value>, CustomError> {
    serde_json::to_value(profile)
        .map(Json)
        .map_err(|e| CustomError::Parsing(format!("serialize bear context_profile: {e}")))
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
    role: BearAgentRole,
    resolved: Option<&ResolvedManagedBlockSet>,
) -> Result<String, CustomError> {
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        return Ok(bear.system_prompt.trim().to_string());
    };

    let den_baseline_text = resolved
        .and_then(|resolved| {
            resolved
                .blocks
                .iter()
                .find(|block| block.key == "den_baseline")
                .map(|block| block.effective_content.as_str())
        })
        .unwrap_or_else(den_baseline);
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

    let user_steering = profile.user_steering.trim();
    let bear_context = profile.bear_context.trim();

    let mut composed = String::new();
    push_section(&mut composed, "Den baseline", den_baseline_text);
    let instructions_heading = match role {
        BearAgentRole::Talk => "Space instructions: Conversation Space".to_string(),
        BearAgentRole::Pair => "Space instructions: Collaboration Space".to_string(),
        BearAgentRole::Curate => "Space instructions: Curation Space".to_string(),
        BearAgentRole::Work => "Space instructions: Execution Space".to_string(),
        BearAgentRole::Watch => "Space instructions: Observation Space".to_string(),
    };
    push_section(&mut composed, &instructions_heading, &role_contract);
    push_section(&mut composed, "User steering", user_steering);
    push_section(&mut composed, "Bear context", bear_context);

    Ok(composed)
}

pub fn compose_role_context(
    bear: &Bear,
    role: BearAgentRole,
    runtime_context: Option<&str>,
) -> Result<ComposedRoleContext, CustomError> {
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        let legacy = bear.system_prompt.trim().to_string();
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

    let role_contract = profile.role_contracts.get(role).trim().to_string();
    let user_steering = profile.user_steering.trim();
    let bear_context = profile.bear_context.trim();
    let runtime_context = runtime_context.map(str::trim).filter(|s| !s.is_empty());

    let mut composed = render_managed_role_prompt(bear, role, None)?;
    if let Some(runtime_context) = runtime_context {
        push_section(&mut composed, "Runtime/thread context", runtime_context);
    }

    Ok(ComposedRoleContext {
        role: role.as_str().to_string(),
        den_baseline: den_baseline().to_string(),
        role_contract,
        user_steering: (!user_steering.is_empty()).then(|| user_steering.to_string()),
        bear_context: (!bear_context.is_empty()).then(|| bear_context.to_string()),
        runtime_context: runtime_context.map(str::to_string),
        composed_prompt: composed,
        is_legacy: false,
    })
}

pub fn render_role_prompt(bear: &Bear, role: BearAgentRole) -> Result<String, CustomError> {
    Ok(compose_role_context(bear, role, None)?.composed_prompt)
}

pub fn default_role_contracts_for_bear(name: &str) -> RoleContracts {
    RoleContracts {
        talk: format!(
            "You are the Bear's talk role: the conversational front door for {name}. Hold synchronous conversations in chat-like surfaces, answer directly when appropriate, and capture task intents when the user asks for external or autonomous work. Do not perform arbitrary outbound autonomous work or promote shared memory unilaterally."
        ),
        pair: format!(
            "You are {name}, the user's Bear, operating in Collaboration Space. Collaboration Space is the Bear's working environment for helping a human inside their current tool and active work context. Identify as the Bear, not as an internal role, sub-agent, or implementation component. When a concrete workspace, document set, design surface, plan, log, or other artifact is available, prefer advancing the task through direct inspection and client-mediated tool use rather than stopping at abstract explanation. Bias toward the first useful concrete action that is low-risk and feasible in the current client context: inspect the relevant artifact, trace the behavior, compare expected and actual state, draft the change, gather evidence, or otherwise move the work forward with minimal conversational delay. Treat code, documents, designs, logs, configs, plans, and other workspace materials as first-class work artifacts and primary evidence sources. In practice: inspect an existing codebase before diagnosing or editing it; when creating something new from scratch, create the first useful structure rather than staying abstract; when organizing a large collection of notes, sample the notes before designing a taxonomy; when adding a blog post to a site, inspect existing posts and publishing conventions before creating the new one. Use client-mediated tools with user approval where appropriate, keep changes reviewable, and report what changed. Do not perform autonomous outbound work outside the client-mediated permission model."
        ),
        curate: format!(
            "You are the Bear's curate role: the internal integrator for {name}. Review branches, task intents, observations, work results, and skill proposals. Promote durable knowledge into shared core memory through Den-controlled mechanisms. Do not perform outbound external communication."
        ),
        work: format!(
            "You are the Bear's work role: the approved outbound executor for {name}. Execute only Den-approved tasks within the provided run context, allowed tools, and scope. Use curated context rather than raw private interaction history. Do not self-approve tasks."
        ),
        watch: format!(
            "You are the Bear's watch role: the inbound observer for {name}. Parse inbound external events into structured observations for review. Do not take outbound action or directly convert events into external work without curate and Den mediation."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::Json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn test_bear(profile: Option<BearContextProfile>) -> Bear {
        Bear {
            id: Uuid::nil(),
            slug: "builder".to_string(),
            name: "Builder Bear".to_string(),
            description: String::new(),
            default_model: Some("openai/gpt-4o".to_string()),
            tools_enabled: None,
            letta_agent_type: None,
            letta_tool_ids: Json(Vec::new()),
            runtime_plan: None,
            context_profile: profile
                .as_ref()
                .map(context_profile_to_json)
                .transpose()
                .unwrap(),
            memfs_repo_path: None,
            provisioning_version: 1,
            system_prompt: "legacy prompt".to_string(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn legacy_bear_uses_system_prompt() {
        let bear = test_bear(None);
        let composed = compose_role_context(&bear, BearAgentRole::Talk, None).unwrap();
        assert!(composed.is_legacy);
        assert_eq!(composed.composed_prompt, "legacy prompt");
    }

    #[test]
    fn composed_bear_includes_layers_in_order() {
        let profile = BearContextProfile {
            composition_version: CONTEXT_PROFILE_VERSION,
            template_id: Some("software_product_builder".to_string()),
            template_version: Some("1".to_string()),
            role_contract_version: Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string()),
            role_contracts: default_role_contracts_for_bear("Builder Bear"),
            user_steering: "Prefer concise plans.".to_string(),
            bear_context: "The user builds BEARS.".to_string(),
            starter_prompts: vec![],
            first_task: None,
        };
        let bear = test_bear(Some(profile));
        let composed =
            compose_role_context(&bear, BearAgentRole::Pair, Some("Runtime now.")).unwrap();
        assert!(!composed.is_legacy);
        assert!(composed.composed_prompt.contains("# Den baseline"));
        assert!(composed
            .composed_prompt
            .contains("# Space instructions: Collaboration Space"));
        assert!(composed.composed_prompt.contains("# User steering"));
        assert!(composed.composed_prompt.contains("# Bear context"));
        assert!(composed
            .composed_prompt
            .contains("# Runtime/thread context"));
    }
}
