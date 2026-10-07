use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn history_inspection_never_grants_configuration_or_hat_mutation(pool: sqlx::PgPool) {
    let owner = create_test_user(&pool).await;
    let admin = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear(&pool).await;
    let owner_token = create_member_token(&pool, owner, bear).await;
    let admin_token = create_token_for_bear(&pool, admin, bear).await;
    let hat = hats::ide_default_hat(&pool, BearId::new(bear))
        .await
        .unwrap()
        .unwrap();
    let inference = InferenceFixture::start();
    let mut config = den_core::config::Config::test_stub();
    config.den_secret_encryption_key = "bearwire-test-secret-key".into();
    config.default_llm_model = "openai/bearwire-test-model".into();
    config.llm_api_url = inference.url.clone();
    bears_db::set_bear_bifrost_virtual_key(
        &pool,
        bear,
        Some("vk-test"),
        Some("History source test"),
        Some("sk-bf-bearwire-test"),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    let state = test_state_with_config(pool.clone(), config);
    let owner_executable = ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(owner),
        &format!("den-conv-{}", Uuid::new_v4().simple()),
        None,
        None,
    )
    .await
    .unwrap();
    let admin_executable = ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        &format!("den-conv-{}", Uuid::new_v4().simple()),
        None,
        None,
    )
    .await
    .unwrap();
    for target in [&owner_executable, &admin_executable] {
        bindings::bind_conversation_hat(&pool, BearId::new(bear), target.id, hat)
            .await
            .unwrap();
    }
    for (kind, canonical_owner, token) in [
        ("unbound", Some(owner), &owner_token),
        ("other-owner", Some(owner), &admin_token),
        ("null-owner", None, &admin_token),
        ("archived-status", Some(owner), &owner_token),
        ("archived-marker", Some(owner), &owner_token),
        ("pending-shaped-history", Some(owner), &owner_token),
        ("closed", Some(owner), &owner_token),
    ] {
        let external = if kind == "pending-shaped-history" {
            format!("new-acp-history-{}", Uuid::new_v4())
        } else {
            format!("den-conv-{}", Uuid::new_v4().simple())
        };
        let canonical = ensure_conversation_for_external_id(
            &pool,
            bear,
            canonical_owner,
            &external,
            None,
            None,
        )
        .await
        .unwrap();
        if kind != "unbound" && kind != "pending-shaped-history" {
            bindings::bind_conversation_hat(&pool, BearId::new(bear), canonical.id, hat)
                .await
                .unwrap();
        }
        if kind == "archived-status" {
            sqlx::query!(
                "UPDATE conversations SET status = 'archived' WHERE id = $1",
                canonical.id
            )
            .execute(&pool)
            .await
            .unwrap();
        } else if kind == "archived-marker" {
            den_service::archived_conversations::set_archived(
                &pool,
                bear,
                &external,
                Some(owner),
                "test",
                true,
            )
            .await
            .unwrap();
        }
        let original_hat = bindings::conversation_hat(&pool, BearId::new(bear), canonical.id)
            .await
            .unwrap();
        let original = persistence::get_conversation_for_external_id(&pool, bear, &external)
            .await
            .unwrap()
            .unwrap();
        let session_id = format!("history-{kind}-{}", Uuid::new_v4());
        let params =
            json!({"bear_slug": slug, "session_id": session_id, "conversation_id": external});
        let opened = rpc_value(state.clone(), token, "session.open", params.clone()).await;
        assert_eq!(opened["result"]["ok"], true, "{kind}: {opened}");
        if kind == "closed" {
            assert_access(&opened["result"]["session"], "executable", true);
            let session =
                client_sessions::find_for_user_bear_session_id(&pool, owner, bear, &session_id)
                    .await
                    .unwrap()
                    .unwrap();
            client_sessions::mark_closed(&pool, session.id)
                .await
                .unwrap();
            let closed = rpc_value(state.clone(), token, "session.state", params.clone()).await;
            assert_access(&closed["result"]["session"], "read_only", false);
        } else {
            assert_access(&opened["result"]["session"], "read_only", false);
        }
        assert_eq!(
            opened["result"]["session"]["history_conversation_id"],
            external
        );
        for method in [
            "session.state",
            "session.model.get",
            "hats.list",
            "session.open",
            "conversation.history",
        ] {
            if kind == "closed" && method == "session.open" {
                continue;
            }
            let inspected = rpc_value(state.clone(), token, method, params.clone()).await;
            assert!(
                inspected.get("error").is_none(),
                "{kind} {method}: {inspected}"
            );
        }
        let actor = if kind == "other-owner" || kind == "null-owner" {
            admin
        } else {
            owner
        };
        let before =
            client_sessions::find_for_user_bear_session_id(&pool, actor, bear, &session_id)
                .await
                .unwrap()
                .unwrap();
        for method in [
            "session.model.set",
            "session.compact",
            "session.hat.select",
            "run.start",
        ] {
            let denied = rpc_value(state.clone(), token, method,
                json!({"bear_slug": slug, "session_id": session_id, "selection_mode": "auto", "hat_id": hat, "prompt": "must not persist"})).await;
            let reason = match (method, kind) {
                ("session.hat.select", _) => {
                    "a hat can only be selected for your live IDE source before its first turn"
                }
                (_, "unbound" | "pending-shaped-history") => {
                    "a named hat is required; start a conversation or Job explicitly bound to a hat"
                }
                (_, "archived-marker") => "archived history is read-only",
                (_, "closed") => "session is not live",
                ("run.start", _) => "run startup requires a live owned transcript",
                _ => "session configuration requires its live owned conversation",
            };
            assert_authorization_error(&denied, method, reason);
            assert_eq!(
                inference.request_count(),
                0,
                "{kind} {method} reached provider preflight/inference"
            );
        }
        let executable_target = if actor == admin {
            &admin_executable
        } else {
            &owner_executable
        };
        let targets = [
            executable_target.external_conversation_id.clone().unwrap(),
            format!("new-acp-rebinding-{}", Uuid::new_v4().simple()),
        ];
        let source_count = conversations(&pool, bear).await.len();
        for target in targets {
            for method in ["session.open", "session.resume", "run.start"] {
                let denied = rpc_value(
                    state.clone(),
                    token,
                    method,
                    json!({
                        "bear_slug": slug, "session_id": session_id, "conversation_id": target,
                        "prompt": "must not rebind read-only history", "cwd": "/must-not-change",
                    }),
                )
                .await;
                let reason = if method != "run.start" {
                    "reconnect cannot change the canonical session conversation"
                } else if kind == "closed" {
                    "session is not live"
                } else {
                    "run startup cannot change the canonical session conversation"
                };
                assert_authorization_error(&denied, method, reason);
                assert_eq!(
                    inference.request_count(),
                    0,
                    "{kind} {method} rebind reached inference preflight"
                );
                assert_eq!(
                    conversations(&pool, bear).await.len(),
                    source_count,
                    "{kind} {method} created a replacement source"
                );
            }
        }
        assert!(
            persistence::get_conversation_model_state(&pool, executable_target.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            persistence::list_messages_page(&pool, executable_target.id, None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            persistence::get_conversation_model_state(&pool, canonical.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            persistence::list_messages_page(&pool, canonical.id, None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            bindings::conversation_hat(&pool, BearId::new(bear), canonical.id)
                .await
                .unwrap(),
            original_hat
        );
        let after = persistence::get_conversation_for_external_id(&pool, bear, &external)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.source_client_session_id,
            original.source_client_session_id
        );
        assert_eq!(after.updated_at, original.updated_at);
        let after_session =
            client_sessions::find_for_user_bear_session_id(&pool, actor, bear, &session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(after_session.updated_at, before.updated_at);
        assert_eq!(after_session.closed_at, before.closed_at);
        assert_eq!(after_session.archived_at, before.archived_at);
        assert_eq!(after_session.cwd, before.cwd);
        assert_eq!(
            den_service::archived_conversations::list_for_bear(&pool, bear)
                .await
                .unwrap()
                .contains(&external),
            kind == "archived-marker"
        );
        assert_eq!(after_session.conversation_id, before.conversation_id);
        assert_eq!(
            after_session.resolved_conversation_id,
            before.resolved_conversation_id
        );
        assert_eq!(
            sqlx::query_scalar!(
                "SELECT count(*) AS \"count!\" FROM turn_runs WHERE session_id = $1",
                session_id,
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
    }
    // The same real key, catalog and provider fixture must admit a live owned
    // source, proving the denials above cannot be unrelated inference failures.
    let canonical = owner_executable;
    let external = canonical.external_conversation_id.as_ref().unwrap().clone();
    let session_id = format!("live-owned-{}", Uuid::new_v4());
    let opened = rpc_value(
        state.clone(),
        &owner_token,
        "session.open",
        json!({
            "bear_slug": slug, "session_id": session_id, "conversation_id": external,
        }),
    )
    .await;
    assert_access(&opened["result"]["session"], "executable", true);
    let configured = rpc_value(
        state.clone(),
        &owner_token,
        "session.model.set",
        json!({
            "bear_slug": slug, "session_id": session_id, "selection_mode": "auto",
        }),
    )
    .await;
    assert_eq!(configured["result"]["ok"], true, "{configured}");
    assert_eq!(inference.request_count(), 0);
    let started = rpc_value(
        state,
        &owner_token,
        "run.start",
        json!({
            "bear_slug": slug, "session_id": session_id, "prompt": "live owned source",
        }),
    )
    .await;
    assert_eq!(started["result"]["accepted"], true, "{started}");
    wait_for_completed_run(&pool, started["result"]["run_id"].as_str().unwrap()).await;
    wait_for_user_message(&pool, bear, &external, "live owned source").await;
    assert_eq!(inference.completion_count(), 1);
    assert!(inference.request_count() >= 1);
    assert!(
        persistence::get_conversation_model_state(&pool, canonical.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(bear), canonical.id)
            .await
            .unwrap(),
        Some(hat)
    );
}
