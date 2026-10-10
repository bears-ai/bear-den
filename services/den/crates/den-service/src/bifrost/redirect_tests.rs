use super::availability_tests::{assert_failure, bear, key, Mock, LIVE};
use super::*;
use den_core::ModelAvailabilityFailureKind as Kind;
use sqlx::PgPool;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, AtomicU16, Ordering},
    thread,
};

struct Redirector {
    url: String,
    status: Arc<AtomicU16>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Redirector {
    fn new(destination: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let destination = format!("{destination}/models?key=PRIVATE");
        let status = Arc::new(AtomicU16::new(307));
        let stop = Arc::new(AtomicBool::new(false));
        let (statuses, stopping) = (status.clone(), stop.clone());
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
                        let mut bytes = Vec::new();
                        let mut buffer = [0; 1024];
                        while bytes.len() < 8192 {
                            let Ok(count) = stream.read(&mut buffer) else {
                                break;
                            };
                            if count == 0 {
                                break;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }
                        let status = statuses.load(Ordering::Acquire);
                        let _ = write!(stream, "HTTP/1.1 {status} Redirect\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("redirect mock accept: {error}"),
                }
            }
        });
        Self {
            url,
            status,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Redirector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn virtual_key_header_is_sensitive_and_invalid_header_values_are_safe_typed_failures() {
    let header = sensitive_virtual_key_header("sk-bf-PRIVATE").unwrap();
    assert!(header.is_sensitive());
    assert!(!format!("{header:?}").contains("PRIVATE"));
    assert_failure(
        sensitive_virtual_key_header("sk-bf-PRIVATE\r\nx-extra: leak"),
        Kind::VirtualKeyRejected,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_origin_redirects_never_receive_virtual_keys_or_authorize_catalog_selection(
    pool: PgPool,
) {
    let destination = Mock::new(200, LIVE);
    let redirector = Redirector::new(&destination.config().llm_api_url);
    let mut config = destination.config();
    config.llm_api_url.clone_from(&redirector.url);
    let client = BifrostClient::new(&config);
    for status in [302, 307, 308] {
        redirector.status.store(status, Ordering::Release);
        assert_failure(
            client
                .list_available_models_with_virtual_key(Some("sk-bf-secret"))
                .await,
            Kind::CatalogUnavailable,
        );
    }
    let bear_id = bear(&pool, "catalog-redirect").await;
    key(&pool, bear_id, "sk-bf-secret", &config).await;
    assert_failure(
        client
            .validate_bear_model_selection(
                &pool,
                bear_id,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
            )
            .await,
        Kind::CatalogUnavailable,
    );
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
    assert_eq!(
        destination.count(),
        0,
        "redirect destination must never be contacted, with or without a virtual key"
    );
}
