use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

use super::*;

fn credential(secret: &str) -> BearEmbeddingCredential {
    BearEmbeddingCredential::from_server_secret(secret.to_string()).unwrap()
}

fn server(status: u16, body: &str, expected_key: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let body = body.to_string();
    let expected_key = expected_key.to_string();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "embedding request never arrived");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
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
        assert!(headers.starts_with("POST /v1/embeddings HTTP/1.1"));
        assert!(
            headers.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("x-bf-vk") && value.trim() == expected_key
                })
            }),
            "required Bear credential was not sent"
        );
        assert!(!headers.to_ascii_lowercase().contains("authorization:"));
        write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        request
    });
    (url, handle)
}

#[test]
fn disabled_when_no_inference_substrate() {
    let cfg = Config::test_stub();
    let client = EmbeddingClient::new(&cfg, credential("fixture-key"));
    assert!(!client.is_enabled());
    assert_eq!(client.model(), "openai/text-embedding-3-small");
    assert_eq!(client.dimensions(), 1536);
}

#[test]
fn request_body_includes_model_input_and_dimensions() {
    let inputs = vec!["hello".to_string(), "world".to_string()];
    let body = embedding_request_body("openai/text-embedding-3-small", &inputs, 1536);
    assert_eq!(body["model"], "openai/text-embedding-3-small");
    assert_eq!(body["input"], json!(["hello", "world"]));
    assert_eq!(body["dimensions"], 1536);
    assert!(embedding_request_body("m", &inputs, 0)
        .get("dimensions")
        .is_none());
}

#[test]
fn parses_and_orders_vectors_by_index() {
    let value = json!({"data": [
        {"index": 1, "embedding": [0.5, 0.5]},
        {"index": 0, "embedding": [0.1, 0.2]}
    ]});
    assert_eq!(
        parse_embedding_response(&value, 2, 2).unwrap(),
        vec![vec![0.1, 0.2], vec![0.5, 0.5]]
    );
}

#[test]
fn rejects_dimension_and_count_mismatch() {
    let value = json!({"data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}]});
    assert!(parse_embedding_response(&value, 1, 2)
        .unwrap_err()
        .to_string()
        .contains("does not match configured dimensions"));
    assert!(parse_embedding_response(&value, 2, 3)
        .unwrap_err()
        .to_string()
        .contains("expected 2"));
}

#[test]
fn credentials_are_required_validated_and_redacted() {
    for secret in ["", " \t", "CANARY\r\nheader: secret"] {
        let error = BearEmbeddingCredential::from_server_secret(secret.into()).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("CANARY"));
    }
    let key = credential("KEY_CANARY");
    assert!(key.header().is_sensitive());
    assert!(!format!("{key:?} {:?}", key.header()).contains("KEY_CANARY"));
}

#[tokio::test]
async fn embeds_queries_and_batches_using_only_bear_virtual_key() {
    for inputs in [
        vec!["query".to_string()],
        vec!["passage a".to_string(), "passage b".to_string()],
    ] {
        let body = json!({"data": inputs.iter().enumerate().map(|(index, _)| json!({"index": index, "embedding": [0.1, 0.2]})).collect::<Vec<_>>()});
        let (url, handle) = server(200, &body.to_string(), "BEAR_KEY_CANARY");
        let mut config = Config::test_stub();
        config.llm_api_url = url;
        config.llm_api_key = "GLOBAL_ADMIN_CANARY".into();
        config.embedding_dimensions = 2;
        let client = EmbeddingClient::new(&config, credential("BEAR_KEY_CANARY"));
        assert_eq!(client.embed(&inputs).await.unwrap().len(), inputs.len());
        let request = handle.join().unwrap();
        assert!(!request.contains("GLOBAL_ADMIN_CANARY"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["input"], json!(inputs));
        assert!(!body.to_string().contains("BEAR_KEY_CANARY"));
    }
}

#[tokio::test]
async fn provider_errors_and_invalid_json_never_echo_body_input_model_keys_or_url() {
    for status in [401, 403, 429, 500, 200] {
        let (url, handle) = server(
            status,
            "BODY_CANARY INPUT_CANARY MODEL_CANARY BEAR_KEY_CANARY GLOBAL_ADMIN_CANARY",
            "BEAR_KEY_CANARY",
        );
        let mut config = Config::test_stub();
        config.llm_api_url = url.clone();
        config.embedding_model = "MODEL_CANARY".into();
        let client = EmbeddingClient::new(&config, credential("BEAR_KEY_CANARY"));
        let error = client.embed_one("INPUT_CANARY").await.unwrap_err();
        let rendered = format!("{error} {error:?}");
        for secret in [
            "BODY_CANARY",
            "INPUT_CANARY",
            "MODEL_CANARY",
            "BEAR_KEY_CANARY",
            "GLOBAL_ADMIN_CANARY",
            &url,
        ] {
            assert!(!rendered.contains(secret));
        }
        if matches!(status, 401 | 403) {
            assert!(rendered.contains("credential denied"));
            assert!(rendered.contains(&status.to_string()));
        }
        handle.join().unwrap();
    }
}

#[tokio::test]
async fn transport_errors_do_not_echo_endpoint_or_credentials() {
    let mut config = Config::test_stub();
    config.llm_api_url = "not-a-url/URL_CANARY".into();
    let client = EmbeddingClient::new(&config, credential("KEY_CANARY"));
    let error = client.embed_one("INPUT_CANARY").await.unwrap_err();
    assert!(
        matches!(&error, DenError::System(message) if message == "embeddings transport failed")
    );
    assert!(!format!("{error:?}").contains("CANARY"));
}
