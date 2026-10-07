use super::*;
use crate::agent_loop::source_admission::tests::fixture;
use den_service::{
    bears::{db, managed_blocks, model_configurations as configurations},
    conversation::{message_types::ConversationMessageWrite, persistence},
};

async fn next_session(
    deps: &NativeRuntimeDeps<'_>,
    source: &AgentLoopSession,
    run: &str,
    runtime_target: Option<&str>,
    human: &str,
) -> AgentLoopSession {
    build_session(
        deps,
        BuildSessionInput {
            origin: source.origin,
            bear_id: source.bear_id,
            conversation_id: &source.conversation_id,
            client_session_id: &source.client_session_id,
            human_message: Some(human),
            runtime_context: None,
            session_id: Some(&source.client_session_id),
            workspace_roots: None,
            runtime_target,
            conversation_selection: None,
            user_id: source.user_id,
            client_context: None,
            client_tools: None,
            request_id: Some(Uuid::new_v4()),
            run_id: Some(run),
            checkpoint_audit_context: None,
            work_run_id: None,
            stream_tokens: true,

            technical_budget_recovery_start_payload: None,
            tool_messages: vec![],
        },
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_model_effort_edits_require_a_fresh_native_turn(pool: PgPool) {
    use crate::agent_loop::source_admission::tests::step_context;
    use den_core::ThinkingEffort;

    let (source, _, hat) = fixture(&pool).await;
    crate::primary_model::tests::reasoning_support(&pool, Some(true)).await;
    let bear_id = source.bear_id.into();
    let configuration = configurations::create(
        &pool,
        bear_id,
        "Turn effort",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, bear_id, hat, Some(configuration.id))
        .await
        .unwrap();
    let bear = db::get_bear(&pool, source.bear_id).await.unwrap().unwrap();
    managed_blocks::compile_and_store_managed_config_for_bear(&pool, &bear)
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("turn-effort-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    let deps = NativeRuntimeDeps {
        pool: &pool,
        config: &config,
        stores: &stores,
    };
    let llm = LlmClient::new(&config);
    let mut current = next_session(&deps, &source, "effort-initial", None, "initial turn").await;
    assert_eq!(
        current.model_request_profile.thinking_effort,
        Some(ThinkingEffort::High)
    );

    for (index, effort) in [
        Some(ThinkingEffort::Low),
        None,
        Some(ThinkingEffort::Medium),
    ]
    .into_iter()
    .enumerate()
    {
        let turn_start_effort = current.model_request_profile.thinking_effort;
        let mut pending =
            run_agent_step_stream(&llm, &current, Some(step_context(&pool, &current)))
                .await
                .unwrap();
        configurations::update(
            &pool,
            bear_id,
            configuration.id,
            "Turn effort",
            "gpt-5",
            effort,
        )
        .await
        .unwrap();
        current.step += 1;
        let context = step_context(&pool, &current);
        let result = run_agent_step_stream(&llm, &current, Some(context.clone())).await;
        assert!(matches!(result, Err(DenError::ValidationError(ref message))
            if message.contains("reasoning effort changed") && message.contains("start a new turn")));
        let stored = context.session_store.get(&current.session_key).unwrap();
        assert_eq!(
            stored.model_request_profile.thinking_effort,
            turn_start_effort
        );
        assert!(
            stored.latest_context_budget.is_none(),
            "reject before preflight effects"
        );
        // A request constructed before the edit must also reject at lazy execution.
        let mut rejected = false;
        while let Some(event) = pending.next().await {
            if let Err(DenError::ValidationError(message)) = event {
                assert!(message.contains("start a new turn"));
                rejected = true;
                break;
            }
        }
        assert!(rejected);

        let fresh = next_session(
            &deps,
            &source,
            &format!("effort-refresh-{index}"),
            None,
            "fresh turn",
        )
        .await;
        assert_eq!(fresh.model, current.model, "only effort changed");
        assert_eq!(fresh.model_request_profile.thinking_effort, effort);
        let mut stream = run_agent_step_stream(&llm, &fresh, Some(step_context(&pool, &fresh)))
            .await
            .unwrap();
        let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
            detail: Some(detail),
            ..
        }) = stream.next().await.unwrap().unwrap()
        else {
            panic!("model request diagnostic")
        };
        assert_eq!(
            detail["effective_request_effort"],
            serde_json::json!(effort.map(ThinkingEffort::as_str))
        );
        drop(stream);
        SESSION_STORE.remove(&current.session_key);
        current = fresh;
    }
    SESSION_STORE.remove(&current.session_key);
    let _ = std::fs::remove_dir_all(&config.bear_sqlite_data_dir);
}

