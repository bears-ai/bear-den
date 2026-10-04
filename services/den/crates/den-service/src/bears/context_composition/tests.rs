use super::*;
use crate::bears::managed_blocks::{
    content_hash, managed_space_block_key, ResolvedManagedBlock, ResolvedManagedBlockSet,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn test_bear(profile: Option<BearContextProfile>) -> Bear {
    Bear {
        id: Uuid::nil(),
        slug: "builder".to_string(),
        name: "Builder Bear".to_string(),
        description: String::new(),
        default_model: Some("openai/gpt-4o".to_string()),
        default_tool_budget_multiplier: None,
        tools_enabled: None,
        work_enabled: false,
        cabinet_enabled: true,
        runtime_plan: None,
        context_profile: profile
            .as_ref()
            .map(context_profile_to_json)
            .transpose()
            .unwrap(),
        provisioning_version: 1,
        system_prompt: "legacy prompt".to_string(),
        birthday: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        live_reflection_enabled: true,
        live_reflection_stale_after_minutes: 30,
        live_reflection_activity_threshold: 20,
        live_reflection_sweep_limit: 25,
    }
}

fn resolved_block(key: &str, content: &str) -> ResolvedManagedBlock {
    ResolvedManagedBlock {
        key: key.to_string(),
        kind: "prompt_text".to_string(),
        scope: if key == "den_baseline" {
            "global"
        } else {
            "space"
        }
        .to_string(),
        source_mode: "custom".to_string(),
        effective_content: content.to_string(),
        effective_content_hash: content_hash(content),
        system_version_id: None,
        system_version_number: None,
        forked_from_version_id: None,
        last_reviewed_version_id: None,
    }
}

#[test]
fn legacy_bear_uses_system_prompt() {
    let bear = test_bear(None);
    let composed =
        compose_role_context(&bear, RuntimeContextLabel::ChannelConversation, None).unwrap();
    assert!(composed.is_legacy);
    assert_eq!(composed.composed_prompt, "legacy prompt");
}

#[test]
fn legacy_bear_trims_blank_runtime_context_metadata() {
    let bear = test_bear(None);
    let composed = compose_role_context(
        &bear,
        RuntimeContextLabel::ChannelConversation,
        Some("  \n\t  "),
    )
    .unwrap();
    assert!(composed.is_legacy);
    assert_eq!(composed.runtime_context, None);
}

#[test]
fn legacy_bear_includes_non_blank_runtime_context_in_prompt() {
    let bear = test_bear(None);
    let composed = compose_role_context(
        &bear,
        RuntimeContextLabel::ChannelConversation,
        Some(" Runtime now. "),
    )
    .unwrap();
    assert!(composed.is_legacy);
    assert_eq!(composed.runtime_context, Some("Runtime now.".to_string()));
    assert_eq!(
        composed.composed_prompt,
        "legacy prompt\n\n# Runtime/thread context\nRuntime now."
    );
}

#[test]
fn den_baseline_comes_from_repository_fragment_body() {
    let registry = PromptFragmentRegistry::from_embedded_sources(&[(
        "fragments/base/den_baseline.md",
        DEN_BASELINE_SOURCE,
    )])
    .unwrap();
    let fragment = registry.require("den_baseline").unwrap();
    assert_eq!(den_baseline(), fragment.body.trim());
    assert!(!den_baseline().contains("---"));
    assert!(!den_baseline().contains("layer: base"));
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
    let composed = compose_role_context(
        &bear,
        RuntimeContextLabel::ArmatureConversation,
        Some("Runtime now."),
    )
    .unwrap();
    assert!(!composed.is_legacy);
    let den_baseline = composed.composed_prompt.find("# Den baseline").unwrap();
    let space_instructions = composed
        .composed_prompt
        .find("# Space instructions: Collaboration Space")
        .unwrap();
    let user_steering = composed.composed_prompt.find("# User steering").unwrap();
    let bear_context = composed.composed_prompt.find("# Bear context").unwrap();
    let runtime_context = composed
        .composed_prompt
        .find("# Runtime/thread context")
        .unwrap();
    assert!(den_baseline < space_instructions);
    assert!(space_instructions < user_steering);
    assert!(user_steering < bear_context);
    assert!(bear_context < runtime_context);
}

#[test]
fn managed_prompt_renders_compile_time_template_fields() {
    let mut profile = BearContextProfile {
        composition_version: CONTEXT_PROFILE_VERSION,
        template_id: None,
        template_version: None,
        role_contract_version: Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string()),
        role_contracts: default_role_contracts_for_bear("{{ bear_name }}"),
        user_steering: "Prefer concise plans for {{ bear_name }}.".to_string(),
        bear_context: "Slug: {{ bear_slug }}.".to_string(),
        starter_prompts: vec![],
        first_task: None,
    };
    profile.role_contracts.chat = "Speak as {{ bear_name }}.".to_string();
    let bear = test_bear(Some(profile));
    let prompt =
        render_managed_role_prompt(&bear, RuntimeContextLabel::ChannelConversation, None).unwrap();
    assert!(prompt.contains("Speak as Builder Bear."));
    assert!(prompt.contains("Prefer concise plans for Builder Bear."));
    assert!(prompt.contains("Slug: builder."));

    let composed =
        compose_role_context(&bear, RuntimeContextLabel::ChannelConversation, None).unwrap();
    assert_eq!(composed.role_contract, "Speak as Builder Bear.");
    assert_eq!(
        composed.user_steering,
        Some("Prefer concise plans for Builder Bear.".to_string())
    );
    assert_eq!(composed.bear_context, Some("Slug: builder.".to_string()));
}

