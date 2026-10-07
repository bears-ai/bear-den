use super::*;
use crate::agent_loop::source_admission::tests::fixture;
use den_service::{
    bears::{db, model_configurations as configurations},
    conversation::persistence,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

struct CatalogMock {
    url: String,
    response: Arc<Mutex<(u16, String)>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl CatalogMock {
    fn new(body: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let response = Arc::new(Mutex::new((200_u16, body.to_string())));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_response = Arc::clone(&response);
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let Some(request) = read_request(&mut stream) else {
                            continue;
                        };
                        thread_requests.lock().unwrap().push(request);
                        let (status, body) = thread_response.lock().unwrap().clone();
                        let response = format!(
                            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len(),
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("catalog mock accept failed: {error}"),
                }
            }
        });
        Self {
            url: format!("http://{address}/v1"),
            response,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn reply(&self, status: u16, body: Value) {
        *self.response.lock().unwrap() = (status, body.to_string());
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for CatalogMock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while bytes.len() < 8192 {
        let count = stream.read(&mut buffer).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).ok();
        }
    }
    None
}

fn live_models(methods: Option<&[&str]>) -> Value {
    let mut model = json!({
        "id": "openai/gpt-5",
        "owned_by": "openai",
        "context_length": 128000,
        "supported_parameters": ["tools", "reasoning_effort"],
    });
    if let Some(methods) = methods {
        model["supported_methods"] = json!(methods);
    }
    json!({"data": [model], "next_page_token": null})
}

