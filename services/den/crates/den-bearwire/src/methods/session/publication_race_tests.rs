use super::*;
use den_core::ids::HatId;
use den_memory::MemorySource;
use den_service::bears::hats::memory_binding::{self, ResolvedMemoryBinding};
use tokio::task::JoinHandle;

struct Fixture {
    pool: sqlx::PgPool,
    state: DenState,
    provider: InferenceFixture,
    user: i32,
    bear: Uuid,
    slug: String,
    token: String,
    session_id: String,
    pending: Option<String>,
    default: HatId,
    selected: HatId,
    default_identity: String,
    selected_identity: String,
}

impl Fixture {
    async fn new(pool: &sqlx::PgPool, pending: bool) -> Self {
        let user = create_test_user(pool).await;
        let (bear, slug) = create_test_bear_without_hats(pool).await;
        let token = create_token_for_bear(pool, user, bear).await;
        let default = hats::create_hat(
            pool,
            BearId::new(bear),
            UserId::new(user),
            "Race default",
            "Default editor identity",
        )
        .await
        .unwrap()
        .id;
        let selected = hats::create_hat(
            pool,
            BearId::new(bear),
            UserId::new(user),
            "Race selection",
            "Explicit editor identity",
        )
        .await
        .unwrap()
        .id;
        let default_identity = format!("default-race-identity-{}", Uuid::new_v4());
        let selected_identity = format!("selected-race-identity-{}", Uuid::new_v4());
        for (hat, name, identity) in [
            (default, "Race default", &default_identity),
            (selected, "Race selection", &selected_identity),
        ] {
            hats::manage::update_hat(
                pool,
                BearId::new(bear),
                hat,
                name,
                "Race fixture",
                identity,
                false,
            )
            .await
            .unwrap();
        }
        let provider = InferenceFixture::start();
        provider.pause_completions();
        let mut config = den_core::config::Config::test_stub();
        config.den_secret_encryption_key = "bearwire-test-secret-key".into();
        config.llm_api_url = provider.url.clone();
        config.default_llm_model = "openai/bearwire-test-model".into();
        seed_test_bifrost_virtual_key(pool, bear, &config).await;
        let state = test_state_with_config(pool.clone(), config);
        let session_id = format!("publication-race-{}", Uuid::new_v4());
        let pending = if pending {
            let opened = rpc_value(
                state.clone(),
                &token,
                "session.open",
                json!({
                    "bear_slug": slug, "session_id": session_id,
                }),
            )
            .await;
            assert_access(&opened["result"]["session"], "awaiting_hat", true);
            Some(
                opened["result"]["session"]["conversation_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        } else {
            None
        };
        hats::set_ide_default_hat(pool, BearId::new(bear), default)
            .await
            .unwrap();
        Self {
            pool: pool.clone(),
            state,
            provider,
            user,
            bear,
            slug,
            token,
            session_id,
            pending,
            default,
            selected,
            default_identity,
            selected_identity,
        }
    }

    fn start(&self, prompt: &str) -> JoinHandle<Value> {
        let state = self.state.clone();
        let token = self.token.clone();
        let params = json!({"bear_slug": self.slug, "session_id": self.session_id,
            "conversation_id": self.pending, "prompt": prompt});
        tokio::spawn(async move { rpc_value(state, &token, "run.start", params).await })
    }

    fn select(&self) -> JoinHandle<Value> {
        let state = self.state.clone();
        let token = self.token.clone();
        let params =
            json!({"bear_slug": self.slug, "session_id": self.session_id, "hat_id": self.selected});
        tokio::spawn(async move { rpc_value(state, &token, "session.hat.select", params).await })
    }

    async fn source(&self, hat: HatId) -> persistence::ConversationRecord {
        let records = conversations(&self.pool, self.bear).await;
        assert_eq!(
            records.len(),
            1,
            "competing or orphaned canonical conversations: {records:?}"
        );
        let conversation = records.into_iter().next().unwrap();
        let session = client_sessions::find_for_user_bear_session_id(
            &self.pool,
            self.user,
            self.bear,
            &self.session_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            session.resolved_conversation_id,
            conversation.external_conversation_id
        );
        assert_eq!(
            conversation.source_client_session_id.as_deref(),
            Some(self.session_id.as_str())
        );
        let viewer = den_service::conversation::viewer::ConversationViewer::resolve(
            &self.pool,
            BearId::new(self.bear),
            UserId::new(self.user),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(viewer
            .may_read_own_source(&self.pool, conversation.id)
            .await
            .unwrap());
        let ResolvedMemoryBinding::Bound(grant) = memory_binding::for_external_conversation(
            &self.pool,
            BearId::new(self.bear),
            conversation.external_conversation_id.as_deref().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(grant.source(), MemorySource::Conversation(conversation.id));
        assert_eq!(grant.hat_id(), Some(hat));
        conversation
    }

    async fn replay_winner(&self, initial: &Value, hat: HatId, first_prompt: &str) {
        wait_for_provider(&self.provider, 1).await;
        assert_eq!(self.provider.completion_count(), 1);
        let canonical = self.source(hat).await;
        let identity = if hat == self.selected {
            &self.selected_identity
        } else {
            &self.default_identity
        };
        let other = if hat == self.selected {
            &self.default_identity
        } else {
            &self.selected_identity
        };
        let body = self.provider.completion_bodies()[0].to_string();
        assert!(body.contains(identity), "winner identity absent: {body}");
        assert!(!body.contains(other), "losing identity leaked: {body}");
        self.provider.resume_completions();
        wait_for_completed_run(&self.pool, initial["result"]["run_id"].as_str().unwrap()).await;
        wait_for_user_message(
            &self.pool,
            self.bear,
            canonical.external_conversation_id.as_deref().unwrap(),
            first_prompt,
        )
        .await;
        wait_for_history(&self.pool, canonical.id, 2).await;
        assert_eq!(
            sqlx::query_scalar!(
                "SELECT count(*) AS \"count!\" FROM turn_runs WHERE session_id = $1",
                self.session_id,
            )
            .fetch_one(&self.pool)
            .await
            .unwrap(),
            1
        );
        let messages = persistence::list_projected_messages_page(
            &self.pool,
            canonical.id,
            None,
            100,
            persistence::ConversationHistoryProjection::UserHistory,
        )
        .await
        .unwrap();
        assert_eq!(messages.len(), 2);
        let followup = self.start("race follow-up prompt").await.unwrap();
        assert_eq!(followup["result"]["accepted"], true, "{followup}");
        wait_for_completed_run(&self.pool, followup["result"]["run_id"].as_str().unwrap()).await;
        wait_for_provider(&self.provider, 2).await;
        wait_for_history(&self.pool, canonical.id, 4).await;
        assert_eq!(
            self.source(hat).await.id,
            canonical.id,
            "next request substituted the winner"
        );
        let bodies = self.provider.completion_bodies();
        assert_eq!(bodies.len(), 2);
        let messages = bodies[1]["messages"].as_array().unwrap();
        for (role, text) in [
            ("user", first_prompt),
            ("assistant", "live source admitted"),
            ("user", "race follow-up prompt"),
        ] {
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["role"] == role && message["content"] == text)
                    .count(),
                1,
                "{bodies:?}"
            );
        }
        assert!(bodies[1].to_string().contains(identity));
        assert!(!bodies[1].to_string().contains(other));
        assert_eq!(
            sqlx::query_scalar!(
                "SELECT count(*) AS \"count!\" FROM turn_runs WHERE session_id = $1",
                self.session_id,
            )
            .fetch_one(&self.pool)
            .await
            .unwrap(),
            2
        );
    }
}

async fn wait_for_blocked(pool: &sqlx::PgPool, expected: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = sqlx::query_scalar!(
                r#"SELECT count(*) AS "count!" FROM pg_stat_activity
                   WHERE datname = current_database() AND wait_event_type = 'Lock'
                     AND pid <> pg_backend_pid()"#,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if count >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup commands did not reach the held database lock");
}

async fn wait_for_provider(provider: &InferenceFixture, expected: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.completion_count() < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("provider completion request did not arrive");
}

async fn wait_for_history(pool: &sqlx::PgPool, canonical: Uuid, expected: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages = persistence::list_projected_messages_page(
                pool,
                canonical,
                None,
                100,
                persistence::ConversationHistoryProjection::UserHistory,
            )
            .await
            .unwrap();
            if messages.len() == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("canonical user/assistant persistence did not settle");
}

async fn responses(first: JoinHandle<Value>, second: JoinHandle<Value>) -> (Value, Value) {
    tokio::time::timeout(Duration::from_secs(15), async {
        (first.await.unwrap(), second.await.unwrap())
    })
    .await
    .expect("concurrent startup commands did not settle")
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_initial_run_starts_share_one_published_source(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool, true).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM client_sessions WHERE bear_id = $1 AND client_session_id = $2 FOR UPDATE",
        fixture.bear,
        fixture.session_id
    )
    .fetch_one(&mut *held)
    .await
    .unwrap();
    let first = fixture.start("race initial prompt");
    wait_for_blocked(&pool, 1).await;
    let second = fixture.start("race initial prompt");
    wait_for_blocked(&pool, 2).await;
    held.commit().await.unwrap();
    let (first, second) = responses(first, second).await;
    assert_eq!(first["result"]["accepted"], true, "{first}");
    assert_eq!(second["result"]["accepted"], true, "{second}");
    assert_eq!(
        first["result"]["run_id"], second["result"]["run_id"],
        "{first}; {second}"
    );
    fixture
        .replay_winner(&first, fixture.default, "race initial prompt")
        .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_initial_run_starts_publish_atomically_without_orphans(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool, false).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('bearwire.source-publication:' || $1, 0)) IS NULL AS \"locked!\"",
        fixture.session_id,
    ).fetch_one(&mut *held).await.unwrap();
    let first = fixture.start("race initial prompt");
    wait_for_blocked(&pool, 1).await;
    let second = fixture.start("race initial prompt");
    wait_for_blocked(&pool, 2).await;
    held.commit().await.unwrap();
    let (first, second) = responses(first, second).await;
    let (winner, loser) = if first["result"]["accepted"] == true {
        (&first, &second)
    } else {
        (&second, &first)
    };
    assert_eq!(winner["result"]["accepted"], true, "{first}; {second}");
    assert_authorization_error(
        loser,
        "run.start",
        "admitted canonical session conversation changed",
    );
    fixture
        .replay_winner(winner, fixture.default, "race initial prompt")
        .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn pending_run_start_adopts_successful_hat_selection_without_overwrite(pool: sqlx::PgPool) {
    let fixture = Fixture::new(&pool, true).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM client_sessions WHERE bear_id = $1 AND client_session_id = $2 FOR UPDATE",
        fixture.bear,
        fixture.session_id
    )
    .fetch_one(&mut *held)
    .await
    .unwrap();
    // PostgreSQL lock queues make selection win after both commands have read pending P.
    let selected = fixture.select();
    wait_for_blocked(&pool, 1).await;
    let started = fixture.start("race selected prompt");
    wait_for_blocked(&pool, 2).await;
    held.commit().await.unwrap();
    let (selected, started) = responses(selected, started).await;
    assert_eq!(selected["result"]["ok"], true, "{selected}");
    assert_eq!(started["result"]["accepted"], true, "{started}");
    let canonical = fixture.source(fixture.selected).await;
    assert_eq!(
        selected["result"]["conversation_id"],
        canonical.external_conversation_id.as_deref().unwrap()
    );
    fixture
        .replay_winner(&started, fixture.selected, "race selected prompt")
        .await;
}
