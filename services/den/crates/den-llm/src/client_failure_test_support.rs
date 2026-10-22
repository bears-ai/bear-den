use std::{
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use tracing::{
    field::{Field, Visit},
    span::{Attributes, Id, Record},
    Event, Metadata, Subscriber,
};

use super::super::*;

pub const CANARIES: [&str; 7] = [
    "PERSONAL_CANARY",
    "HEADER_SECRET_CANARY",
    "URL_SECRET_CANARY",
    "URL_USER_CANARY",
    "TYPE_CANARY",
    "COOKIE_CANARY",
    "REASON_CANARY",
];

pub fn assert_safe(value: &str) {
    for canary in CANARIES {
        assert!(
            !value.contains(canary),
            "provider data escaped boundary: {canary}"
        );
    }
}

pub fn expect_provider(error: DenError) -> ProviderFailure {
    assert_safe(&format!("{error} {error:?}"));
    let DenError::Provider(failure) = error else {
        panic!("expected a typed provider failure")
    };
    assert_safe(&serde_json::to_string(&failure).unwrap());
    failure
}

pub fn canary_body(message: &str) -> String {
    json!({
        "error": {
            "message": format!("{message} PERSONAL_CANARY Authorization: Bearer HEADER_SECRET_CANARY https://URL_USER_CANARY:URL_SECRET_CANARY@upstream.invalid?key=URL_SECRET_CANARY"),
            "type": "TYPE_CANARY",
            "request": { "messages": ["PERSONAL_CANARY"], "headers": { "x-bf-vk": "HEADER_SECRET_CANARY", "cookie": "COOKIE_CANARY" } }
        }
    }).to_string()
}

pub fn request() -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "openai/gpt-5.5".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: Some("PERSONAL_CANARY".to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: Vec::new(),
        stream: true,
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        thinking_effort: None,
        telemetry: Some(LlmRequestTelemetry {
            request_id: Some("request-fixture".to_string()),
            conversation_id: Some("conversation-fixture".to_string()),
            bifrost_virtual_key: Some("HEADER_SECRET_CANARY".to_string()),
            operation: Some(LlmOperation::AgentTurn),
            ..Default::default()
        }),
    }
}

pub fn client(url: &str) -> LlmClient {
    let mut config = Config::test_stub();
    config.llm_api_url = url.to_string();
    LlmClient::new(&config)
}

pub async fn call(
    client: &LlmClient,
    style: LlmApiStyle,
    request: &ChatCompletionRequest,
) -> Result<Response, DenError> {
    match style {
        LlmApiStyle::ChatCompletionsStream => client.chat_completions_stream(request).await,
        LlmApiStyle::ResponsesStream => client.responses_stream(request).await,
    }
}

pub enum Reply {
    Http {
        status: u16,
        body: String,
        missing_bytes: usize,
    },
    Disconnect,
}

impl Reply {
    pub fn http(status: u16, body: impl Into<String>) -> Self {
        Self::Http {
            status,
            body: body.into(),
            missing_bytes: 0,
        }
    }
}

pub struct Server {
    pub url: String,
    task: thread::JoinHandle<Vec<String>>,
}

impl Server {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://URL_USER_CANARY:URL_SECRET_CANARY@{}/v1",
            listener.local_addr().unwrap()
        );
        let task = thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "expected fixture request did not arrive"
                            );
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("fixture accept failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                requests.push(read_request(&mut stream));
                if let Reply::Http {
                    status,
                    body,
                    missing_bytes,
                } = reply
                {
                    // Failure-body cap tests may drop the response before the fixture
                    // finishes writing; neither side needs the remaining bytes.
                    let _ = write!(stream,
                        "HTTP/1.1 {status} REASON_CANARY\r\nContent-Type: application/json\r\nContent-Length: {}\r\nSet-Cookie: COOKIE_CANARY\r\nLocation: https://upstream.invalid?key=URL_SECRET_CANARY\r\nConnection: close\r\n\r\n{body}",
                        body.len() + missing_bytes);
                }
            }
            requests
        });
        Self { url, task }
    }

    pub fn finish(self) -> Vec<String> {
        self.task.join().expect("fixture server")
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "request ended before its body");
        bytes.extend_from_slice(&buffer[..count]);
        let request = String::from_utf8_lossy(&bytes);
        if let Some((headers, body)) = request.split_once("\r\n\r\n") {
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if body.len() >= length {
                return String::from_utf8(bytes).unwrap();
            }
        }
        assert!(bytes.len() < 256 * 1024, "unexpected fixture request size");
    }
}

pub fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .split_once("\r\n\r\n")
        .unwrap()
        .0
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
}

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<String>>);

impl Capture {
    pub fn output(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target().starts_with("den_llm::client")
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl Visit for Fields<'_> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                write!(self.0, "{}={value:?};", field.name()).unwrap();
            }
        }
        event.record(&mut Fields(&mut self.0.lock().unwrap()));
    }
}