async fn config_with_key(pool: &PgPool, bear_id: BearId, mock: &CatalogMock) -> Config {
    let mut config = Config::test_stub();
    config.llm_api_url.clone_from(&mock.url);
    config.den_secret_encryption_key = "runtime-catalog-test-secret-key".into();
    db::set_bear_bifrost_virtual_key(
        pool,
        bear_id.as_uuid(),
        Some("vk-runtime-test"),
        Some("runtime-test"),
        Some("sk-bf-runtime-test"),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    config
}

#[test]
fn primary_transport_preference_is_context_owned_not_provider_inferred() {
    let pair = TurnExecutionOrigin::ArmatureConversation(den_core::ArmatureAvailability::Connected);
    let work = TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected);
    for origin in [pair, work] {
        assert_eq!(
            transport_preference(origin, None),
            PrimaryTransportPreference::ResponsesWhenUnknown
        );
    }
    assert_eq!(
        transport_preference(
            TurnExecutionOrigin::ChannelConversation,
            Some(ThinkingEffort::High)
        ),
        PrimaryTransportPreference::ResponsesWhenUnknown
    );
    assert_eq!(
        transport_preference(TurnExecutionOrigin::ChannelConversation, None),
        PrimaryTransportPreference::ChatCompletionsWhenUnknown
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn live_unknown_methods_use_responses_and_shared_client_reuses_catalog(pool: PgPool) {
    let (session, canonical, hat) = fixture(&pool).await;
    let bear_id = session.bear_id.into();
    super::tests::reasoning_support(&pool, Some(true)).await;
    let configuration = configurations::create(
        &pool,
        bear_id,
        "Careful",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    configurations::set_hat_override(&pool, bear_id, hat, Some(configuration.id))
        .await
        .unwrap();
    let primary = resolve_for_source(
        &pool,
        bear_id,
        NativeTurnSource::Conversation(canonical),
        "gpt-4.1",
    )
    .await
    .unwrap();
    let mock = CatalogMock::new(live_models(None));
    let config = config_with_key(&pool, bear_id, &mock).await;
    // Mirror startup injection: both layers receive clones of one state-owned Arc.
    let state_client = Arc::new(BifrostClient::new(&config));
    let runtime_client = Arc::clone(&state_client);
    let preference = transport_preference(session.origin, primary.thinking_effort);
    let style = execution_api_style_with_client(
        &runtime_client,
        &pool,
        &config,
        bear_id,
        &primary,
        preference,
    )
    .await
    .unwrap();
    assert_eq!(style, LlmApiStyle::ResponsesStream);
    assert_eq!(
        state_client
            .cached_bear_catalog_snapshot(bear_id.as_uuid())
            .unwrap()
            .resolve(&primary.model_handle)
            .unwrap()
            .supports_responses_api,
        None
    );
    for _ in 0..3 {
        assert_eq!(
            execution_api_style_with_client(
                &runtime_client,
                &pool,
                &config,
                bear_id,
                &primary,
                preference
            )
            .await
            .unwrap(),
            style
        );
    }
    assert_eq!(
        mock.request_count(),
        1,
        "runtime must retain the process client's catalog cache"
    );
    let request = crate::llm::ChatCompletionRequest {
        model: primary.model_handle.clone(),
        messages: vec![],
        tools: vec![crate::llm::LlmToolDefinition {
            name: "fs_read_text_file".into(),
            description: None,
            parameters: json!({}),
        }],
        stream: true,
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        thinking_effort: compatible_effort(style, true, primary.thinking_effort),
        telemetry: None,
    };
    assert_eq!(request.to_responses_body()["reasoning"]["effort"], "high");
    assert_eq!(request.model, primary.model_handle);
    let requests = mock.requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /v1/models?"));
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("x-bf-vk: sk-bf-runtime-test"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn successful_catalog_missing_selected_model_never_uses_pin_continuity(pool: PgPool) {
    let (session, _, hat) = fixture(&pool).await;
    let bear_id = session.bear_id.into();
    let primary =
        configurations::resolve_primary(&pool, bear_id, Some(hat), Some("gpt-5"), "gpt-4.1")
            .await
            .unwrap();
    let mock = CatalogMock::new(json!({"data": [], "next_page_token": null}));
    let config = config_with_key(&pool, bear_id, &mock).await;
    let client = BifrostClient::new(&config);
    let result = execution_api_style_with_client(
        &client,
        &pool,
        &config,
        bear_id,
        &primary,
        PrimaryTransportPreference::ResponsesWhenUnknown,
    )
    .await;
    assert!(
        matches!(result, Err(DenError::ValidationError(message)) if message.contains("missing from the Bifrost catalog"))
    );
    assert_eq!(mock.request_count(), 1);
    assert_eq!(primary.model_handle, "openai/gpt-5");
}

#[sqlx::test(migrations = "../../migrations")]
async fn real_catalog_outage_retains_explicit_pin_using_other_layers_shared_cache(pool: PgPool) {
    let (session, canonical, _) = fixture(&pool).await;
    let bear_id = session.bear_id.into();
    persistence::set_conversation_model_state(
        &pool,
        canonical,
        "explicit",
        Some("gpt-5"),
        Some("gpt-5"),
        None,
    )
    .await
    .unwrap();
    let primary = resolve_for_source(
        &pool,
        bear_id,
        NativeTurnSource::Conversation(canonical),
        "gpt-4.1",
    )
    .await
    .unwrap();
    let mock = CatalogMock::new(live_models(Some(&["chat_completion"])));
    let config = config_with_key(&pool, bear_id, &mock).await;
    let state_client = Arc::new(BifrostClient::new(&config));
    let runtime_client = Arc::clone(&state_client);
    state_client
        .refresh_bear_catalog_snapshot(&pool, bear_id.as_uuid(), &config.den_secret_encryption_key)
        .await
        .unwrap();
    mock.reply(503, json!({"error": "catalog temporarily unavailable"}));
    // Force the refresh that TTL expiry would perform. Feed its real failed HTTP
    // outcome into the same selector used by runtime; do not mutate the cache.
    let refresh = state_client
        .refresh_bear_catalog_snapshot(&pool, bear_id.as_uuid(), &config.den_secret_encryption_key)
        .await;
    assert!(refresh.is_err());
    let preference = PrimaryTransportPreference::ResponsesWhenUnknown;
    assert_eq!(
        catalog_api_style(&runtime_client, bear_id, &primary, preference, refresh).unwrap(),
        LlmApiStyle::ChatCompletionsStream,
        "cached false support must win; an isolated empty cache would choose Responses"
    );
    let requests_after_outage = mock.request_count();
    assert!(
        requests_after_outage > 1,
        "exercise actual loopback outage retries"
    );
    assert_eq!(
        execution_api_style_with_client(
            &runtime_client,
            &pool,
            &config,
            bear_id,
            &primary,
            preference
        )
        .await
        .unwrap(),
        LlmApiStyle::ChatCompletionsStream
    );
    assert_eq!(
        mock.request_count(),
        requests_after_outage,
        "failed refresh must retain the shared successful snapshot"
    );
    assert_eq!(primary.model_handle, "openai/gpt-5");

    let uncached_client = BifrostClient::new(&config);
    assert_eq!(
        catalog_api_style(
            &uncached_client,
            bear_id,
            &primary,
            preference,
            Err(DenError::System("catalog outage".into()))
        )
        .unwrap(),
        LlmApiStyle::ResponsesStream,
        "preserve BearWire's same-pin continuity even without usable cached metadata"
    );
    let mut inherited = primary.clone();
    inherited.source = PrimaryModelSource::DeploymentDefault;
    assert!(
        catalog_api_style(
            &runtime_client,
            bear_id,
            &inherited,
            preference,
            Err(DenError::System("catalog outage".into()))
        )
        .is_err(),
        "an outage does not authorize default/configuration fallback"
    );

    let mut unavailable = runtime_client
        .cached_bear_catalog_snapshot(bear_id.as_uuid())
        .unwrap();
    unavailable
        .models
        .get_mut(&primary.model_handle)
        .unwrap()
        .available = false;
    assert!(matches!(
        catalog_api_style(
            &runtime_client,
            bear_id,
            &primary,
            preference,
            Ok(unavailable)
        ),
        Err(DenError::ValidationError(_))
    ));
}
