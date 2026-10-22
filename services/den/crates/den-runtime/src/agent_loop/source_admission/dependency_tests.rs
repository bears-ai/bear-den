use super::*;
use crate::{agent_loop::AgentLoopSessionStore, llm::ChatMessage};
use den_core::ArmatureAvailability;
use den_memory::personal_facts::PersonalFactId;
use sqlx::postgres::PgPoolOptions;

const ORDINARY_ORIGINS: [TurnExecutionOrigin; 6] = [
    TurnExecutionOrigin::ChannelConversation,
    TurnExecutionOrigin::BrowserTaskSession,
    TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
    TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
    TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
];

#[tokio::test]
async fn every_ordinary_origin_denies_protected_session_before_pool_access() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;
    for origin in ORDINARY_ORIGINS {
        let mut session = tests::test_session(Uuid::new_v4(), 1, "conversation", "client", "run");
        session.origin = origin;
        session.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, session.governance).context_label;
        session.work_run_id =
            matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_)).then(Uuid::new_v4);
        session.context_dependencies = ContextDependencies::personal_fact(
            session.bear_id.into(),
            PersonalFactId(Uuid::new_v4()),
        );
        let source = OrdinarySessionSource::from(&session);
        assert!(std::ptr::eq(
            std::ptr::from_ref(source.context_dependencies),
            std::ptr::from_ref(&session.context_dependencies)
        ));
        let error = require_ordinary_session_source(&pool, source)
            .await
            .unwrap_err();
        assert!(matches!(error, DenError::Authorization(_)));
        assert_eq!(
            error.to_string(),
            session
                .context_dependencies
                .require_unrestricted_reuse()
                .unwrap_err()
                .to_string()
        );

        // The closed pool is actually reached for the same ordinary source
        // without protection; an unrelated authorization failure cannot pass.
        session.context_dependencies = ContextDependencies::default();
        assert!(matches!(
            require_ordinary_session_source(&pool, (&session).into()).await,
            Err(DenError::DatabaseUnavailable(_))
        ));
    }
}

#[test]
fn session_clones_and_message_replacement_retain_dependencies_without_exposing_them() {
    let mut session = tests::test_session(Uuid::new_v4(), 1, "conversation", "client", "run");
    assert!(session.context_dependencies.is_empty());
    session.messages.push(ChatMessage {
        role: "assistant".into(),
        content: Some("context that will be replaced".into()),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    let fact = PersonalFactId(Uuid::new_v4());
    session
        .context_dependencies
        .union_from(&ContextDependencies::personal_fact(
            session.bear_id.into(),
            fact,
        ));
    let expected = session.context_dependencies.clone();
    let store = AgentLoopSessionStore::default();
    store.insert(session.clone());
    store.update(&session.session_key, |stored| {
        stored.messages.clear();
        stored.step += 1;
    });
    let continued = store.get(&session.session_key).unwrap();
    assert!(continued.messages.is_empty());
    assert_eq!(continued.context_dependencies, expected);

    assert_eq!(
        OrdinarySessionSource::from(&continued).context_dependencies,
        &expected
    );
    assert!(continued
        .context_dependencies
        .require_unrestricted_reuse()
        .is_err());
    let snapshot = continued.session_info_runtime_snapshot();
    assert!(snapshot.get("context_dependencies").is_none());
    assert!(snapshot.get("den_context_dependencies").is_none());
    assert!(!snapshot.to_string().contains(&fact.0.to_string()));
    assert!(!format!("{continued:?}").contains(&fact.0.to_string()));
}
