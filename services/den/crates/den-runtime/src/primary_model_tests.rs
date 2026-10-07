use super::*;
use crate::agent_loop::{
    run_agent_step_stream,
    source_admission::tests::{fixture, step_context, work_fixture},
};
use den_service::{
    bears::{
        db,
        model_configurations::{self as configurations, PrimaryModelSource},
    },
    conversation::persistence,
};
use futures::StreamExt;

#[sqlx::test(migrations = "../../migrations")]
async fn channel_model_diagnostic_is_durable_even_without_the_bearwire_publisher(pool: PgPool) {
    let (mut session, _, _) = fixture(&pool).await;
    session.origin = den_core::TurnExecutionOrigin::ChannelConversation;
    session.profile = den_core::RuntimeContextLabel::ChannelConversation;
    let llm = crate::llm::LlmClient::new(&Config::test_stub());
    let mut stream = run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
        .await
        .unwrap();
    // The disabled test client fails without contacting any upstream provider;
    // the validated request diagnostic must already be persisted at that edge.
    while let Some(event) = stream.next().await {
        if event.is_err() {
            break;
        }
    }
    let rows = crate::bearwire_events::list_bearwire_events_for_run(
        &pool,
        session.run_id.as_deref().unwrap(),
        100,
    )
    .await
    .unwrap();
    let diagnostic = rows
        .iter()
        .find(|row| row.event.data["kind"] == "model_request_profile_resolved")
        .expect("durable model request diagnostic");
    assert_eq!(
        diagnostic.event.scope,
        bearwire_protocol::wire::BearWireEventScope::Persistent
    );
    let detail = &diagnostic.event.data["detail"];
    assert_eq!(detail["source"], "deployment_default");
    assert_eq!(detail["model"], "openai/gpt-4.1");
    assert!(detail["configuration_id"].is_null());
    assert!(detail["configured_thinking_effort"].is_null());
    assert!(detail["effective_request_effort"].is_null());
}

