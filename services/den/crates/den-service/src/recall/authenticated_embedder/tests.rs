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

use den_memory::{
    library::CuratedMemoryGrant, scoped::MemoryReadGrant, MemorySource, MemoryStoreManager,
};
use uuid::Uuid;

use super::*;
use crate::{
    bears::db::{self, BearParams},
    recall::{query, reindex_bear_now},
};

struct Fixture {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
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
                let headers = request
                    .split_once("\r\n\r\n")
                    .unwrap()
                    .0
                    .to_ascii_lowercase();
                assert!(
                    headers.contains("x-bf-vk:"),
                    "unauthenticated embedding request"
                );
                assert!(!headers.contains("authorization:"));
                recorded.lock().unwrap().push(request);
                let body = if status == 200 {
                    r#"{"data":[{"index":0,"embedding":[0.1,0.2]}]}"#
                } else {
                    "BODY_CANARY KEY_A_CANARY KEY_B_CANARY INPUT_CANARY GLOBAL_CANARY"
                };
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| {
                request
                    .split_once("\r\n\r\n")
                    .unwrap()
                    .0
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("x-bf-vk")
                            .then(|| value.trim().to_string())
                    })
                    .unwrap()
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

async fn bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        db::create_bear(
            pool,
            BearParams {
                slug,
                name: slug,
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    )
}

async fn set_key(pool: &PgPool, config: &Config, bear: BearId, secret: &str) {
    db::set_bear_bifrost_virtual_key(
        pool,
        bear.as_uuid(),
        Some("fixture-vk-id"),
        None,
        Some(secret),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
}

fn safe_error<T>(result: Result<T, DenError>, expected: &str) {
    let Err(error) = result else {
        panic!("expected safe failure")
    };
    let text = format!("{error} {error:?}");
    assert!(text.contains(expected));
    assert!(!text.contains("CANARY"));
    assert!(!text.contains("http://"));
}

#[tokio::test]
async fn unset_embeddings_skip_without_database_or_http() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let config = Config::test_stub();
    assert!(
        authenticated_embedder(&pool, &config, BearId::new(Uuid::nil()))
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn current_bear_isolation_rotation_revocation_and_all_search_guards(pool: PgPool) {
    let fixture = Fixture::new(200);
    let mut config = Config::test_stub();
    config.llm_api_url = fixture.url.clone();
    config.llm_api_key = "GLOBAL_CANARY".into();
    config.den_secret_encryption_key = "encryption-key-fixture-canary".into();
    config.embedding_dimensions = 2;
    config.qdrant_url = Some("http://127.0.0.1:1".into());
    let a = bear(&pool, "embedding-a").await;
    let b = bear(&pool, "embedding-b").await;
    set_key(&pool, &config, a, "KEY_A_CANARY").await;
    set_key(&pool, &config, b, "KEY_B_CANARY").await;
    for id in [a, b] {
        authenticated_embedder(&pool, &config, id)
            .await
            .unwrap()
            .unwrap()
            .embed_one("INPUT_CANARY")
            .await
            .unwrap();
    }
    set_key(&pool, &config, a, "ROTATED_A_CANARY").await;
    authenticated_embedder(&pool, &config, a)
        .await
        .unwrap()
        .unwrap()
        .embed_one("INPUT_CANARY")
        .await
        .unwrap();
    assert_eq!(
        fixture.keys(),
        ["KEY_A_CANARY", "KEY_B_CANARY", "ROTATED_A_CANARY"]
    );
    assert!(fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| !request.contains("GLOBAL_CANARY")));

    db::clear_bear_bifrost_virtual_key(&pool, a.as_uuid())
        .await
        .unwrap();
    safe_error(
        authenticated_embedder(&pool, &config, a).await,
        "credential is missing",
    );
    let grant = MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), None);
    let curated = CuratedMemoryGrant::new(Vec::new());
    safe_error(
        query::semantic_search_for_bear(&pool, &config, a.as_uuid(), "INPUT_CANARY", 5).await,
        "credential is missing",
    );
    safe_error(
        query::search_curated_library(&pool, &config, a.as_uuid(), &curated, "INPUT_CANARY", 5)
            .await,
        "credential is missing",
    );
    safe_error(
        query::search_bear_memory_with_grant(&pool, &config, a.as_uuid(), grant, "INPUT_CANARY", 5)
            .await,
        "credential is missing",
    );
    safe_error(
        query::search_bear_memory_for_entities(
            &pool,
            &config,
            a.as_uuid(),
            &["entity".into()],
            "INPUT_CANARY",
            5,
        )
        .await,
        "credential is missing",
    );
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("missing-embedding-key-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    safe_error(
        reindex_bear_now(&pool, &config, &stores, a.as_uuid()).await,
        "credential is missing",
    );
    assert_eq!(
        fixture.keys().len(),
        3,
        "missing current Bear key must never borrow Bear B's key"
    );

    set_key(&pool, &config, a, "KEY_A_CANARY").await;
    let denied = Fixture::new(403);
    config.llm_api_url = denied.url.clone();
    safe_error(
        query::semantic_search_for_bear(&pool, &config, a.as_uuid(), "INPUT_CANARY", 5).await,
        "HTTP 403: credential denied",
    );
    safe_error(
        query::search_curated_library(&pool, &config, a.as_uuid(), &curated, "INPUT_CANARY", 5)
            .await,
        "HTTP 403: credential denied",
    );
    safe_error(
        query::search_bear_memory_with_grant(&pool, &config, a.as_uuid(), grant, "INPUT_CANARY", 5)
            .await,
        "HTTP 403: credential denied",
    );
    safe_error(
        query::search_bear_memory_for_entities(
            &pool,
            &config,
            a.as_uuid(),
            &["entity".into()],
            "INPUT_CANARY",
            5,
        )
        .await,
        "HTTP 403: credential denied",
    );
    assert_eq!(denied.keys(), vec!["KEY_A_CANARY"; 4]);

    config.den_secret_encryption_key = "wrong-encryption-key-canary".into();
    safe_error(
        authenticated_embedder(&pool, &config, a).await,
        "credential lookup failed",
    );
    assert_eq!(denied.keys().len(), 4);
}
