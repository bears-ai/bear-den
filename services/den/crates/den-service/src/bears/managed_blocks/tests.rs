use super::*;
use time::OffsetDateTime;

fn test_bear() -> Bear {
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
        context_profile: None,
        provisioning_version: 1,
        system_prompt: String::new(),
        birthday: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        live_reflection_enabled: true,
        live_reflection_stale_after_minutes: 30,
        live_reflection_activity_threshold: 20,
        live_reflection_sweep_limit: 25,
    }
}

#[test]
fn bound_compilation_ignores_historical_role_templates_and_hashes() {
    let mut bear = test_bear();
    bear.system_prompt = "{{ invalid_legacy_system".into();
    bear.context_profile = Some(Json(json!({
        "role_contracts": {
            "chat": "{{ invalid_legacy_chat", "pair": "{{ current_date }}",
            "curate": "{% invalid", "work": "{{ retired_variable }}", "watch": "{{"
        },
        "user_steering": "Use concise answers for {{ bear_name }}.",
        "bear_context": "Current charter: {{ bear_slug }}."
    })));
    let mut resolved = ResolvedManagedBlockSet {
        bear_id: bear.id,
        blocks: vec![ResolvedManagedBlock {
            key: managed_space_block_key(RuntimeContextLabel::ArmatureConversation),
            kind: "prompt_text".into(),
            scope: "space".into(),
            source_mode: "custom".into(),
            effective_content: "{{ invalid_legacy_binding".into(),
            effective_content_hash: content_hash("{{ invalid_legacy_binding"),
            system_version_id: None,
            system_version_number: None,
            forked_from_version_id: None,
            last_reviewed_version_id: None,
        }],
    };
    let first = compile_managed_config_for_bear(&bear, resolved.clone()).unwrap();
    let base = first.rendered_prompts["bound_base"].as_str().unwrap();
    assert!(base.contains("Use concise answers for Builder Bear."));
    assert!(base.contains("Current charter: builder."));
    assert!(!base.contains("invalid_legacy"));
    for role in RuntimeContextLabel::ALL {
        assert!(first.rendered_prompts.get(role.as_str()).is_none());
        assert!(first.rendered_prompt_hashes.get(role.as_str()).is_none());
    }
    assert!(first.rendered_prompts["bound_pair_mode"]
        .as_str()
        .unwrap()
        .contains("Interactive collaboration mode"));
    assert!(first.rendered_prompts["bound_chat_mode"]
        .as_str()
        .unwrap()
        .contains("Conversation mode"));
    assert!(first.rendered_prompts["bound_work_mode"]
        .as_str()
        .unwrap()
        .contains("Authorized Work mode"));
    bear.context_profile.as_mut().unwrap().0["role_contracts"] =
        json!("malformed retired metadata");
    resolved.blocks[0].effective_content = "Different historical binding".into();
    resolved.blocks[0].effective_content_hash = content_hash("Different historical binding");
    let next = compile_managed_config_for_bear(&bear, resolved).unwrap();
    assert_eq!(first.rendered_prompts, next.rendered_prompts);
    assert_eq!(first.rendered_prompt_hashes, next.rendered_prompt_hashes);
    assert_eq!(first.config_hash, next.config_hash);
    assert_ne!(
        first.resolved_blocks.blocks[0].effective_content,
        next.resolved_blocks.blocks[0].effective_content
    );
}

#[test]
fn bound_compilation_without_profile_uses_platform_baseline_not_legacy_prompt() {
    let mut bear = test_bear();
    bear.system_prompt = "{{ invalid_legacy_system".into();
    let compiled = compile_managed_config_for_bear(
        &bear,
        ResolvedManagedBlockSet {
            bear_id: bear.id,
            blocks: vec![],
        },
    )
    .unwrap();
    let base = compiled.rendered_prompts["bound_base"].as_str().unwrap();
    assert!(base.contains("# Den baseline"));
    assert!(!base.contains("invalid_legacy_system"));
}

#[test]
fn bound_compilation_still_validates_current_steering_and_bear_context() {
    for key in ["user_steering", "bear_context"] {
        let mut bear = test_bear();
        let mut profile = json!({"role_contracts": "ignored historical metadata"});
        profile[key] = json!("{{ invalid_current_template");
        bear.context_profile = Some(Json(profile));
        assert!(
            compile_managed_config_for_bear(
                &bear,
                ResolvedManagedBlockSet {
                    bear_id: bear.id,
                    blocks: vec![],
                }
            )
            .is_err(),
            "{key} must still be compiled and validated"
        );
    }
}

#[test]
fn content_hash_is_deterministic() {
    assert_eq!(content_hash("abc"), content_hash("abc"));
    assert_ne!(content_hash("abc"), content_hash("abcd"));
}

#[test]
fn managed_space_block_key_matches_roles() {
    assert_eq!(
        managed_space_block_key(RuntimeContextLabel::ChannelConversation),
        "space_instruction.chat"
    );
    assert_eq!(
        managed_space_block_key(RuntimeContextLabel::ArmatureConversation),
        "space_instruction.pair"
    );
    assert_eq!(
        managed_space_block_key(RuntimeContextLabel::Curation),
        "space_instruction.curate"
    );
    assert_eq!(
        managed_space_block_key(RuntimeContextLabel::JobRun),
        "space_instruction.work"
    );
    assert_eq!(
        managed_space_block_key(RuntimeContextLabel::Observation),
        "space_instruction.watch"
    );
}

#[test]
fn seed_data_contains_expected_blocks_in_order() {
    let blocks = system_block_seed_data();
    let keys: Vec<&str> = blocks.iter().map(|b| b.key).collect();
    assert_eq!(
        keys,
        vec![
            "den_baseline",
            "space_instruction.chat",
            "space_instruction.pair",
            "space_instruction.curate",
            "space_instruction.work",
            "space_instruction.watch",
        ]
    );
}

#[test]
fn resolved_blocks_json_serializes() {
    let resolved = ResolvedManagedBlockSet {
        bear_id: test_bear().id,
        blocks: vec![ResolvedManagedBlock {
            key: "den_baseline".to_string(),
            kind: "prompt_text".to_string(),
            scope: "global".to_string(),
            source_mode: "inherit".to_string(),
            effective_content: "hello".to_string(),
            effective_content_hash: content_hash("hello"),
            system_version_id: Some(1),
            system_version_number: Some(1),
            forked_from_version_id: None,
            last_reviewed_version_id: None,
        }],
    };
    let json = resolved_blocks_json(&resolved).unwrap();
    assert_eq!(json.0["blocks"][0]["key"], "den_baseline");
}
