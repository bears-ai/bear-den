use super::*;
use axum::body::Body;

#[tokio::test]
async fn slow_stream_and_chunked_oversize_are_bounded() {
    let started = Arc::new(tokio::sync::Notify::new());
    let handler_started = started.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().fallback(get(move || {
        let started = handler_started.clone();
        async move {
            started.notify_one();
            Body::from_stream(futures::stream::once(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<_, std::convert::Infallible>("late-body")
            }))
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (authority, resolver) = fixture();
    let surface = authority.snapshot.surface;
    let operation = tokio::spawn(async move {
        execute(
            &authority,
            &resolver,
            surface,
            http::Transport::Loopback(address),
        )
        .await
    });
    started.notified().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert_eq!(
        operation.await.unwrap().err(),
        Some(RepositoryError::Timeout)
    );
    tokio::time::resume();
    server.abort();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().fallback(get(|| async {
        Body::from_stream(futures::stream::iter([
            Ok::<_, std::convert::Infallible>("x".repeat(8 * 1024)),
            Ok("x".repeat(8 * 1024 + 1)),
        ]))
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (authority, resolver) = fixture();
    assert_eq!(
        execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address)
        )
        .await
        .err(),
        Some(RepositoryError::ResponseTooLarge)
    );
    server.abort();
}

/// Test-only environment changes are restored even on assertion failure. Every
/// HTTP path in this test binary uses no_proxy; no production process is spawned.
struct ProxyEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl ProxyEnvironment {
    fn new(proxy: &str) -> Self {
        let mut prior = Vec::new();
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            prior.push((name, std::env::var_os(name)));
            std::env::set_var(
                name,
                if matches!(name, "NO_PROXY" | "no_proxy") {
                    ""
                } else {
                    proxy
                },
            );
        }
        Self(prior)
    }
}
impl Drop for ProxyEnvironment {
    fn drop(&mut self) {
        for (name, prior) in &self.0 {
            if let Some(value) = prior {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

#[tokio::test]
async fn ambient_proxy_cannot_receive_authentication() {
    let (proxy_address, proxy, proxy_task) = server(StatusCode::OK, TOKEN.into()).await;
    let _environment = ProxyEnvironment::new(&format!("http://{proxy_address}"));
    let (address, provider, task) = server(StatusCode::OK, response()).await;
    let (authority, resolver) = fixture();
    assert!(execute(
        &authority,
        &resolver,
        authority.snapshot.surface,
        http::Transport::Loopback(address)
    )
    .await
    .is_ok());
    assert!(provider.authenticated.load(Ordering::SeqCst));
    assert_eq!(proxy.calls.load(Ordering::SeqCst), 0);
    task.abort();
    proxy_task.abort();
}
