use den_core::{
    ids::{BearId, HatId},
    ThinkingEffort,
};
use den_http::armature_tokens;
use den_service::{
    bears::{
        db as bears_db,
        db::BearParams,
        hats::{self, turn_binding::NativeTurnSource},
        model_configurations as configurations, Bear,
    },
    conversation::{persistence, viewer::ConversationViewer},
    DenState,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::{preflight_pair_run_model, ResolvedRunModel, ResolvedRunModelSource};
use super::{admit, helpers, AdmittedRunSource};

struct OrdinaryFixture {
    bear: Bear,
    hat: HatId,
    session: String,
    source: AdmittedRunSource,
    token: armature_tokens::CreatedArmatureToken,
}

impl OrdinaryFixture {
    async fn new(pool: &PgPool) -> Self {
        let user = helpers::user(pool).await;
        let slug = format!("model-preflight-{}", Uuid::new_v4().simple());
        let bear_id = BearId::new(
            bears_db::create_bear(
                pool,
                BearParams {
                    slug: &slug,
                    name: "Model preflight test",
                    description: "test",
                    system_prompt: "test",
                    default_model: None,
                    tools_enabled: None,
                    context_profile: None,
                },
            )
            .await
            .unwrap(),
        );
        bears_db::grant_membership(
            pool,
            user.get(),
            bear_id.as_uuid(),
            Some(bears_db::BEAR_ROLE_ADMIN),
        )
        .await
        .unwrap();
        let hat = hats::create_hat(pool, bear_id, user, "Pair", "Collaborate")
            .await
            .unwrap()
            .id;
        hats::set_ide_default_hat(pool, bear_id, hat).await.unwrap();
        let token = armature_tokens::create_for_bear(pool, user.get(), bear_id.as_uuid(), "test")
            .await
            .unwrap();
        let session = format!("model-preflight-{}", Uuid::new_v4().simple());
        let viewer = ConversationViewer::resolve(pool, bear_id, user)
            .await
            .unwrap()
            .unwrap();
        let source = admit(
            pool,
            &viewer,
            bear_id,
            user,
            &session,
            &format!("new-acp-{}", Uuid::new_v4().simple()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            source.turn_source,
            NativeTurnSource::Conversation(source.conversation.id)
        );
        // Keep this snapshot across config edits to prove the raw Bear projection
        // cannot mask current canonical configuration state.
        let bear = bears_db::get_bear(pool, bear_id.as_uuid())
            .await
            .unwrap()
            .unwrap();
        Self {
            bear,
            hat,
            session,
            source,
            token,
        }
    }

    fn bear_id(&self) -> BearId {
        self.bear.id.into()
    }

    fn external_id(&self) -> &str {
        self.source
            .conversation
            .external_conversation_id
            .as_deref()
            .unwrap()
    }

    async fn preflight(
        &self,
        state: &DenState,
    ) -> Result<ResolvedRunModel, den_http::errors::CustomError> {
        preflight_pair_run_model(
            state,
            &self.bear,
            &self.session,
            self.external_id(),
            self.source.turn_source,
        )
        .await
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn inherited_config_preflight_uses_deployment_bear_then_verified_hat(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    let state = helpers::model_ready_state_for_bear(
        &pool,
        fixture.bear_id(),
        &["openai/gpt-4.1", "openai/gpt-5"],
    )
    .await;
    let deployment = fixture.preflight(&state).await.unwrap();
    assert_eq!(deployment.handle, "openai/gpt-4.1");
    assert_eq!(deployment.source, ResolvedRunModelSource::SystemDefault);

    let careful = configurations::create(
        &pool,
        fixture.bear_id(),
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(careful.id))
        .await
        .unwrap();
    assert!(fixture.bear.default_model.is_none());
    let inherited = fixture.preflight(&state).await.unwrap();
    assert_eq!(inherited.handle, "openai/gpt-5");
    assert_eq!(inherited.source, ResolvedRunModelSource::BearDefault);

    let quick = configurations::create(&pool, fixture.bear_id(), "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_hat_override(&pool, fixture.bear_id(), fixture.hat, Some(quick.id))
        .await
        .unwrap();
    let overridden = fixture.preflight(&state).await.unwrap();
    assert_eq!(overridden.handle, "openai/gpt-4.1");
    assert_eq!(overridden.source, ResolvedRunModelSource::HatOverride);
    assert_eq!(overridden.source.as_str(), "hat_override");
    assert_eq!(
        overridden.api_style,
        den_llm::LlmApiStyle::ChatCompletionsStream
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_preflight_uses_eligible_job_hat_not_transcript_hat_or_pin(pool: PgPool) {
    let fixture = helpers::Fixture::new(&pool).await;
    let state = helpers::model_ready_state_for_bear(
        &pool,
        fixture.bear,
        &["openai/gpt-4.1", "openai/gpt-5"],
    )
    .await;
    let quick = configurations::create(&pool, fixture.bear, "Pair quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_default(&pool, fixture.bear, Some(quick.id))
        .await
        .unwrap();
    let pair_hat = hats::ide_default_hat(&pool, fixture.bear)
        .await
        .unwrap()
        .unwrap();
    configurations::set_hat_override(&pool, fixture.bear, pair_hat, Some(quick.id))
        .await
        .unwrap();
    let careful = configurations::create(
        &pool,
        fixture.bear,
        "Work careful",
        "gpt-5",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, fixture.bear, fixture.hat, Some(careful.id))
        .await
        .unwrap();
    let external = format!("den-conv-{}", Uuid::new_v4().simple());
    let transcript = persistence::ensure_conversation_for_external_id(
        &pool,
        fixture.bear.as_uuid(),
        Some(fixture.user.get()),
        &external,
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, fixture.bear, transcript.id, pair_hat)
        .await
        .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        transcript.id,
        "explicit",
        Some("gpt-4.1"),
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let viewer = ConversationViewer::resolve(&pool, fixture.bear, fixture.user)
        .await
        .unwrap()
        .unwrap();
    let source = admit(
        &pool,
        &viewer,
        fixture.bear,
        fixture.user,
        &fixture.session,
        &external,
        Some(fixture.expected),
    )
    .await
    .unwrap();
    assert_eq!(
        source.turn_source,
        NativeTurnSource::WorkRun(fixture.expected.work_run_id)
    );
    let bear = bears_db::get_bear(&pool, fixture.bear.as_uuid())
        .await
        .unwrap()
        .unwrap();
    let selected = preflight_pair_run_model(
        &state,
        &bear,
        &fixture.session,
        &external,
        source.turn_source,
    )
    .await
    .unwrap();
    assert_eq!(selected.handle, "openai/gpt-5");
    assert_eq!(selected.source, ResolvedRunModelSource::HatOverride);

    hats::manage::disable_work(&pool, fixture.bear, fixture.hat)
        .await
        .unwrap();
    let error = preflight_pair_run_model(
        &state,
        &bear,
        &fixture.session,
        &external,
        source.turn_source,
    )
    .await
    .err()
    .expect("revoked Work hat must not fall back to the transcript pin");
    assert!(error.to_string().contains("no longer eligible"), "{error}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_pin_wins_and_clearing_it_restores_hat_inheritance(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    let state = helpers::model_ready_state_for_bear(
        &pool,
        fixture.bear_id(),
        &["openai/gpt-4.1", "openai/gpt-5"],
    )
    .await;
    let careful = configurations::create(
        &pool,
        fixture.bear_id(),
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(careful.id))
        .await
        .unwrap();
    configurations::set_hat_override(&pool, fixture.bear_id(), fixture.hat, Some(careful.id))
        .await
        .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "explicit",
        Some("gpt-4.1"),
        None,
        None,
    )
    .await
    .unwrap();
    let pin = fixture.preflight(&state).await.unwrap();
    assert_eq!(pin.handle, "openai/gpt-4.1");
    assert_eq!(pin.source, ResolvedRunModelSource::ConversationExplicit);

    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let inherited = fixture.preflight(&state).await.unwrap();
    assert_eq!(inherited.handle, "openai/gpt-5");
    assert_eq!(inherited.source, ResolvedRunModelSource::HatOverride);
    for invalid_pin in [None, Some("")] {
        persistence::set_conversation_model_state(
            &pool,
            fixture.source.conversation.id,
            "explicit",
            invalid_pin,
            invalid_pin,
            None,
        )
        .await
        .unwrap();
        assert!(
            fixture.preflight(&state).await.is_err(),
            "invalid pin disappeared into inheritance"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_choice_blocks_start_instead_of_using_auto_bear_or_deployment(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    let state = helpers::model_ready_state_for_bear(
        &pool,
        fixture.bear_id(),
        &["openai/gpt-4.1", "openai/gpt-5"],
    )
    .await;
    let quick = configurations::create(&pool, fixture.bear_id(), "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(quick.id))
        .await
        .unwrap();
    let careful = configurations::create(
        &pool,
        fixture.bear_id(),
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, fixture.bear_id(), fixture.hat, Some(careful.id))
        .await
        .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.preflight(&state).await.unwrap().handle,
        "openai/gpt-5"
    );
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-5'"
    )
    .execute(&pool)
    .await
    .unwrap();
    let error = fixture
        .preflight(&state)
        .await
        .err()
        .expect("revoked choice must fail closed");
    assert!(
        error.to_string().contains("no longer selectable"),
        "{error}"
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {}", fixture.token.raw_token)
            .parse()
            .unwrap(),
    );
    let error = super::super::run_start_result(
        &state,
        &headers,
        &json!({
            "bear_slug": fixture.bear.slug, "session_id": fixture.session,
            "conversation_id": fixture.external_id(), "prompt": "Must not infer",
        }),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("no longer selectable"),
        "{error}"
    );
    assert!(
        den_runtime::turn_runs::active_run_for_session(&pool, &fixture.session)
            .await
            .unwrap()
            .is_none()
    );

    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "explicit",
        Some("gpt-5"),
        Some("gpt-5"),
        None,
    )
    .await
    .unwrap();
    assert!(
        fixture.preflight(&state).await.is_err(),
        "revoked pins also fail closed"
    );
    sqlx::query!("DELETE FROM model_selection_options WHERE handle = 'openai/gpt-5'")
        .execute(&pool)
        .await
        .unwrap();
    let error = fixture
        .preflight(&state)
        .await
        .err()
        .expect("removed pin must fail closed");
    assert!(
        error.to_string().contains("not in the Den catalog"),
        "{error}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn historical_auto_diagnostic_cannot_mask_next_turn_configuration_changes(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    let state = helpers::model_ready_state_for_bear(
        &pool,
        fixture.bear_id(),
        &["openai/gpt-4.1", "openai/gpt-5"],
    )
    .await;
    let quick = configurations::create(&pool, fixture.bear_id(), "Quick", "gpt-4.1", None)
        .await
        .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(quick.id))
        .await
        .unwrap();
    let first = fixture.preflight(&state).await.unwrap();
    assert!(persistence::establish_conversation_default_model_state(
        &pool,
        fixture.source.conversation.id,
        &first.handle,
        "pair_default_resolved_at_preflight",
    )
    .await
    .unwrap());
    let careful = configurations::create(
        &pool,
        fixture.bear_id(),
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(careful.id))
        .await
        .unwrap();
    let next = fixture.preflight(&state).await.unwrap();
    assert_eq!(next.handle, "openai/gpt-5");
    assert_eq!(next.source, ResolvedRunModelSource::BearDefault);
    configurations::update(
        &pool,
        fixture.bear_id(),
        careful.id,
        "Careful",
        "gpt-4.1",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.preflight(&state).await.unwrap().handle,
        "openai/gpt-4.1"
    );
    configurations::set_hat_override(&pool, fixture.bear_id(), fixture.hat, Some(careful.id))
        .await
        .unwrap();
    configurations::update(
        &pool,
        fixture.bear_id(),
        careful.id,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    let next = fixture.preflight(&state).await.unwrap();
    assert_eq!(next.handle, "openai/gpt-5");
    assert_eq!(next.source, ResolvedRunModelSource::HatOverride);
    let diagnostic =
        persistence::get_conversation_model_state(&pool, fixture.source.conversation.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(diagnostic.selection_mode, "auto");
    assert_eq!(diagnostic.selected_model.as_deref(), Some("openai/gpt-4.1"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn provider_preflight_rejects_inherited_model_missing_from_bear_catalog(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    // Den admits both models, but this Bear's provider key only admits the lower-precedence one.
    let state =
        helpers::model_ready_state_for_bear(&pool, fixture.bear_id(), &["openai/gpt-4.1"]).await;
    let careful = configurations::create(&pool, fixture.bear_id(), "Careful", "gpt-5", None)
        .await
        .unwrap();
    configurations::set_default(&pool, fixture.bear_id(), Some(careful.id))
        .await
        .unwrap();
    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let error = fixture
        .preflight(&state)
        .await
        .err()
        .expect("provider cannot supply selected model");
    assert!(
        error
            .to_string()
            .contains("openai/gpt-5 is not present in the Bifrost catalog"),
        "{error}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn auto_diagnostic_cannot_bypass_provider_catalog_refresh(pool: PgPool) {
    let fixture = OrdinaryFixture::new(&pool).await;
    let state = helpers::state(pool.clone()); // No provider virtual key or cached catalog.
    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "auto",
        None,
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let error = fixture
        .preflight(&state)
        .await
        .err()
        .expect("auto rows are not continuity pins");
    assert!(
        error
            .to_string()
            .contains("catalog validation failed before run start"),
        "{error}"
    );
    persistence::set_conversation_model_state(
        &pool,
        fixture.source.conversation.id,
        "explicit",
        Some("gpt-4.1"),
        Some("gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let pin = fixture.preflight(&state).await.unwrap();
    assert_eq!(pin.handle, "openai/gpt-4.1");
    assert_eq!(pin.source, ResolvedRunModelSource::ConversationExplicit);
}
