use super::*;
use den_service::{
    bears::db::{self, BearParams, BEAR_ROLE_MEMBER},
    conversation::persistence::ensure_conversation_for_external_id,
};
use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn diagnostics_only_returns_transitions_for_the_viewers_bear_and_session(pool: PgPool) {
    let suffix = Uuid::new_v4().simple().to_string();
    async fn user(pool: &PgPool, name: &str) -> i32 {
        sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
            format!("{name}@example.test"),
            name,
            name,
            "unused",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }
    async fn bear(pool: &PgPool, slug: &str) -> Uuid {
        db::create_bear(
            pool,
            BearParams {
                slug,
                name: "Diagnostics test Bear",
                description: "test",
                system_prompt: "test",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap()
    }
    async fn session(pool: &PgPool, user_id: i32, bear_id: Uuid, slug: &str, id: &str) {
        let conversation_id = format!("conv-{}", Uuid::new_v4().simple());
        ensure_conversation_for_external_id(
            pool,
            bear_id,
            Some(user_id),
            &conversation_id,
            None,
            None,
        )
        .await
        .unwrap();
        client_sessions::upsert_session(
            pool,
            client_sessions::UpsertClientSession {
                user_id,
                bear_id,
                bear_slug: slug.to_string(),
                client_session_id: id.to_string(),
                runtime_session_id: format!("runtime-{user_id}-{bear_id}-{id}"),
                conversation_id,
                resolved_conversation_id: None,
                client: "diagnostics-test".to_string(),
                cwd: None,
                current_mode: None,
            },
        )
        .await
        .unwrap();
    }
    async fn append(pool: &PgPool, bear_id: Uuid, user_id: Option<i32>, id: &str, label: &str) {
        let transition = FocusedExecutionTransition {
            state_version: 1,
            from: None,
            to: FocusedExecutionState::Selected,
            reason: FocusedExecutionTransitionReason::Reconciled,
            correlation_id: label.to_string(),
            causation_id: None,
            session_id: id.to_string(),
            task_id: None,
            run_id: None,
            attempt_id: None,
            fence_epoch: None,
            open_obligations: 0,
            task_selection_preserved: true,
        };
        bearwire_events::append_bearwire_event(
            pool,
            id,
            Some(bear_id),
            user_id,
            BearWireEvent::persistent_typed(
                bearwire_protocol::lifecycle::FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
                transition,
            ),
        )
        .await
        .unwrap();
    }

    let owner = user(&pool, &format!("o{}", &suffix[..16])).await;
    let other = user(&pool, &format!("x{}", &suffix[..16])).await;
    let slug = format!("diagnostics-{suffix}");
    let other_slug = format!("other-diagnostics-{suffix}");
    let bear_id = bear(&pool, &slug).await;
    let other_bear_id = bear(&pool, &other_slug).await;
    let session_id = format!("shared-{suffix}");
    // These rows represent historical collisions predating the ownership guard.
    sqlx::query!("ALTER TABLE client_sessions DISABLE TRIGGER client_sessions_global_owner_guard")
        .execute(&pool)
        .await
        .unwrap();
    for (user_id, id, bear_slug) in [
        (owner, bear_id, slug.as_str()),
        (other, bear_id, slug.as_str()),
        (owner, other_bear_id, other_slug.as_str()),
    ] {
        db::grant_membership(&pool, user_id, id, Some(BEAR_ROLE_MEMBER))
            .await
            .unwrap();
        session(&pool, user_id, id, bear_slug, &session_id).await;
    }
    sqlx::query!("ALTER TABLE client_sessions ENABLE TRIGGER client_sessions_global_owner_guard")
        .execute(&pool)
        .await
        .unwrap();

    append(&pool, bear_id, Some(owner), &session_id, "owner.first").await;
    append(&pool, bear_id, Some(other), &session_id, "other").await;
    append(&pool, other_bear_id, Some(owner), &session_id, "other.bear").await;
    append(&pool, bear_id, None, &session_id, "legacy").await;
    append(&pool, bear_id, Some(owner), &session_id, "owner.second").await;
    append(&pool, bear_id, Some(other), &session_id, "other.latest").await;

    let config = std::sync::Arc::new(den_core::config::Config::test_stub());
    let state = DenState::new(
        pool,
        config.clone(),
        std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
        den_memory::MemoryStoreManager::new(config.as_ref()),
    );
    let owner_page = focused_execution_diagnostics(&state, owner, bear_id, &session_id, 1)
        .await
        .unwrap();
    assert!(owner_page.history_truncated);
    assert_eq!(
        owner_page.transitions[0].transition.correlation_id,
        "owner.second"
    );
    let owner_all = focused_execution_diagnostics(&state, owner, bear_id, &session_id, 10)
        .await
        .unwrap();
    assert!(!owner_all.history_truncated);
    assert_eq!(
        owner_all
            .transitions
            .iter()
            .map(|record| record.transition.correlation_id.as_str())
            .collect::<Vec<_>>(),
        vec!["owner.first", "owner.second"]
    );
    let other_page = focused_execution_diagnostics(&state, other, bear_id, &session_id, 10)
        .await
        .unwrap();
    assert_eq!(
        other_page
            .transitions
            .iter()
            .map(|record| record.transition.correlation_id.as_str())
            .collect::<Vec<_>>(),
        vec!["other", "other.latest"]
    );
    let different_bear =
        focused_execution_diagnostics(&state, owner, other_bear_id, &session_id, 10)
            .await
            .unwrap();
    assert_eq!(
        different_bear.transitions[0].transition.correlation_id,
        "other.bear"
    );
    assert_eq!(different_bear.transitions.len(), 1);
}
