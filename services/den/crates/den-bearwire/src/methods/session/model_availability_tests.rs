use super::*;
use den_runtime::bearwire_events;

async fn snapshot(pool: &sqlx::PgPool, fixture: &Fixture) -> Value {
    json!({
        "model": format!("{:?}", persistence::get_conversation_model_state(pool, fixture.canonical.unwrap()).await.unwrap()),
        "session": client_sessions::find_for_user_bear_session_id(pool, fixture.user, fixture.bear.as_uuid(), &fixture.session).await.unwrap(),
        "events": format!("{:?}", bearwire_events::list_bearwire_events_after(pool, &fixture.session, None, 100).await.unwrap()),
        "messages": format!("{:?}", persistence::list_messages_page(pool, fixture.canonical.unwrap(), None, 100).await.unwrap()),
    })
}

async fn pin(fixture: &Fixture) -> Value {
    rpc_value(
        fixture.state.clone(),
        &fixture.token,
        "session.model.set",
        json!({
            "bear_slug": fixture.slug, "session_id": fixture.session,
            "selection_mode": "explicit", "model": "gpt-5",
        }),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn acp_model_availability_filters_fresh_choices_and_denies_absent_pin_without_writes(
    pool: sqlx::PgPool,
) {
    let fixture = Fixture::new(&pool, false).await;
    let offered = fixture.model().await;
    assert!(offered["model_options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|option| option["handle"] == "openai/gpt-5"));
    // A recently verified positive result cannot authorize a later selection.
    fixture.catalog.set_models(&["openai/gpt-4.1"]);
    let current = fixture.model().await;
    assert!(!current["model_options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|option| option["handle"] == "openai/gpt-5"));
    let before = snapshot(&pool, &fixture).await;
    let denied = pin(&fixture).await;
    assert_eq!(
        denied["error"]["data"]["error_code"], "model_missing",
        "{denied}"
    );
    assert_eq!(denied["error"]["data"]["unavailable_model"], "openai/gpt-5");
    assert_eq!(snapshot(&pool, &fixture).await, before);
    assert!(fixture.catalog.request_count() >= 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn acp_model_availability_keeps_stored_pin_inspectable_and_clearable_during_failures(
    pool: sqlx::PgPool,
) {
    for (status, code) in [
        (401, "virtual_key_rejected"),
        (403, "virtual_key_rejected"),
        (503, "catalog_unavailable"),
        (0, "virtual_key_missing"),
    ] {
        let fixture = Fixture::new(&pool, false).await;
        let selected = pin(&fixture).await;
        assert!(selected.get("error").is_none(), "{selected}");
        if status == 0 {
            bears_db::set_bear_bifrost_virtual_key(
                &pool,
                fixture.bear.as_uuid(),
                None,
                None,
                None,
                &fixture.state.config.den_secret_encryption_key,
            )
            .await
            .unwrap();
        } else {
            fixture.catalog.set_status(status);
        }
        let before = snapshot(&pool, &fixture).await;
        let denied = pin(&fixture).await;
        assert_eq!(denied["error"]["data"]["error_code"], code, "{denied}");
        assert_eq!(snapshot(&pool, &fixture).await, before);
        let inspected = fixture.model().await;
        assert_eq!(inspected["selection_mode"], "explicit");
        assert_eq!(inspected["requested_model"], "openai/gpt-5");
        assert_eq!(inspected["selected_model"], "openai/gpt-5");
        assert_eq!(inspected["model_available"], false);
        assert_eq!(inspected["model_availability"]["error_code"], code);
        assert!(inspected["model_options"].as_array().unwrap().is_empty());
        for response in [&denied, &inspected] {
            let text = response.to_string();
            assert!(!text.contains("PRIVATE_PROVIDER_RESPONSE"));
            assert!(!text.contains("private.test"));
            assert!(!text.contains("SECRET"));
            assert!(!text.contains("sk-bf-bearwire-test"));
        }
        let cleared = rpc_value(
            fixture.state.clone(),
            &fixture.token,
            "session.model.set",
            json!({
                "bear_slug": fixture.slug, "session_id": fixture.session, "selection_mode": "auto",
            }),
        )
        .await;
        assert!(cleared.get("error").is_none(), "{cleared}");
        assert_eq!(cleared["result"]["selection_mode"], "auto");
        assert!(cleared["result"]["requested_model"].is_null());
        assert_eq!(cleared["result"]["source"], "deployment_default");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn acp_model_availability_live_removal_preserves_pin_identity_without_substitution(
    pool: sqlx::PgPool,
) {
    let fixture = Fixture::new(&pool, false).await;
    assert!(pin(&fixture).await.get("error").is_none());
    fixture.catalog.set_models(&["openai/gpt-4.1"]);
    let before = snapshot(&pool, &fixture).await;
    let inspected = fixture.model().await;
    assert_eq!(inspected["effective_model"], "openai/gpt-5");
    assert_eq!(inspected["source"], "conversation_pin");
    assert_eq!(inspected["model_available"], false);
    assert_eq!(
        inspected["model_availability"]["error_code"],
        "model_missing"
    );
    assert_eq!(snapshot(&pool, &fixture).await, before);
}
