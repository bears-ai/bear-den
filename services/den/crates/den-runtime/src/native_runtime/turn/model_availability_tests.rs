use super::*;
use crate::{
    agent_loop::source_admission::tests::fixture,
    primary_model::transport_tests::{config_with_key, CatalogMock},
};
use den_core::ModelAvailabilityFailureKind;
use den_service::bears::model_configurations as configurations;

#[sqlx::test(migrations = "../../migrations")]
async fn den_metadata_without_bifrost_membership_fails_before_recall_inference_and_persistence(
    pool: PgPool,
) {
    let (source, canonical, hat) = fixture(&pool).await;
    let bear_id = source.bear_id.into();
    // Den metadata accepts this model; only the authenticated live gateway can
    // decide whether this Bear can actually execute it.
    let selected = configurations::create(&pool, bear_id, "Unavailable primary", "gpt-5", None)
        .await
        .unwrap();
    configurations::set_hat_override(&pool, bear_id, hat, Some(selected.id))
        .await
        .unwrap();
    let mock = CatalogMock::new(serde_json::json!({"data": [{"id": "openai/gpt-4.1"}]}));
    let mut config = config_with_key(&pool, bear_id, &mock).await;
    config.qdrant_url = Some(config.llm_api_url.clone());
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("primary-preflight-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    let binding = RoleRuntimeBinding {
        binding_id: NativeTurnSource::Conversation(canonical).binding_id(bear_id),
        compatibility_backend: Some("native".into()),
    };
    let result = start_native_turn_event_stream(
        TurnStartRequest {
            sqlx_pool: &pool,
            config: &config,
            memory_stores: &stores,
            request_id: Uuid::new_v4(),
            run_id: source.run_id.as_deref(),
            checkpoint_audit_context: None,
            user_id: source.user_id.unwrap(),
            session_id: &source.client_session_id,
            bear_id: source.bear_id,
            bear_slug: &source.bear_slug,
            client: "bear-armature",
            cwd: None,
            workspace_roots: None,
            binding: &binding,
            conversation_selection: &source.conversation_id,
            upstream_target: &source.conversation_id,
            prompt: "Recall what we learned and begin the next task",
            prompt_context: None,
            client_tools: None,
            runtime_context: None,
            runtime_context_len: 0,
            technical_budget_recovery_start_payload: None,
            stream_tokens: true,
            // Adapter hints must not authorize the wrong/lower-precedence model.
            api_style: Some(crate::llm::LlmApiStyle::ChatCompletionsStream),
            supports_reasoning_effort: None,
        },
        source.origin,
    )
    .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("missing primary must fail before a stream starts"),
    };
    let DenError::ModelAvailability(failure) = error else {
        panic!("expected typed model failure")
    };
    assert_eq!(failure.kind, ModelAvailabilityFailureKind::ModelMissing);
    assert_eq!(failure.model.unwrap().as_str(), "openai/gpt-5");
    assert_eq!(
        mock.request_count(),
        1,
        "catalog only: no embedding, recall, or inference request"
    );
    assert!(
        !std::path::Path::new(&config.bear_sqlite_data_dir).exists(),
        "assembly must not open memory stores or prompt providers"
    );
    assert!(
        conversation_persistence::list_messages_page(&pool, canonical, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(crate::bearwire_events::list_bearwire_events_for_run(
        &pool,
        source.run_id.as_deref().unwrap(),
        100
    )
    .await
    .unwrap()
    .is_empty());
    assert!(SESSION_STORE.get(&source.session_key).is_none());
}
