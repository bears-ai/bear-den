use super::*;
use crate::agent_loop::source_admission::tests::test_session;
use den_memory::personal_facts::PersonalFactId;
use den_service::conversation::dependencies::ContextDependencies;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn direct_overflow_entry_denies_protected_context_before_compaction_in_every_mode() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;
    let mut session = test_session(Uuid::new_v4(), 1, "conversation", "client", "run");
    session.context_dependencies =
        ContextDependencies::personal_fact(session.bear_id.into(), PersonalFactId(Uuid::new_v4()));
    session.messages.push(ChatMessage {
        role: "system".into(),
        content: Some("existing context".into()),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    let expected_dependencies = session.context_dependencies.clone();
    let expected_messages = serde_json::to_value(&session.messages).unwrap();
    let mut config = Config::test_stub();
    for mode in ["off", "observe", "active"] {
        config.compaction_mode = mode.into();
        let error = compact_session_messages_for_overflow(&pool, &config, &session)
            .await
            .unwrap_err();
        assert!(matches!(error, DenError::Authorization(_)));
        assert_eq!(
            error.to_string(),
            expected_dependencies
                .require_unrestricted_reuse()
                .unwrap_err()
                .to_string()
        );
        assert_eq!(session.context_dependencies, expected_dependencies);
        assert_eq!(
            serde_json::to_value(&session.messages).unwrap(),
            expected_messages
        );
    }

    // Off mode still preserves ordinary unannotated sessions without any I/O.
    session.context_dependencies = ContextDependencies::default();
    config.compaction_mode = "off".into();
    let (messages, recovered) = compact_session_messages_for_overflow(&pool, &config, &session)
        .await
        .unwrap();
    assert!(!recovered);
    assert_eq!(serde_json::to_value(messages).unwrap(), expected_messages);
}