pub(crate) async fn reasoning_support(pool: &PgPool, support: Option<bool>) {
    sqlx::query!(
        r"UPDATE model_selection_options
          SET metadata_json = jsonb_set(metadata_json, '{supports_reasoning_effort}',
              COALESCE(to_jsonb($1::boolean), 'null'::jsonb))
          WHERE handle = 'openai/gpt-5'",
        support,
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_conversation_inheritance_pin_clear_and_selection_view_agree(pool: PgPool) {
    let (session, canonical, hat) = fixture(&pool).await;
    let bear_id = session.bear_id.into();
    let source = NativeTurnSource::Conversation(canonical);
    reasoning_support(&pool, Some(true)).await;
    let careful = configurations::create(
        &pool,
        bear_id,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let quick = configurations::create(&pool, bear_id, "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    assert_eq!(
        resolve_for_source(&pool, bear_id, source, "gpt-4.1")
            .await
            .unwrap()
            .source,
        PrimaryModelSource::DeploymentDefault
    );
    configurations::set_default(&pool, bear_id, Some(careful.id))
        .await
        .unwrap();
    let inherited = resolve_for_source(&pool, bear_id, source, "missing/default")
        .await
        .unwrap();
    assert_eq!(inherited.configuration_id, Some(careful.id));
    assert_eq!(inherited.thinking_effort, Some(ThinkingEffort::High));
    configurations::set_hat_override(&pool, bear_id, hat, Some(quick.id))
        .await
        .unwrap();
    let selected = resolve_for_source(&pool, bear_id, source, "missing/default")
        .await
        .unwrap();
    assert_eq!(selected.source, PrimaryModelSource::HatOverride);
    assert_eq!(selected.configuration_id, Some(quick.id));
    assert_eq!(
        selected.thinking_effort, None,
        "hat Model default is not Bear reasoning inheritance"
    );
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
    let pin = resolve_for_source(&pool, bear_id, source, "missing/default")
        .await
        .unwrap();
    assert_eq!(pin.source, PrimaryModelSource::ConversationPin);
    assert_eq!(pin.configuration_id, None);
    assert_eq!(pin.configuration_name, None);
    assert_eq!(pin.thinking_effort, None);
    let bear = db::get_bear(&pool, session.bear_id).await.unwrap().unwrap();
    let view = den_service::model_selection::load_conversation_model_selection_view(
        &pool,
        &bear,
        session.user_id.unwrap(),
        "gpt-4.1",
        &session.conversation_id,
        Some(&session.client_session_id),
        false,
    )
    .await
    .unwrap();
    assert_eq!(view.source, "conversation_explicit");
    assert_eq!(view.effective_model, pin.model_handle);
    assert_eq!(view.thinking_effort, None);
    persistence::set_conversation_model_state(&pool, canonical, "auto", None, None, None)
        .await
        .unwrap();
    let view = den_service::model_selection::load_conversation_model_selection_view(
        &pool,
        &bear,
        session.user_id.unwrap(),
        "gpt-4.1",
        &session.conversation_id,
        Some(&session.client_session_id),
        false,
    )
    .await
    .unwrap();
    assert_eq!(view.source, "hat_override");
    assert_eq!(view.configuration_id, Some(quick.id));
    configurations::set_hat_override(&pool, bear_id, hat, None)
        .await
        .unwrap();
    let view = den_service::model_selection::load_conversation_model_selection_view(
        &pool,
        &bear,
        session.user_id.unwrap(),
        "gpt-4.1",
        &session.conversation_id,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(view.source, "bear_default");
    assert_eq!(view.thinking_effort, Some(ThinkingEffort::High));
    configurations::set_default(&pool, bear_id, None)
        .await
        .unwrap();
    assert_eq!(
        den_service::model_selection::load_conversation_model_selection_view(
            &pool,
            &bear,
            session.user_id.unwrap(),
            "gpt-4.1",
            &session.conversation_id,
            None,
            false,
        )
        .await
        .unwrap()
        .source,
        "deployment_default"
    );
    // A malformed explicit pin is not permission to use a lower-precedence config.
    persistence::set_conversation_model_state(
        &pool,
        canonical,
        "explicit",
        Some(""),
        Some(""),
        None,
    )
    .await
    .unwrap();
    assert!(resolve_for_source(&pool, bear_id, source, "gpt-4.1")
        .await
        .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_uses_verified_job_hat_not_a_conversation_selector(pool: PgPool) {
    let (session, hat) = work_fixture(&pool).await;
    reasoning_support(&pool, Some(true)).await;
    let careful = configurations::create(
        &pool,
        session.bear_id.into(),
        "Work careful",
        "gpt-5",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, session.bear_id.into(), hat, Some(careful.id))
        .await
        .unwrap();
    let canonical = persistence::get_conversation_for_external_id(
        &pool,
        session.bear_id,
        &session.conversation_id,
    )
    .await
    .unwrap()
    .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        canonical.id,
        "explicit",
        Some("gpt-4.1"),
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let primary = resolve_for_source(
        &pool,
        session.bear_id.into(),
        NativeTurnSource::WorkRun(session.work_run_id.unwrap()),
        "gpt-4.1",
    )
    .await
    .unwrap();
    assert_eq!(primary.source, PrimaryModelSource::HatOverride);
    assert_eq!(primary.configuration_id, Some(careful.id));
    assert_eq!(primary.thinking_effort, Some(ThinkingEffort::Medium));
    den_service::bears::hats::manage::disable_work(&pool, session.bear_id.into(), hat)
        .await
        .unwrap();
    assert!(resolve_for_source(
        &pool,
        session.bear_id.into(),
        NativeTurnSource::WorkRun(session.work_run_id.unwrap()),
        "gpt-4.1"
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn each_loop_step_and_lazy_consumption_revalidate_selection_and_capabilities(pool: PgPool) {
    let (mut session, _, hat) = fixture(&pool).await;
    reasoning_support(&pool, Some(true)).await;
    let careful = configurations::create(
        &pool,
        session.bear_id.into(),
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, session.bear_id.into(), hat, Some(careful.id))
        .await
        .unwrap();
    session.model = "openai/gpt-5".into();
    session.model_request_profile.approved_model_ref = session.model.clone();
    session.model_request_profile.thinking_effort = careful.thinking_effort;
    // Stale adapter support is not the execution authority.
    session.model_request_profile.supports_reasoning_effort = Some(false);
    let llm = crate::llm::LlmClient::new(&Config::test_stub());
    let mut stream = run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
        .await
        .unwrap();
    let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        detail: Some(detail),
        ..
    }) = stream.next().await.unwrap().unwrap()
    else {
        panic!("model diagnostics")
    };
    assert_eq!(detail["effective_request_effort"], "high");
    assert_eq!(detail["configured_thinking_effort"], "high");
    reasoning_support(&pool, Some(false)).await;
    let mut denied = false;
    while let Some(event) = stream.next().await {
        if let Err(DenError::ValidationError(_)) = event {
            denied = true;
        }
    }
    assert!(
        denied,
        "lazy inference must fail before opening upstream transport"
    );
    assert!(
        run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
            .await
            .is_err()
    );
    reasoning_support(&pool, None).await;
    assert!(
        run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
            .await
            .is_err()
    );
    reasoning_support(&pool, Some(true)).await;
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-5'"
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        run_agent_step_stream(&llm, &session, Some(step_context(&pool, &session)))
            .await
            .is_err()
    );
}
