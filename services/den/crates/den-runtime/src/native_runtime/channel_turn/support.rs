use super::super::*;
use crate::{
    agent_loop::source_admission::tests::fixture,
    native_runtime::{RuntimeToolInvocation, RuntimeToolInvoker},
};
use den_core::config::Config;
use den_memory::MemoryStoreManager;
use den_service::bears::db;
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

pub(super) enum Reply {
    Text(&'static str),
    Tools,
    PartialTools,
    Empty,
    Failure,
}
pub(super) struct Provider {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Provider {
    pub fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let mut replies: VecDeque<_> = replies.into();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let Some((headers, body)) = read_request(&mut stream) else {
                    continue;
                };
                let (mime, body) = if headers.starts_with("GET /v1/models") {
                    ("application/json",json!({"data":[{"id":"openai/gpt-4.1","owned_by":"openai","context_length":128000,"supported_parameters":["tools"],"supported_methods":["chat_completion"]}],"next_page_token":null}).to_string())
                } else {
                    assert!(
                        headers.starts_with("POST /v1/chat/completions"),
                        "{headers}"
                    );
                    captured
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(&body).unwrap());
                    let reply = replies.pop_front().expect("unexpected model request");
                    ("text/event-stream", sse(reply))
                };
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8(bytes[..end].to_vec()).ok()?;
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
        if bytes.len() >= end + 4 + length {
            return Some((headers, bytes[end + 4..end + 4 + length].to_vec()));
        }
    }
}
fn sse(reply: Reply) -> String {
    let (delta,finish)=match reply {
        Reply::Text(text)=>(json!({"role":"assistant","content":text}),"stop"),
        Reply::Empty=>(json!({"role":"assistant","content":""}),"stop"),
        Reply::Failure=>return "data: {\"error\":{\"message\":\"fixture provider failed\",\"type\":\"server_error\",\"code\":\"server_error\"}}\n\ndata: [DONE]\n\n".into(),
        Reply::Tools=>(json!({"role":"assistant","tool_calls":[{"index":0,"id":"completed-call","type":"function","function":{"name":"session_info","arguments":"{}"}}]}),"tool_calls"),
        Reply::PartialTools=>(json!({"role":"assistant","content":"Working…","tool_calls":[{"index":0,"id":"completed-call","type":"function","function":{"name":"session_info","arguments":"{\"batch\":true}"}},{"index":1,"id":"pending-call","type":"function","function":{"name":"session_info","arguments":"{\"batch\":true}"}}]}),"tool_calls"),
    };
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
        json!({"id":"fixture","choices":[{"index":0,"delta":{},"finish_reason":finish}]})
    )
}

#[derive(Default)]
pub(super) struct Invoker {
    pub calls: AtomicUsize,
    pub pending: tokio::sync::Notify,
    pub completed_call: Mutex<Option<String>>,
    pub pending_call: Mutex<Option<String>>,
}
#[async_trait::async_trait]
impl RuntimeToolInvoker for Invoker {
    async fn invoke(&self, invocation: RuntimeToolInvocation) -> Result<Value, DenError> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        // The runtime may reorder independent calls in a provider batch. Block
        // the second actual execution, not a particular provider call ID.
        if index == 1 && invocation.arguments["batch"] == true {
            *self.pending_call.lock().unwrap() = Some(invocation.tool_call_id.as_str().into());
            self.pending.notify_one();
            std::future::pending::<()>().await;
        }
        *self.completed_call.lock().unwrap() = Some(invocation.tool_call_id.as_str().into());
        Ok(json!({"result":"completed tool evidence"}))
    }
}
pub(super) struct Fixture {
    pub source: crate::agent_loop::AgentLoopSession,
    pub canonical: Uuid,
    pub config: Config,
    pub stores: MemoryStoreManager,
    pub provider: Provider,
    pub invoker: Arc<Invoker>,
}
impl Fixture {
    pub async fn new(pool: &PgPool, replies: Vec<Reply>) -> Self {
        let (source, canonical, _) = fixture(pool).await;
        let provider = Provider::new(replies);
        let mut config = Config::test_stub();
        config.llm_api_url = provider.url.clone();
        config.den_secret_encryption_key = "channel-fixture-secret".into();
        config.bear_sqlite_data_dir = std::env::temp_dir()
            .join(format!("channel-fixture-{}", Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        db::set_bear_bifrost_virtual_key(
            pool,
            source.bear_id,
            Some("channel-test"),
            Some("channel-test"),
            Some("sk-fixture-key"),
            &config.den_secret_encryption_key,
        )
        .await
        .unwrap();
        let stores = MemoryStoreManager::new(&config);
        Self {
            source,
            canonical,
            config,
            stores,
            provider,
            invoker: Arc::new(Invoker::default()),
        }
    }
    pub async fn start(&self, pool: &PgPool, text: &str) -> NativeChannelTurn {
        let lease = ConversationTurnLease::acquire(pool, self.canonical)
            .await
            .unwrap();
        let request = Uuid::new_v4();
        let input = persist_channel_input(
            pool,
            self.source.bear_id,
            self.source.user_id.unwrap(),
            &lease,
            request,
            text,
        )
        .await
        .unwrap();
        let binding =
            den_service::bears::hats::turn_binding::NativeTurnSource::Conversation(self.canonical)
                .binding_id(self.source.bear_id.into());
        start_native_channel_turn(
            NativeWebChatTurnParams {
                deps: &super::super::super::NativeRuntimeDeps {
                    pool,
                    config: &self.config,
                    stores: &self.stores,
                },
                bear_id: self.source.bear_id,
                bear_slug: &self.source.bear_slug,
                turn_binding_id: &binding,
                user_id: self.source.user_id.unwrap(),
                username: None,
                membership_role: Some("member"),
                conversation_id: &self.source.conversation_id,
                session_id: &self.source.client_session_id,
                prompt: text,
                request_id: request,
                tool_invoker: self.invoker.clone(),
            },
            input,
            lease,
        )
        .await
        .unwrap()
    }
}
