use super::*;
use den_core::ModelAvailabilityFailureKind as Kind;
use sqlx::PgPool;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
};

pub(super) struct Mock {
    url: String,
    reply: Arc<Mutex<(u16, String, bool)>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Mock {
    pub(super) fn new(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let reply = Arc::new(Mutex::new((status, body.to_owned(), false)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (responses, records, stopping) = (reply.clone(), requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut buffer = [0; 1024];
                        while request.len() < 8192 {
                            let Ok(count) = stream.read(&mut buffer) else {
                                break;
                            };
                            if count == 0 {
                                break;
                            }
                            request.extend_from_slice(&buffer[..count]);
                            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        records
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&request).into_owned());
                        let (status, body, truncated) = responses.lock().unwrap().clone();
                        let length = body.len() + if truncated { 100 } else { 0 };
                        let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
        });
        Self {
            url,
            reply,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    pub(super) fn config(&self) -> Config {
        let mut config = Config::test_stub();
        config.llm_api_url.clone_from(&self.url);
        config.llm_api_key = "WRONG-PROCESS-BEARER".into();
        config.den_secret_encryption_key = "catalog-test-encryption".into();
        config
    }

    pub(super) fn respond(&self, status: u16, body: &str) {
        *self.reply.lock().unwrap() = (status, body.to_owned(), false);
    }

    pub(super) fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

pub(super) const LIVE: &str =
    r#"{"data":[{"id":"openai/gpt-6-sol","supported_methods":["responses"]}]}"#;
pub(super) const PRIVATE: &str =
    "PRIVATE sk-bf-secret https://user:password@gateway/models?key=SECRET";

pub(super) fn assert_failure<T: std::fmt::Debug>(result: Result<T, DenError>, kind: Kind) {
    let error = result.unwrap_err();
    let DenError::ModelAvailability(failure) = &error else {
        panic!("expected typed failure: {error:?}")
    };
    assert_eq!(failure.kind, kind);
    let rendered = format!("{error:?} {error}");
    for private in [
        "PRIVATE",
        "sk-bf-secret",
        "password",
        "https://",
        "WRONG-PROCESS-BEARER",
    ] {
        assert!(!rendered.contains(private), "raw cause leaked");
    }
}

#[tokio::test]
async fn status_decode_body_and_network_failures_are_typed_and_safe() {
    for (status, body, kind) in [
        (401, PRIVATE, Kind::VirtualKeyRejected),
        (403, PRIVATE, Kind::VirtualKeyRejected),
        (503, PRIVATE, Kind::CatalogUnavailable),
        (200, PRIVATE, Kind::CatalogUnavailable),
    ] {
        let mock = Mock::new(status, body);
        let client = BifrostClient::new(&mock.config());
        assert_failure(
            client
                .list_available_models_with_virtual_key(Some("sk-bf-secret"))
                .await,
            kind,
        );
        assert_eq!(mock.count(), 1);
        let request = mock.requests.lock().unwrap()[0].to_ascii_lowercase();
        assert!(request.contains("x-bf-vk: sk-bf-secret"));
        assert!(!request.contains("authorization:"));
    }
    let mock = Mock::new(200, PRIVATE);
    mock.reply.lock().unwrap().2 = true;
    assert_failure(
        BifrostClient::new(&mock.config())
            .list_available_models_with_virtual_key(Some("sk-bf-secret"))
            .await,
        Kind::CatalogUnavailable,
    );
    let mut config = mock.config();
    // Invalid URL fails without any DNS/network request, and never retains its secret.
    config.llm_api_url = "https://[PRIVATE?key=SECRET".into();
    assert_failure(
        BifrostClient::new(&config)
            .list_available_models_with_virtual_key(Some("sk-bf-secret"))
            .await,
        Kind::CatalogUnavailable,
    );
    assert_failure(
        BifrostClient::new(&mock.config())
            .list_available_models_with_virtual_key(Some(" "))
            .await,
        Kind::VirtualKeyMissing,
    );
}

#[test]
fn require_available_model_never_invents_catalog_membership_or_enabled_state() {
    let mut model = serde_json::from_str::<BifrostLiveModelsResponse>(LIVE)
        .unwrap()
        .data
        .pop()
        .unwrap()
        .into_metadata()
        .unwrap();
    model.enabled = false;
    let snapshot = BifrostCatalogSnapshot::from_available_models(vec![model]);
    assert_failure(
        snapshot.require_available_model("openai/gpt-6-sol"),
        Kind::ModelUnavailable,
    );
    assert_failure(
        snapshot.require_available_model("openai/other-model"),
        Kind::ModelMissing,
    );
    let DenError::ModelAvailability(failure) = snapshot
        .require_available_model("openai/gpt-6-sol")
        .unwrap_err()
    else {
        unreachable!()
    };
    assert!(failure.public_message().contains("openai/gpt-6-sol"));
}

pub(super) async fn bear(pool: &PgPool, slug: &str) -> Uuid {
    crate::bears::db::create_bear(
        pool,
        crate::bears::db::BearParams {
            slug,
            name: "Catalog Bear",
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

pub(super) async fn key(pool: &PgPool, bear_id: Uuid, value: &str, config: &Config) {
    crate::bears::db::set_bear_bifrost_virtual_key(
        pool,
        bear_id,
        Some("test-vk"),
        Some("catalog-test"),
        Some(value),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn writes_require_fresh_authenticated_catalog_and_bear_credentials_are_isolated(
    pool: PgPool,
) {
    let mock = Mock::new(200, LIVE);
    let config = mock.config();
    let client = BifrostClient::new(&config);
    let first = bear(&pool, "catalog-first").await;
    let other = bear(&pool, "catalog-other").await;
    key(&pool, first, "sk-bf-first", &config).await;
    client
        .bear_catalog_snapshot(&pool, first, &config.den_secret_encryption_key)
        .await
        .unwrap();
    assert_eq!(mock.count(), 1);
    assert_failure(
        client
            .bear_catalog_snapshot(&pool, other, &config.den_secret_encryption_key)
            .await,
        Kind::VirtualKeyMissing,
    );
    assert_eq!(
        mock.count(),
        1,
        "another Bear must not inherit the first Bear's cache or process bearer"
    );
    mock.respond(200, r#"{"data":[]}"#);
    assert_failure(
        client
            .validate_bear_model_selection(
                &pool,
                first,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
            )
            .await,
        Kind::ModelMissing,
    );
    assert_eq!(
        mock.count(),
        2,
        "writes cannot rely on a positive TTL cache"
    );
    assert!(client
        .cached_bear_catalog_snapshot(first)
        .unwrap()
        .models
        .is_empty());
    mock.respond(200, LIVE);
    client
        .validate_bear_model_selection(
            &pool,
            first,
            "openai/gpt-6-sol",
            &config.den_secret_encryption_key,
        )
        .await
        .unwrap();
    assert_eq!(mock.count(), 3);
    assert_failure(
        client
            .bear_catalog_snapshot(&pool, first, "wrong-encryption-key")
            .await,
        Kind::VirtualKeyRejected,
    );
    assert!(client.cached_bear_catalog_snapshot(first).is_none());
    assert_eq!(
        mock.count(),
        3,
        "wrong encryption key must fail before HTTP/cache reuse"
    );
    client
        .bear_catalog_snapshot(&pool, first, &config.den_secret_encryption_key)
        .await
        .unwrap();
    key(&pool, first, "sk-bf-rotated", &config).await;
    mock.respond(403, PRIVATE);
    assert_failure(
        client
            .bear_catalog_snapshot(&pool, first, &config.den_secret_encryption_key)
            .await,
        Kind::VirtualKeyRejected,
    );
    assert_eq!(
        mock.count(),
        5,
        "rotation bypasses cache; rejection is not retried"
    );
    assert!(client.cached_bear_catalog_snapshot(first).is_none());
    {
        let requests = mock.requests.lock().unwrap();
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("x-bf-vk: sk-bf-rotated"));
        assert!(requests
            .iter()
            .all(|request| !request.to_ascii_lowercase().contains("authorization:")));
    }
    key(&pool, other, "sk-bf-other", &config).await;
    mock.respond(200, r#"{"data":[]}"#);
    assert_failure(
        client
            .validate_bear_model_selection(
                &pool,
                other,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
            )
            .await,
        Kind::ModelMissing,
    );
    assert_eq!(mock.count(), 6);
    assert!(mock.requests.lock().unwrap()[5]
        .to_ascii_lowercase()
        .contains("x-bf-vk: sk-bf-other"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn missing_key_and_both_auth_statuses_cannot_use_a_positive_write_cache(pool: PgPool) {
    let mock = Mock::new(200, LIVE);
    let config = mock.config();
    let client = BifrostClient::new(&config);
    let bear_id = bear(&pool, "catalog-auth").await;
    assert_failure(
        client
            .validate_bear_model_selection(
                &pool,
                bear_id,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
            )
            .await,
        Kind::VirtualKeyMissing,
    );
    assert_eq!(mock.count(), 0);
    key(&pool, bear_id, "sk-bf-secret", &config).await;
    for status in [401, 403] {
        mock.respond(200, LIVE);
        client
            .refresh_bear_catalog_snapshot(&pool, bear_id, &config.den_secret_encryption_key)
            .await
            .unwrap();
        mock.respond(status, PRIVATE);
        assert_failure(
            client
                .validate_bear_model_selection(
                    &pool,
                    bear_id,
                    "openai/gpt-6-sol",
                    &config.den_secret_encryption_key,
                )
                .await,
            Kind::VirtualKeyRejected,
        );
        assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
    }
    assert_eq!(mock.count(), 4);
}
