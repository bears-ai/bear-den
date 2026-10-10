//! Mutable authenticated catalog fixture; never calls an external provider.

use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub(crate) struct CatalogFixture {
    pub url: String,
    reply: Arc<Mutex<(u16, String)>>,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl CatalogFixture {
    pub fn start(models: &[&str], virtual_key: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let reply = Arc::new(Mutex::new((200, model_body(models))));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let expected_key = format!("x-bf-vk: {virtual_key}");
        let server_reply = reply.clone();
        let server_requests = requests.clone();
        let server_stop = stop.clone();
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        respond(stream, &expected_key, &server_reply, &server_requests);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("catalog fixture accept: {error}"),
                }
            }
        });
        Self {
            url,
            reply,
            requests,
            stop,
            server: Some(server),
        }
    }

    pub fn set_models(&self, models: &[&str]) {
        *self.reply.lock().unwrap() = (200, model_body(models));
    }

    pub fn set_status(&self, status: u16) {
        *self.reply.lock().unwrap() = (
            status,
            "PRIVATE_PROVIDER_RESPONSE https://private.test/?key=SECRET".into(),
        );
    }

    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn model_body(models: &[&str]) -> String {
    json!({"data": models.iter().map(|model| json!({
        "id": model, "owned_by": "openai", "context_length": 128000,
        "max_output_tokens": 4096, "supported_parameters": ["tools"],
        "supported_methods": ["chat_completion"],
    })).collect::<Vec<_>>()})
    .to_string()
}

fn respond(
    mut stream: TcpStream,
    expected_key: &str,
    reply: &Mutex<(u16, String)>,
    requests: &AtomicUsize,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    reader.read_line(&mut request).unwrap();
    assert!(
        request.starts_with("GET /models"),
        "no inference should reach the catalog fixture"
    );
    let mut authenticated = false;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        authenticated |= line.trim() == expected_key;
    }
    assert!(
        authenticated,
        "catalog request must use this Bear's virtual key"
    );
    requests.fetch_add(1, Ordering::SeqCst);
    let (status, body) = reply.lock().unwrap().clone();
    write!(stream, "HTTP/1.1 {status} Fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
}

impl Drop for CatalogFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
    }
}
