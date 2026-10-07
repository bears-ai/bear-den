//! Observable local provider with a completion gate for deterministic races.

use serde_json::Value;
use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Default)]
struct ProviderState {
    requests: AtomicUsize,
    completions: AtomicUsize,
    paused: AtomicBool,
    stop: AtomicBool,
    bodies: Mutex<Vec<Value>>,
}

pub(super) struct InferenceFixture {
    pub url: String,
    state: Arc<ProviderState>,
    server: Option<JoinHandle<()>>,
}

impl InferenceFixture {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind inference fixture");
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(ProviderState::default());
        let server_state = state.clone();
        let server = thread::spawn(move || {
            let mut workers = Vec::new();
            while !server_state.stop.load(Ordering::SeqCst) {
                let (stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("inference fixture accept failed: {error}"),
                };
                let state = server_state.clone();
                workers.push(thread::spawn(move || respond(stream, &state)));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self {
            url,
            state,
            server: Some(server),
        }
    }

    pub fn request_count(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }
    pub fn completion_count(&self) -> usize {
        self.state.completions.load(Ordering::SeqCst)
    }
    pub fn pause_completions(&self) {
        self.state.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume_completions(&self) {
        self.state.paused.store(false, Ordering::SeqCst);
    }
    pub fn completion_bodies(&self) -> Vec<Value> {
        self.state.bodies.lock().unwrap().clone()
    }
}

fn respond(mut stream: TcpStream, state: &ProviderState) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = super::super::read_http_request(&mut stream);
    state.requests.fetch_add(1, Ordering::SeqCst);
    let (content_type, body) = if request.starts_with("GET /models") {
        (
            "application/json",
            r#"{"data":[{"id":"openai/bearwire-test-model","name":"BearWire test model","owned_by":"openai","context_length":128000,"max_output_tokens":4096,"supported_parameters":["tools"],"supported_methods":["chat_completion"]}]}"#,
        )
    } else {
        assert!(
            request.starts_with("POST /chat/completions "),
            "unexpected request: {request}"
        );
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        state
            .bodies
            .lock()
            .unwrap()
            .push(serde_json::from_str(body).unwrap());
        state.completions.fetch_add(1, Ordering::SeqCst);
        while state.paused.load(Ordering::SeqCst) && !state.stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        if state.stop.load(Ordering::SeqCst) {
            return;
        }
        (
            "text/event-stream",
            concat!(
                "data: {\"id\":\"chatcmpl-history-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"live source admitted\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"chatcmpl-history-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ),
        )
    };
    write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
}

impl Drop for InferenceFixture {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        self.resume_completions();
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}
