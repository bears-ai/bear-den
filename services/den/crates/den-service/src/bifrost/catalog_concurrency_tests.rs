use super::availability_tests::{assert_failure, bear, key, LIVE, PRIVATE};
use super::*;
use crate::bears::model_configurations::PrimaryModelSource;
use den_core::ModelAvailabilityFailureKind as Kind;
use sqlx::PgPool;
use std::{
    future::{poll_fn, Future},
    io::{Read, Write},
    net::TcpListener,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex,
    },
    task::Poll,
    thread,
};

struct DelayedMock {
    url: String,
    reply: Arc<Mutex<(u16, String)>>,
    requests: Arc<Mutex<Vec<String>>>,
    release: Arc<(Mutex<bool>, Condvar)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DelayedMock {
    fn new(old_status: u16, old_body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let reply = Arc::new(Mutex::new((200, LIVE.to_owned())));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (responses, records, gate, stopping) = (
            reply.clone(),
            requests.clone(),
            release.clone(),
            stop.clone(),
        );
        let old_body = old_body.to_owned();
        let worker = thread::spawn(move || {
            let mut responders = Vec::new();
            let mut index = 0;
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        index += 1;
                        let delayed = index == 2;
                        let (responses, records, gate, old_body) = (
                            responses.clone(),
                            records.clone(),
                            gate.clone(),
                            old_body.clone(),
                        );
                        responders.push(thread::spawn(move || {
                            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                            let mut bytes = Vec::new();
                            let mut buffer = [0; 1024];
                            while bytes.len() < 8192 {
                                let Ok(count) = stream.read(&mut buffer) else { return };
                                if count == 0 { return; }
                                bytes.extend_from_slice(&buffer[..count]);
                                if bytes.windows(4).any(|window| window == b"\r\n\r\n") { break; }
                            }
                            records.lock().unwrap().push(String::from_utf8_lossy(&bytes).into_owned());
                            let (status, body) = if delayed {
                                let (mutex, wake) = &*gate;
                                let mut released = mutex.lock().unwrap();
                                while !*released { released = wake.wait(released).unwrap(); }
                                (old_status, old_body)
                            } else { responses.lock().unwrap().clone() };
                            let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
            for responder in responders {
                responder.join().unwrap();
            }
        });
        Self {
            url,
            reply,
            requests,
            release,
            stop,
            worker: Some(worker),
        }
    }

    fn config(&self) -> Config {
        let mut config = Config::test_stub();
        config.llm_api_url.clone_from(&self.url);
        config.den_secret_encryption_key = "catalog-concurrency-encryption".into();
        config
    }

    fn respond(&self, status: u16, body: &str) {
        *self.reply.lock().unwrap() = (status, body.to_owned());
    }

    async fn wait_for_delayed_request(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.requests.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    fn release_old(&self) {
        let (mutex, wake) = &*self.release;
        *mutex.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl Drop for DelayedMock {
    fn drop(&mut self) {
        self.release_old();
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

async fn assert_queued<F: Future>(mut future: Pin<&mut F>, mock: &DelayedMock) {
    poll_fn(|context| {
        assert!(
            future.as_mut().poll(context).is_pending(),
            "check must wait behind the held refresh"
        );
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        mock.requests.lock().unwrap().len(),
        2,
        "queued check must not start HTTP or reuse the positive cache"
    );
}

async fn warm(pool: &PgPool, mock: &DelayedMock) -> (BifrostClient, Config, Uuid) {
    let config = mock.config();
    let bear_id = bear(pool, "catalog-concurrent").await;
    key(pool, bear_id, "sk-bf-original", &config).await;
    let client = BifrostClient::new(&config);
    client
        .refresh_bear_catalog_snapshot(pool, bear_id, &config.den_secret_encryption_key)
        .await
        .unwrap();
    (client, config, bear_id)
}

fn old_execution(
    client: &BifrostClient,
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
) -> tokio::task::JoinHandle<Result<Option<BifrostCatalogEntry>, DenError>> {
    let (client, pool, secret) = (
        client.clone(),
        pool.clone(),
        config.den_secret_encryption_key.clone(),
    );
    tokio::spawn(async move {
        client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                "openai/gpt-6-sol",
                &secret,
                PrimaryModelSource::ConversationPin,
            )
            .await
    })
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_successful_checks_both_fetch_fresh_and_publish_the_latest_result(pool: PgPool) {
    let latest = r#"{"data":[{"id":"openai/gpt-6-sol","context_length":64000,"supported_methods":["responses"]}]}"#;
    let mock = DelayedMock::new(200, latest);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    assert_eq!(
        client
            .cached_bear_catalog_snapshot(bear_id)
            .unwrap()
            .require_available_model("openai/gpt-6-sol")
            .unwrap()
            .context_window,
        0
    );
    mock.respond(200, latest);
    let first = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    let mut second = std::pin::pin!(client.validate_bear_model_execution(
        &pool,
        bear_id,
        "openai/gpt-6-sol",
        &config.den_secret_encryption_key,
        PrimaryModelSource::BearDefault,
    ));
    assert_queued(second.as_mut(), &mock).await;
    mock.release_old();
    let first = first.await.unwrap().unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert!(first.available && second.available);
    assert_eq!(first.context_window, 64_000);
    assert_eq!(second.context_window, 64_000);
    assert_eq!(
        client
            .cached_bear_catalog_snapshot(bear_id)
            .unwrap()
            .require_available_model("openai/gpt-6-sol")
            .unwrap()
            .context_window,
        64_000
    );
    assert_eq!(
        mock.requests.lock().unwrap().len(),
        3,
        "warm plus two actual fresh authenticated requests"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn delayed_old_positive_cannot_overwrite_new_missing_snapshot_or_enter_pin_fallback(
    pool: PgPool,
) {
    let mock = DelayedMock::new(200, LIVE);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    let old = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    assert!(
        client.cached_bear_catalog_snapshot(bear_id).is_none(),
        "pending refresh hides prior authority"
    );
    mock.respond(200, r#"{"data":[]}"#);
    let mut newer = std::pin::pin!(client.validate_bear_model_execution(
        &pool,
        bear_id,
        "openai/gpt-6-sol",
        &config.den_secret_encryption_key,
        PrimaryModelSource::ConversationPin,
    ));
    assert_queued(newer.as_mut(), &mock).await;
    mock.release_old();
    assert!(old.await.unwrap().unwrap().unwrap().available);
    assert_failure(newer.await, Kind::ModelMissing);
    let cached = client
        .bear_catalog_snapshot(&pool, bear_id, &config.den_secret_encryption_key)
        .await
        .unwrap();
    assert_failure(
        cached.require_available_model("openai/gpt-6-sol"),
        Kind::ModelMissing,
    );
    assert!(client
        .cached_bear_catalog_snapshot(bear_id)
        .unwrap()
        .models
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn delayed_old_positive_cannot_repopulate_a_rejected_key_tombstone(pool: PgPool) {
    let mock = DelayedMock::new(200, LIVE);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    let old = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    key(&pool, bear_id, "sk-bf-rotated", &config).await;
    mock.respond(403, PRIVATE);
    let mut newer = std::pin::pin!(client.validate_bear_model_execution(
        &pool,
        bear_id,
        "openai/gpt-6-sol",
        &config.den_secret_encryption_key,
        PrimaryModelSource::ConversationPin,
    ));
    assert_queued(newer.as_mut(), &mock).await;
    mock.release_old();
    assert_failure(old.await.unwrap(), Kind::VirtualKeyRejected);
    assert_failure(newer.await, Kind::VirtualKeyRejected);
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
    assert_failure(
        client
            .bear_catalog_snapshot(&pool, bear_id, &config.den_secret_encryption_key)
            .await,
        Kind::VirtualKeyRejected,
    );
    assert_failure(
        client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
                PrimaryModelSource::ConversationPin,
            )
            .await,
        Kind::VirtualKeyRejected,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_rotation_without_another_refresh_rejects_delayed_publication(pool: PgPool) {
    let mock = DelayedMock::new(200, LIVE);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    let old = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    key(&pool, bear_id, "sk-bf-rotated", &config).await;
    mock.release_old();
    assert_failure(old.await.unwrap(), Kind::VirtualKeyRejected);
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
    mock.respond(200, r#"{"data":[]}"#);
    assert_failure(
        client
            .validate_bear_model_execution(
                &pool,
                bear_id,
                "openai/gpt-6-sol",
                &config.den_secret_encryption_key,
                PrimaryModelSource::ConversationPin,
            )
            .await,
        Kind::ModelMissing,
    );
    assert!(mock
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .to_ascii_lowercase()
        .contains("x-bf-vk: sk-bf-rotated"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delayed_old_rejection_cannot_invalidate_a_new_rotated_positive_catalog(pool: PgPool) {
    let mock = DelayedMock::new(401, PRIVATE);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    let old = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    key(&pool, bear_id, "sk-bf-rotated", &config).await;
    let mut newer = std::pin::pin!(client.validate_bear_model_execution(
        &pool,
        bear_id,
        "openai/gpt-6-sol",
        &config.den_secret_encryption_key,
        PrimaryModelSource::ConversationPin,
    ));
    assert_queued(newer.as_mut(), &mock).await;
    mock.release_old();
    assert_failure(old.await.unwrap(), Kind::VirtualKeyRejected);
    assert!(newer.await.unwrap().unwrap().available);
    assert!(client
        .cached_bear_catalog_snapshot(bear_id)
        .unwrap()
        .require_available_model("openai/gpt-6-sol")
        .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn ttl_read_during_a_pending_refresh_cannot_authorize_the_prior_positive_cache(pool: PgPool) {
    let mock = DelayedMock::new(200, LIVE);
    let (client, config, bear_id) = warm(&pool, &mock).await;
    let old = old_execution(&client, &pool, &config, bear_id);
    mock.wait_for_delayed_request().await;
    mock.respond(403, PRIVATE);
    let mut newer = std::pin::pin!(client.bear_catalog_snapshot(
        &pool,
        bear_id,
        &config.den_secret_encryption_key,
    ));
    assert_queued(newer.as_mut(), &mock).await;
    mock.release_old();
    assert!(old.await.unwrap().unwrap().unwrap().available);
    assert_failure(newer.await, Kind::VirtualKeyRejected);
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
}
