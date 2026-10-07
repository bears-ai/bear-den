use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn closed_pending_session_is_read_only_until_explicit_reopen(pool: sqlx::PgPool) {
    let user = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear_without_hats(&pool).await;
    let token = create_token_for_bear(&pool, user, bear).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Editor",
        "Editor identity",
    )
    .await
    .unwrap();
    let state = test_state(pool.clone());
    let session_id = format!("closed-pending-{}", Uuid::new_v4());
    let params = json!({"bear_slug": slug, "session_id": session_id});
    let opened = rpc_value(state.clone(), &token, "session.open", params.clone()).await;
    assert_access(&opened["result"]["session"], "awaiting_hat", true);
    let closed = rpc_value(state.clone(), &token, "session.close", params.clone()).await;
    assert_eq!(closed["result"]["closed"], true, "{closed}");
    assert_eq!(closed["result"]["pair_reflection"]["status"], "skipped");
    let inspected = rpc_value(state.clone(), &token, "session.state", params.clone()).await;
    assert_access(&inspected["result"]["session"], "read_only", false);
    assert!(inspected["result"]["session"]["history_conversation_id"].is_null());
    assert!(inspected["result"]["session"]["resolved_conversation_id"].is_null());
    let listed = rpc_value(
        state.clone(),
        &token,
        "session.state",
        json!({
            "bear_slug": slug, "include_closed": true,
        }),
    )
    .await;
    assert_access(&listed["result"]["sessions"][0], "read_only", false);
    let model = rpc_value(state.clone(), &token, "session.model.get", params.clone()).await;
    assert_eq!(
        model["result"]["access"],
        json!({"state": "read_only", "may_select_hat": false})
    );
    assert!(model["result"]["conversation_id"].is_null());
    let denied = rpc_value(
        state.clone(),
        &token,
        "session.hat.select",
        json!({
            "bear_slug": slug, "session_id": session_id, "hat_id": hat.id,
        }),
    )
    .await;
    assert_authorization_error(
        &denied,
        "session.hat.select",
        "a hat can only be selected for your live IDE source before its first turn",
    );
    let run = rpc_value(
        state.clone(),
        &token,
        "run.start",
        json!({
            "bear_slug": slug, "session_id": session_id, "prompt": "closed source must not run",
        }),
    )
    .await;
    assert_authorization_error(&run, "run.start", "session is not live");
    assert!(conversations(&pool, bear).await.is_empty());
    let reopened = rpc_value(state.clone(), &token, "session.open", params).await;
    assert_access(&reopened["result"]["session"], "awaiting_hat", true);
    assert_eq!(
        reopened["result"]["session"]["conversation_id"],
        opened["result"]["session"]["conversation_id"]
    );
    assert!(conversations(&pool, bear).await.is_empty());
    let selected = rpc_value(
        state,
        &token,
        "session.hat.select",
        json!({
            "bear_slug": slug, "session_id": session_id, "hat_id": hat.id,
        }),
    )
    .await;
    assert_access(&selected["result"]["session"], "executable", true);
    assert_eq!(conversations(&pool, bear).await.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn missing_explicit_history_never_falls_back_to_a_new_selection(pool: sqlx::PgPool) {
    let user = create_test_user(&pool).await;
    let (bear, slug) = create_test_bear(&pool).await;
    let token = create_token_for_bear(&pool, user, bear).await;
    let state = test_state(pool.clone());
    let session_id = format!("missing-history-{}", Uuid::new_v4());
    for requested in [
        format!("den-conv-{}", Uuid::new_v4().simple()),
        "missing-history".into(),
        "new-".into(),
    ] {
        for method in ["session.open", "session.resume"] {
            let denied = rpc_value(
                state.clone(),
                &token,
                method,
                json!({
                    "bear_slug": slug, "session_id": session_id, "conversation_id": requested,
                }),
            )
            .await;
            assert_eq!(denied["error"]["code"], -32001, "{denied}");
            assert_eq!(
                denied["error"]["data"]["error"], "Not Found: conversation not found",
                "{denied}"
            );
            assert!(denied.get("result").is_none(), "{denied}");
            assert!(
                client_sessions::find_for_user_bear_session_id(&pool, user, bear, &session_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(conversations(&pool, bear).await.is_empty());
        }
    }
    // An explicit provisional target is still a valid request for a fresh source.
    let pending = format!("new-acp-zed-{}", Uuid::new_v4().simple());
    let opened = rpc_value(
        state,
        &token,
        "session.open",
        json!({
            "bear_slug": slug, "session_id": session_id, "conversation_id": pending,
        }),
    )
    .await;
    assert_access(&opened["result"]["session"], "executable", true);
    assert_eq!(opened["result"]["session"]["conversation_id"], pending);
    assert!(opened["result"]["session"]["history_conversation_id"]
        .as_str()
        .unwrap()
        .starts_with("den-conv-"));
    assert_eq!(conversations(&pool, bear).await.len(), 1);
}
