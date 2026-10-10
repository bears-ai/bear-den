use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use den_core::{Governance, TurnExecutionOrigin};
use den_memory::{scoped::MemoryReadGrant, MemorySource};
use den_service::bears::db::{self, BearParams};

use super::*;
use crate::reflection::conductor::{enqueue_recall_index, run_next_recall_index_once};
use tracing::instrument::WithSubscriber;

#[derive(Clone)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for CapturedEvents {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target().starts_with("den_runtime::")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(Vec<String>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push(format!("{field}={value:?}"));
            }
        }
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        self.0.lock().unwrap().extend(fields.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct DeniedFixture {
    url: String,
    contacted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DeniedFixture {
    fn new(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let contacted = Arc::new(AtomicBool::new(false));
        let observed = contacted.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    let request = String::from_utf8_lossy(&bytes);
                    if let Some((headers, body)) = request.split_once("\r\n\r\n") {
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if body.len() >= length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let headers = request.split_once("\r\n\r\n").unwrap().0;
                assert!(headers.lines().any(|line| line
                    .split_once(':')
                    .is_some_and(|(name, value)| name.eq_ignore_ascii_case("x-bf-vk")
                        && value.trim() == "BEAR_KEY_CANARY")));
                observed.store(true, Ordering::Relaxed);
                let body = "BODY_CANARY BEAR_KEY_CANARY INPUT_CANARY";
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            url,
            contacted,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for DeniedFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

async fn fixture_bear(pool: &PgPool) -> Uuid {
    db::create_bear(
        pool,
        BearParams {
            slug: "runtime-embedding",
            name: "Runtime Embedding",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn recall_missing_key_and_provider_denial_are_nonfatal_and_index_failure_is_safe(
    pool: PgPool,
) {
    let bear_id = fixture_bear(&pool).await;
    let mut config = Config::test_stub();
    config.qdrant_url = Some("http://127.0.0.1:1".into());
    config.den_secret_encryption_key = "runtime-embedding-encryption-fixture".into();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("runtime-recall-auth-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    let grant = MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), None);
    let fixture = DeniedFixture::new(403);
    config.llm_api_url = fixture.url.clone();
    let ctx = AssembleTurnContext {
        pool: &pool,
        config: &config,
        stores: &stores,
        bear_id,
        origin: TurnExecutionOrigin::ChannelConversation,
        governance: Governance::Interactive,
        conversation_id: "recall-auth-fixture",
        turn_runtime_context: None,
        human_message: Some("INPUT_CANARY"),
        tool_messages: &[],
        session_id: None,
        workspace_roots: None,
        runtime_target: None,
        conversation_selection: None,
        user_id: None,
        client_context: None,
        include_prompt_memory: false,
        key_memory_cache: None,
        native_runtime: true,
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    assert!(
        build_recall_section(&ctx, "canonical anchors remain", grant)
            .with_subscriber(CapturedEvents(events.clone()))
            .await
            .is_none()
    );
    enqueue_recall_index(&pool, bear_id, "manual")
        .await
        .unwrap();
    let run = run_next_recall_index_once(&pool, &config, &stores, bear_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "failed");
    assert!(run
        .error
        .as_ref()
        .unwrap()
        .contains("credential is missing"));
    assert!(!serde_json::to_string(&run).unwrap().contains("CANARY"));
    assert!(
        !fixture.contacted.load(Ordering::Relaxed),
        "missing key must not send HTTP"
    );

    db::set_bear_bifrost_virtual_key(
        &pool,
        bear_id,
        Some("fixture-vk-id"),
        None,
        Some("BEAR_KEY_CANARY"),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    assert!(
        build_recall_section(&ctx, "canonical anchors remain", grant)
            .with_subscriber(CapturedEvents(events.clone()))
            .await
            .is_none()
    );
    assert!(fixture.contacted.load(Ordering::Relaxed));
    let unauthorized = DeniedFixture::new(401);
    let mut unauthorized_config = config.clone();
    unauthorized_config.llm_api_url = unauthorized.url.clone();
    let unauthorized_ctx = AssembleTurnContext {
        config: &unauthorized_config,
        ..ctx.clone()
    };
    assert!(
        build_recall_section(&unauthorized_ctx, "canonical anchors remain", grant)
            .with_subscriber(CapturedEvents(events.clone()))
            .await
            .is_none()
    );
    assert!(unauthorized.contacted.load(Ordering::Relaxed));
    {
        let recorded = events.lock().unwrap();
        assert!(!recorded.is_empty());
        assert!(recorded.iter().all(|event| !event.contains("CANARY")));
    }

    // A canonical head ensures reflection actually reaches the embedding path.
    let store = stores.store_for_bear(bear_id).await.unwrap();
    den_memory::append_memory_record(
        &store,
        &den_memory::LogicalMemoryPath::shared_core("auth-fixture"),
        "note",
        "curate",
        None,
        "INPUT_CANARY",
        &json!({}),
    )
    .await
    .unwrap();
    enqueue_recall_index(&pool, bear_id, "manual")
        .await
        .unwrap();
    let run = run_next_recall_index_once(&pool, &config, &stores, bear_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "failed");
    assert!(run
        .error
        .as_ref()
        .unwrap()
        .contains("HTTP 403: credential denied"));
    let serialized = serde_json::to_string(&run).unwrap();
    assert!(!serialized.contains("CANARY"));
    assert!(!serialized.contains(&fixture.url));
    assert_eq!(run.output_summary, json!({}));
    store.pool().close().await;
    std::fs::remove_dir_all(&config.bear_sqlite_data_dir).unwrap();
}