#[test]
fn managed_prompt_rejects_turn_time_template_fields() {
    let profile = BearContextProfile {
        composition_version: CONTEXT_PROFILE_VERSION,
        template_id: None,
        template_version: None,
        role_contract_version: Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string()),
        role_contracts: RoleContracts {
            chat: "Today is {{ current_date }}.".to_string(),
            ..default_role_contracts_for_bear("Builder Bear")
        },
        user_steering: String::new(),
        bear_context: String::new(),
        starter_prompts: vec![],
        first_task: None,
    };
    let bear = test_bear(Some(profile));
    let err = render_managed_role_prompt(&bear, RuntimeContextLabel::ChannelConversation, None)
        .unwrap_err();
    assert!(err.to_string().contains("failed to render"));
}

#[test]
fn managed_resolved_blocks_override_profile_and_repository_defaults() {
    let profile = BearContextProfile {
        composition_version: CONTEXT_PROFILE_VERSION,
        template_id: None,
        template_version: None,
        role_contract_version: Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string()),
        role_contracts: RoleContracts {
            chat: "Profile chat contract.".to_string(),
            ..default_role_contracts_for_bear("Builder Bear")
        },
        user_steering: String::new(),
        bear_context: String::new(),
        starter_prompts: vec![],
        first_task: None,
    };
    let bear = test_bear(Some(profile));
    let resolved = ResolvedManagedBlockSet {
        bear_id: bear.id,
        blocks: vec![
            resolved_block("den_baseline", "Managed baseline for {{ bear_name }}."),
            resolved_block(
                &managed_space_block_key(RuntimeContextLabel::ChannelConversation),
                "Managed chat for {{ bear_slug }}.",
            ),
        ],
    };

    let prompt = render_managed_role_prompt(
        &bear,
        RuntimeContextLabel::ChannelConversation,
        Some(&resolved),
    )
    .unwrap();

    assert!(prompt.contains("# Den baseline\nManaged baseline for Builder Bear."));
    assert!(prompt.contains("# Space instructions: Conversation Space\nManaged chat for builder."));
    assert!(!prompt.contains("Profile chat contract."));
    assert!(!prompt.contains("You are operating as a Bear in Den."));
}

#[test]
fn pair_role_contract_includes_plan_request_guidance() {
    let pair_contract = default_role_contracts_for_bear("Builder Bear").pair;
    assert!(pair_contract.contains(
        "When the user asks to make, create, draft, update, or track a plan or task list"
    ));
    assert!(pair_contract
        .contains("prefer planning-state tools when the current runtime makes them available"));
    assert!(
        pair_contract.contains("Do not write active plans or ephemeral progress to durable memory")
    );
}
