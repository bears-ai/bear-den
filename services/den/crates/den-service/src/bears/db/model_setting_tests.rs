use super::*;

#[test]
fn parses_agent_loop_control_settings() {
    assert_eq!(
        parse_agent_loop_control_setting(Some("careful")).unwrap(),
        Some(AgentLoopControlLevel::Careful)
    );
    assert_eq!(parse_agent_loop_control_setting(Some(" ")).unwrap(), None);
    assert!(parse_agent_loop_control_setting(Some("careless")).is_err());
}

#[test]
fn resolves_bear_default_then_deployment_default() {
    assert_eq!(
        resolve_model_from_values(Some(" openai/gpt-4o-mini "), "openai/gpt-5-mini"),
        "openai/gpt-4o-mini"
    );
    for bear_default in [None, Some(""), Some("   ")] {
        assert_eq!(
            resolve_model_from_values(bear_default, " openai/gpt-5-mini "),
            "openai/gpt-5-mini"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn historic_profile_settings_do_not_override_or_populate_bear_defaults(pool: PgPool) {
    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "model-default-test-bear",
            name: "Model Default Test Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create bear");

    // Seed a historical row using the legacy admin writers, pending their cutover.
    set_profile_model_setting(
        &pool,
        bear_id,
        RuntimeContextLabel::JobRun,
        Some("openai/gpt-4.1"),
    )
    .await
    .expect("seed historical profile model");
    set_profile_agent_loop_control_setting(
        &pool,
        bear_id,
        RuntimeContextLabel::JobRun,
        Some(AgentLoopControlLevel::Strict),
    )
    .await
    .expect("seed historical profile loop control");

    let mut bear = get_bear(&pool, bear_id).await.unwrap().unwrap();
    assert_eq!(bear.default_model, None);
    assert_eq!(
        resolve_model_for_bear(&bear, "openai/gpt-5-mini"),
        "openai/gpt-5-mini"
    );
    assert_eq!(
        bear_agent_loop_control_setting(&pool, bear_id)
            .await
            .unwrap(),
        None
    );

    bear.default_model = Some("openai/gpt-4o-mini".into());
    assert_eq!(
        resolve_model_for_bear(&bear, "openai/gpt-5-mini"),
        "openai/gpt-4o-mini"
    );
    set_bear_agent_loop_control_setting(&pool, bear_id, Some(AgentLoopControlLevel::Standard))
        .await
        .expect("set bear loop control");
    assert_eq!(
        bear_agent_loop_control_setting(&pool, bear_id)
            .await
            .unwrap(),
        Some(AgentLoopControlLevel::Standard)
    );
    set_bear_agent_loop_control_setting(&pool, bear_id, None)
        .await
        .expect("clear bear loop control");
    assert_eq!(
        bear_agent_loop_control_setting(&pool, bear_id)
            .await
            .unwrap(),
        None
    );

    let settings = list_profile_model_settings(&pool, bear_id).await.unwrap();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0].model.as_deref(), Some("openai/gpt-4.1"));
    assert_eq!(
        settings[0].agent_loop_control_level.as_deref(),
        Some("strict")
    );
    assert_eq!(
        get_bear(&pool, bear_id)
            .await
            .unwrap()
            .unwrap()
            .default_model,
        None
    );
}
