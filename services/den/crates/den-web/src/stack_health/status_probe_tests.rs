//! Upstream config bodies are not safe public status evidence.

use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn response(status: &str, body: &str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "status probe did not reach the test server"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("test status server: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 8192];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
    });
    (url, server)
}

#[tokio::test]
async fn management_status_retains_failure_state_without_exposing_config_bodies() {
    for (status, body, evidence) in [
        (
            "500 Internal Server Error",
            "{\"password\":\"UPSTREAM-SECRET\"}",
            "HTTP 500",
        ),
        (
            "200 OK",
            "not JSON; password=UPSTREAM-SECRET",
            "JSON parse failed",
        ),
        (
            "403 Forbidden",
            "{\"error\":\"Authentication is not enabled\",\"password\":\"UPSTREAM-SECRET\"}",
            "management authentication is not enabled",
        ),
    ] {
        let (url, server) = response(status, body);
        let check = check_bifrost_management_auth(&url).await;
        server.join().unwrap();
        assert_eq!(check.state, CheckState::Fail);
        assert!(check.detail.contains(evidence));
        assert!(!check.detail.contains("UPSTREAM-SECRET"));
    }
}
