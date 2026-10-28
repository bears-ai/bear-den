//! PostgreSQL-backed native-edge ownership, replay and reconnect regressions.
use super::{
    access,
    conversations::{self, Create},
    history,
};
use den_core::{
    config::Config,
    ids::{BearId, UserId},
};
use den_service::{
    bears::{db, hats},
    conversation::{message_types::ConversationMessageWrite, persistence},
    DenState,
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

fn state(pool: PgPool) -> DenState {
    let config = Arc::new(Config::test_stub());
    DenState::new(
        pool,
        config.clone(),
        Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        den_memory::MemoryStoreManager::new(&config),
    )
}
async fn user(pool: &PgPool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    sqlx::query_scalar!("INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",format!("{suffix}@example.test"),format!("u{}",&suffix[..16]),"Native API test","unused").fetch_one(pool).await.unwrap()
}
async fn seed(pool: &PgPool) -> (DenState, Uuid, [i32; 3], Uuid) {
    let owner = user(pool).await;
    let member = user(pool).await;
    let admin = user(pool).await;
    let slug = format!("native-{}", Uuid::new_v4().simple());
    let bear = db::create_bear(
        pool,
        db::BearParams {
            slug: &slug,
            name: "Native Bear",
            description: "Safe profile",
            system_prompt: "Test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    for (user, role) in [
        (owner, db::BEAR_ROLE_MEMBER),
        (member, db::BEAR_ROLE_MEMBER),
        (admin, db::BEAR_ROLE_ADMIN),
    ] {
        db::grant_membership(pool, user, bear, Some(role))
            .await
            .unwrap();
    }
    let hat = hats::create_hat_with_summary(
        pool,
        BearId::new(bear),
        UserId::new(admin),
        "General",
        "Private purpose must not leave the service",
        Some("Shared summary"),
    )
    .await
    .unwrap();
    (
        state(pool.clone()),
        bear,
        [owner, member, admin],
        hat.id.as_uuid(),
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn creation_is_idempotent_and_inspection_never_grants_execution(pool: PgPool) {
    let (state, bear, [owner, member, admin], hat) = seed(&pool).await;
    let mut request = Create {
        bear_id: bear,
        hat_id: hat,
        request_id: Uuid::new_v4(),
    };
    let first = conversations::create_for(&state, owner, &request)
        .await
        .unwrap();
    let again = conversations::create_for(&state, owner, &request)
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(first.chat_uri, again.chat_uri);
    let source = access::selected(&state, owner, bear, &first.id)
        .await
        .unwrap();
    assert!(access::executable(&state, owner, &source).await.is_ok());
    assert!(access::selected(&state, member, bear, &first.id)
        .await
        .is_err());
    assert!(access::selected(&state, admin, bear, &first.id)
        .await
        .is_ok());
    assert!(access::executable(&state, admin, &source).await.is_err());
    let inventory = conversations::inventory(&state, admin, bear).await.unwrap();
    assert!(!inventory.conversations[0].can_send);
    let hat_projection = serde_json::to_value(&inventory.hats[0]).unwrap();
    assert_eq!(hat_projection["short_summary"], "Shared summary");
    assert!(hat_projection.get("purpose").is_none());
    let other = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Other",
        "Other purpose",
    )
    .await
    .unwrap();
    request.hat_id = other.id.as_uuid();
    assert_eq!(
        conversations::create_for(&state, owner, &request)
            .await
            .unwrap_err()
            .status,
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        hats::bindings::conversation_hat(&pool, BearId::new(bear), source.id)
            .await
            .unwrap()
            .unwrap()
            .as_uuid(),
        hat
    );
    den_service::archived_conversations::set_archived(
        &pool,
        bear,
        &first.id,
        Some(owner),
        "native_test",
        true,
    )
    .await
    .unwrap();
    assert!(access::executable(&state, owner, &source).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_text_is_replayable_and_a_persisted_turn_cannot_be_restarted(pool: PgPool) {
    let (state, bear, [owner, ..], hat) = seed(&pool).await;
    let created = conversations::create_for(
        &state,
        owner,
        &Create {
            bear_id: bear,
            hat_id: hat,
            request_id: Uuid::new_v4(),
        },
    )
    .await
    .unwrap();
    let source = access::selected(&state, owner, bear, &created.id)
        .await
        .unwrap();
    let turn = Uuid::new_v4();
    history::require_new_turn(&state, source.id, turn)
        .await
        .unwrap();
    let mut user = ConversationMessageWrite::user_turn(
        "question",
        json!({"type":"user_input","request_id":turn}),
        Some(format!("ahp-user:{turn}")),
    );
    user.provider_message_id = Some(turn.to_string());
    persistence::append_message(&pool, source.id, &user)
        .await
        .unwrap();
    persistence::append_message(&pool, source.id, &user)
        .await
        .unwrap();
    let mut assistant = ConversationMessageWrite::assistant_turn(
        "answer",
        json!({"type":"assistant_output","ahp_state":"complete"}),
    );
    assistant.source_event_id = Some(format!("ahp-assistant:{turn}"));
    persistence::append_message(&pool, source.id, &assistant)
        .await
        .unwrap();
    assert!(history::require_new_turn(&state, source.id, turn)
        .await
        .is_err());
    let replay = den_runtime::agent_loop::load_transcript_messages(&pool, bear, &created.id)
        .await
        .unwrap();
    assert_eq!(replay.len(), 2);
    assert_eq!(replay[0].role, "user");
    assert_eq!(replay[1].role, "assistant");
    assert_eq!(replay[0].content.as_deref(), Some("question"));
    assert_eq!(replay[1].content.as_deref(), Some("answer"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn reconnect_snapshots_load_typed_empty_terminal_outcomes_without_fake_prose(pool: PgPool) {
    use den_runtime::channel_lifecycle::ChannelTurnOutcome;
    use den_runtime::native_runtime::persist_channel_input;
    use den_service::tool_turns::ConversationTurnLease;
    let (state, bear, [owner, ..], hat) = seed(&pool).await;
    let created = conversations::create_for(
        &state,
        owner,
        &Create {
            bear_id: bear,
            hat_id: hat,
            request_id: Uuid::new_v4(),
        },
    )
    .await
    .unwrap();
    let source = access::selected(&state, owner, bear, &created.id)
        .await
        .unwrap();
    for (index, outcome) in [
        ChannelTurnOutcome::Cancelled,
        ChannelTurnOutcome::Completed,
        ChannelTurnOutcome::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        let lease = ConversationTurnLease::acquire(&pool, source.id)
            .await
            .unwrap();
        let input = persist_channel_input(&pool, bear, owner, &lease, Uuid::new_v4(), "question")
            .await
            .unwrap();
        let terminal = input
            .finish_without_runtime(&pool, outcome, 7, Some("Safe failure".into()))
            .await
            .unwrap();
        // Re-finalization returns the canonical outcome, not a second writable copy.
        let repeated = input
            .finish_without_runtime(&pool, ChannelTurnOutcome::Failed, 99, None)
            .await
            .unwrap();
        assert_eq!(repeated.outcome, terminal.outcome);
        drop(lease);
        let snapshot = super::ahp::chat_snapshot_for_test(&state, owner, source.id)
            .await
            .unwrap();
        let turns = snapshot["turns"].as_array().unwrap();
        assert_eq!(turns.len(), index + 1);
        let turn = &turns[index];
        assert_eq!(turn["duration"], 7);
        let expected = match outcome {
            ChannelTurnOutcome::Completed => "complete",
            ChannelTurnOutcome::Cancelled => "cancelled",
            ChannelTurnOutcome::Failed => "error",
        };
        assert_eq!(turn["state"], expected);
        assert_eq!(
            turn["responseParts"].as_array().unwrap().len(),
            usize::from(outcome == ChannelTurnOutcome::Failed)
        );
        if outcome == ChannelTurnOutcome::Failed {
            assert_eq!(turn["responseParts"][0]["error"]["message"], "Safe failure");
        }
    }
    let replay = den_runtime::agent_loop::load_transcript_messages(&pool, bear, &created.id)
        .await
        .unwrap();
    assert_eq!(replay.len(), 3);
    assert!(replay.iter().all(|message| message.role == "user"));
}
