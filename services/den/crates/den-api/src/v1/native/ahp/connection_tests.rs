use super::super::wire::{UserKind, UserMessage, UserOrigin};
use super::*;
use den_core::config::Config;
use std::sync::Arc;

fn state() -> DenState {
    let config = Arc::new(Config::test_stub());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    DenState::new(
        pool,
        config.clone(),
        Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        den_memory::MemoryStoreManager::new(&config),
    )
}
fn connection() -> Connection {
    let (events, _) = mpsc::channel(8);
    Connection {
        state: state(),
        headers: HeaderMap::new(),
        user: 1,
        client: None,
        subscriptions: HashSet::new(),
        last_client_seq: 0,
        live: None,
        running: None,
        events,
    }
}

#[tokio::test]
async fn initialize_and_snapshot_reconnect_use_released_shapes() {
    let mut connection = connection();
    let initialized=connection.command("initialize",json!({"channel":wire::ROOT,"protocolVersions":["1.0.0"],"clientId":"apple","initialSubscriptions":[wire::ROOT]})).await.unwrap();
    assert_eq!(initialized["protocolVersion"], "1.0.0");
    assert!(initialized["serverSeq"].is_u64());
    assert_eq!(initialized["snapshots"][0]["resource"], wire::ROOT);
    assert!(initialized["snapshots"][0]["fromSeq"].is_u64());
    assert_eq!(
        initialized["snapshots"][0]["state"]["agents"][0]["provider"],
        "den"
    );
    assert!(connection
        .command(
            "initialize",
            json!({"channel":wire::ROOT,"protocolVersions":["1.0.0"],"clientId":"apple"})
        )
        .await
        .is_err());
    let mut fresh = self::connection();
    let reconnected=fresh.command("reconnect",json!({"channel":wire::ROOT,"clientId":"apple","lastSeenServerSeq":0,"subscriptions":[wire::ROOT]})).await.unwrap();
    assert_eq!(reconnected["type"], "snapshot");
    assert!(reconnected.get("actions").is_none());
    assert!(fresh.running.is_none());
}

#[tokio::test]
async fn unsupported_and_stale_actions_are_rejected_without_execution() {
    let mut connection = connection();
    connection.client = Some("apple".into());
    let id = uuid::Uuid::new_v4();
    let params = json!({"channel":Channel::Chat(id).uri(),"clientSeq":1,"action":{"type":"chat/toolCallConfirmed","toolCallId":"not-an-obligation"}});
    let rejected = connection.dispatch(params.clone()).await;
    assert!(rejected[0]["params"]["rejectionReason"].is_string());
    assert_eq!(rejected[0]["params"]["origin"]["clientSeq"], 1);
    assert!(connection.running.is_none());
    let duplicate = connection.dispatch(params).await;
    assert!(duplicate[0]["params"]["rejectionReason"]
        .as_str()
        .unwrap()
        .contains("stale"));
    assert!(connection.running.is_none());
    for method in [
        "createSession",
        "createChat",
        "listSessions",
        "fetchTurns",
        "authenticate",
        "terminal/create",
    ] {
        assert_eq!(
            connection.command(method, json!({})).await.unwrap_err().0,
            -32601
        );
    }
}

#[tokio::test]
async fn cancellation_before_admission_cannot_launch_a_turn() {
    let (tx, mut rx) = mpsc::channel(4);
    let (cancel, watch) = watch::channel(false);
    cancel.send(true).unwrap();
    let input = turn::Input {
        conversation: uuid::Uuid::new_v4(),
        turn: uuid::Uuid::new_v4(),
        message: UserMessage {
            text: "hello".into(),
            origin: UserOrigin {
                kind: UserKind::User,
            },
        },
        origin: Origin {
            client_id: "apple".into(),
            client_seq: 1,
        },
    };
    Box::pin(turn::run(state(), HeaderMap::new(), input, watch, tx)).await;
    assert!(matches!(rx.recv().await, Some(turn::Event::Rejected(_))));
    assert!(matches!(rx.recv().await, Some(turn::Event::Finished)));
    assert!(rx.recv().await.is_none());
}
