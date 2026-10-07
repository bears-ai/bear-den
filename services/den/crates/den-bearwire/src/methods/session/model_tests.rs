use super::*;
use den_core::ThinkingEffort;
use den_service::{
    bears::{hats, model_configurations as configurations},
    conversation::persistence,
};

struct Fixture {
    bear: BearId,
    user: i32,
    slug: String,
    token: String,
    session: String,
    state: DenState,
    canonical: Option<Uuid>,
}

impl Fixture {
    async fn new(pool: &sqlx::PgPool, pending: bool) -> Self {
        let user = create_test_user(pool).await;
        let (bear, slug) = if pending {
            create_test_bear_without_hats(pool).await
        } else {
            create_test_bear(pool).await
        };
        let token = create_token_for_bear(pool, user, bear).await;
        let state = test_state(pool.clone());
        let session = format!("model-display-{}", Uuid::new_v4().simple());
        let opened = rpc_value(
            state.clone(),
            &token,
            "session.open",
            json!({
                "bear_slug": slug, "session_id": session,
            }),
        )
        .await;
        assert_eq!(opened["result"]["ok"], true, "{opened}");
        let canonical = match opened["result"]["session"]["history_conversation_id"].as_str() {
            Some(external) => Some(
                persistence::get_conversation_for_external_id(pool, bear, external)
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
            ),
            None => None,
        };
        assert_eq!(canonical.is_none(), pending);
        Self {
            bear: bear.into(),
            user,
            slug,
            token,
            session,
            state,
            canonical,
        }
    }

    fn params(&self) -> Value {
        json!({"bear_slug": self.slug, "session_id": self.session})
    }

    async fn model(&self) -> Value {
        let result = rpc_value(
            self.state.clone(),
            &self.token,
            "session.model.get",
            self.params(),
        )
        .await;
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(result["result"]["ok"], true, "{result}");
        result["result"].clone()
    }