#[sqlx::test(migrations = "../../migrations")]
async fn next_turn_reloads_configuration_and_pin_without_runtime_target_guessing(pool: PgPool) {
    let (source, canonical, hat) = fixture(&pool).await;
    crate::primary_model::tests::reasoning_support(&pool, Some(true)).await;
    let bear_id = source.bear_id.into();
    let careful = configurations::create(
        &pool,
        bear_id,
        "Careful",
        "gpt-5",
        Some(den_core::ThinkingEffort::High),
    )
    .await
    .unwrap();
    let quick = configurations::create(&pool, bear_id, "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_default(&pool, bear_id, Some(quick.id))
        .await
        .unwrap();
    configurations::set_hat_override(&pool, bear_id, hat, Some(careful.id))
        .await
        .unwrap();
    let bear = db::get_bear(&pool, source.bear_id).await.unwrap().unwrap();
    managed_blocks::compile_and_store_managed_config_for_bear(&pool, &bear)
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("model-config-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    let deps = NativeRuntimeDeps {
        pool: &pool,
        config: &config,
        stores: &stores,
    };
    let decoy_hat = den_service::bears::hats::create_hat(
        &pool,
        bear_id,
        source.user_id.unwrap().into(),
        "Decoy",
        "Unrelated context",
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, bear_id, decoy_hat.id, Some(quick.id))
        .await
        .unwrap();
    let decoy = persistence::ensure_conversation_for_external_id(
        &pool,
        source.bear_id,
        source.user_id,
        "model-config-decoy",
        None,
        None,
    )
    .await
    .unwrap();
    den_service::bears::hats::bindings::bind_conversation_hat(
        &pool,
        bear_id,
        decoy.id,
        decoy_hat.id,
    )
    .await
    .unwrap();
    let first = next_session(
        &deps,
        &source,
        "model-config-first",
        Some("model-config-decoy"),
        "first user",
    )
    .await;
    assert_eq!(first.model, "openai/gpt-5");
    assert_eq!(
        first.model_request_profile.thinking_effort,
        Some(den_core::ThinkingEffort::High)
    );
    assert_eq!(
        first.model_request_profile.supports_reasoning_effort,
        Some(true)
    );
    assert_eq!(
        first.api_style,
        Some(crate::llm::LlmApiStyle::ResponsesStream)
    );
    persistence::append_message(
        &pool,
        canonical,
        &ConversationMessageWrite::user_turn("first user", serde_json::json!({}), None),
    )
    .await
    .unwrap();
    persistence::append_message(
        &pool,
        canonical,
        &ConversationMessageWrite::assistant_turn("first assistant", serde_json::json!({})),
    )
    .await
    .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        canonical,
        "explicit",
        Some("gpt-5"),
        Some("gpt-5"),
        None,
    )
    .await
    .unwrap();
    let pinned = next_session(&deps, &source, "model-config-pinned", None, "second user").await;
    assert_eq!(pinned.model, "openai/gpt-5");
    assert_eq!(
        pinned.model_request_profile.thinking_effort, None,
        "even a same-model pin replaces the entire config"
    );
    for text in ["first user", "first assistant", "second user"] {
        assert_eq!(
            pinned
                .messages
                .iter()
                .filter(|message| message.content.as_deref() == Some(text))
                .count(),
            1
        );
    }
    persistence::set_conversation_model_state(&pool, canonical, "auto", None, None, None)
        .await
        .unwrap();
    configurations::set_hat_override(&pool, bear_id, hat, Some(quick.id))
        .await
        .unwrap();
    let third = next_session(&deps, &source, "model-config-third", None, "third user").await;
    assert_eq!(third.model, "openai/gpt-4.1");
    assert_eq!(third.model_request_profile.thinking_effort, None);
    for session in [&first, &pinned, &third] {
        SESSION_STORE.remove(&session.session_key);
    }
    let _ = std::fs::remove_dir_all(&config.bear_sqlite_data_dir);
}