    async fn hat(&self, pool: &sqlx::PgPool) -> den_core::ids::HatId {
        hats::bindings::conversation_hat(pool, self.bear, self.canonical.unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_display_uses_current_bear_and_hat_configs_not_auto_diagnostics(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool, false).await;
    let careful = configurations::create(
        &pool,
        fixture.bear,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_default(&pool, fixture.bear, Some(careful.id))
        .await
        .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.canonical.unwrap(),
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let inherited = fixture.model().await;
    assert_eq!(inherited["effective_model"], "openai/gpt-5");
    assert_eq!(inherited["source"], "bear_default");
    assert_eq!(inherited["configuration_id"], careful.id.to_string());
    assert_eq!(inherited["configuration_name"], "Careful");
    assert_eq!(inherited["thinking_effort"], "high");
    assert_eq!(inherited["selected_model"], "gpt-4.1");
    assert!(inherited["model_resolution_error"].is_null());

    let quick = configurations::create(&pool, fixture.bear, "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_hat_override(
        &pool,
        fixture.bear,
        fixture.hat(&pool).await,
        Some(quick.id),
    )
    .await
    .unwrap();
    let overridden = fixture.model().await;
    assert_eq!(overridden["effective_model"], "openai/gpt-4.1");
    assert_eq!(overridden["source"], "hat_override");
    assert_eq!(overridden["configuration_id"], quick.id.to_string());
    assert!(overridden["thinking_effort"].is_null());
    configurations::update(
        &pool,
        fixture.bear,
        quick.id,
        "Updated",
        "gpt-5",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    let updated = fixture.model().await;
    assert_eq!(updated["effective_model"], "openai/gpt-5");
    assert_eq!(updated["configuration_name"], "Updated");
    assert_eq!(updated["thinking_effort"], "medium");
    let diagnostic = persistence::get_conversation_model_state(&pool, fixture.canonical.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(diagnostic.selected_model.as_deref(), Some("gpt-4.1"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_set_display_pin_and_clear_agree_with_canonical_inheritance(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool, false).await;
    let careful = configurations::create(
        &pool,
        fixture.bear,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(
        &pool,
        fixture.bear,
        fixture.hat(&pool).await,
        Some(careful.id),
    )
    .await
    .unwrap();
    for (mode, model, expected_model, expected_source) in [
        (
            "explicit",
            Some("gpt-4.1"),
            "openai/gpt-4.1",
            "conversation_pin",
        ),
        ("auto", None, "openai/gpt-5", "hat_override"),
    ] {
        let changed = rpc_value(
            fixture.state.clone(),
            &fixture.token,
            "session.model.set",
            json!({
                "bear_slug": fixture.slug, "session_id": fixture.session,
                "selection_mode": mode, "model": model,
            }),
        )
        .await;
        assert!(changed.get("error").is_none(), "{changed}");
        assert_eq!(changed["result"]["effective_model"], expected_model);
        assert_eq!(changed["result"]["source"], expected_source);
        let inspected = fixture.model().await;
        assert_eq!(
            inspected["effective_model"],
            changed["result"]["effective_model"]
        );
        assert_eq!(inspected["source"], changed["result"]["source"]);
        if mode == "explicit" {
            assert!(inspected["configuration_id"].is_null());
            assert!(inspected["configuration_name"].is_null());
            assert!(inspected["thinking_effort"].is_null());
        } else {
            assert_eq!(inspected["configuration_id"], careful.id.to_string());
            assert_eq!(inspected["thinking_effort"], "high");
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_model_display_is_null_and_never_falls_back_to_cached_or_default_choice(
    pool: sqlx::PgPool,
) {
    let fixture = Fixture::new(&pool, false).await;
    let careful = configurations::create(&pool, fixture.bear, "Careful", "gpt-5", None)
        .await
        .unwrap();
    configurations::set_hat_override(
        &pool,
        fixture.bear,
        fixture.hat(&pool).await,
        Some(careful.id),
    )
    .await
    .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.canonical.unwrap(),
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-5'"
    )
    .execute(&pool)
    .await
    .unwrap();
    let invalid = fixture.model().await;
    assert!(invalid["effective_model"].is_null());
    assert!(invalid["configuration_id"].is_null());
    assert!(invalid["source"].is_null());
    assert!(invalid["model_resolution_error"]
        .as_str()
        .unwrap()
        .contains("no longer selectable"));
    assert!(invalid["model_options"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model["handle"] != "openai/gpt-5"));
    persistence::set_conversation_model_state(
        &pool,
        fixture.canonical.unwrap(),
        "explicit",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let invalid_pin = fixture.model().await;
    assert!(invalid_pin["effective_model"].is_null());
    assert!(invalid_pin["model_resolution_error"]
        .as_str()
        .unwrap()
        .contains("has no model"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_model_display_previews_inheritance_without_manufacturing_a_hat_source(
    pool: sqlx::PgPool,
) {
    let fixture = Fixture::new(&pool, true).await;
    let deployment = fixture.model().await;
    assert_eq!(deployment["effective_model"], "openai/gpt-4.1");
    assert_eq!(deployment["source"], "deployment_default");
    assert!(deployment["conversation_id"].is_null());
    let careful = configurations::create(
        &pool,
        fixture.bear,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_default(&pool, fixture.bear, Some(careful.id))
        .await
        .unwrap();
    // Installing an IDE default after the pending session was opened does not bind it.
    let hat = hats::create_hat(
        &pool,
        fixture.bear,
        UserId::new(fixture.user),
        "IDE",
        "test",
    )
    .await
    .unwrap();
    hats::set_ide_default_hat(&pool, fixture.bear, hat.id)
        .await
        .unwrap();
    let quick = configurations::create(&pool, fixture.bear, "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_hat_override(&pool, fixture.bear, hat.id, Some(quick.id))
        .await
        .unwrap();
    let inherited = fixture.model().await;
    assert_eq!(inherited["effective_model"], "openai/gpt-5");
    assert_eq!(inherited["source"], "bear_default");
    assert_eq!(inherited["configuration_id"], careful.id.to_string());
    assert_eq!(inherited["thinking_effort"], "high");
    assert_eq!(inherited["access"]["state"], "awaiting_hat");
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-5'"
    )
    .execute(&pool)
    .await
    .unwrap();
    let invalid = fixture.model().await;
    assert!(invalid["effective_model"].is_null());
    assert!(invalid["model_resolution_error"]
        .as_str()
        .unwrap()
        .contains("no longer selectable"));
    assert!(
        persistence::list_conversations_for_bear(&pool, fixture.bear.as_uuid(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    let session = client_sessions::find_for_user_bear_session_id(
        &pool,
        fixture.user,
        fixture.bear.as_uuid(),
        &fixture.session,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(session.resolved_conversation_id.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_model_display_uses_verified_job_hat_and_does_not_fall_back_after_revocation(
    pool: sqlx::PgPool,
) {
    let fixture = Fixture::new(&pool, false).await;
    let quick = configurations::create(&pool, fixture.bear, "Pair", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_default(&pool, fixture.bear, Some(quick.id))
        .await
        .unwrap();
    configurations::set_hat_override(
        &pool,
        fixture.bear,
        fixture.hat(&pool).await,
        Some(quick.id),
    )
    .await
    .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.canonical.unwrap(),
        "explicit",
        Some("gpt-4.1"),
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let run_id = create_checkoutable_work_run(&pool, fixture.user, fixture.bear.as_uuid()).await;
    let checkout =
        checkout_work_run_for_session(&pool, run_id, fixture.bear.as_uuid(), &fixture.session)
            .await
            .unwrap();
    assert!(
        checkout.execution_attempt.is_some(),
        "Work fixture must be admitted"
    );
    let run = den_docket::work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    let work_hat = hats::bindings::job_hat(&pool, fixture.bear, run.job_id)
        .await
        .unwrap()
        .unwrap();
    let careful = configurations::create(
        &pool,
        fixture.bear,
        "Work",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, fixture.bear, work_hat, Some(careful.id))
        .await
        .unwrap();
    let work = fixture.model().await;
    assert_eq!(work["effective_model"], "openai/gpt-5");
    assert_eq!(work["source"], "hat_override");
    assert_eq!(work["configuration_id"], careful.id.to_string());
    assert_eq!(work["thinking_effort"], "high");
    assert_eq!(work["access"]["state"], "executable");
    hats::manage::disable_work(&pool, fixture.bear, work_hat)
        .await
        .unwrap();
    let revoked = fixture.model().await;
    assert_eq!(revoked["access"]["state"], "read_only");
    assert!(revoked["effective_model"].is_null());
    assert!(revoked["model_resolution_error"]
        .as_str()
        .unwrap()
        .contains("verified live source"));
}
