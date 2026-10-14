use super::Fixture;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, Uri},
    response::Response,
    routing::any,
    Router,
};
use sqlx::PgPool;

async fn endpoint(
    State(pool): State<PgPool>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    super::super::serve(&pool, None, &method, &uri, &headers, &body).await
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres, test-only JWT_SECRET, and CABINET_MCP_SDK_PROBE compiled from sdk_probe.rs"]
async fn existing_rmcp_sdk_uses_authenticated_stateless_streamable_http(pool: PgPool) {
    let executable = std::env::var("CABINET_MCP_SDK_PROBE")
        .expect("compile sdk_probe.rs against existing bear-armature SDK artifacts first");
    let fixture = Fixture::new(&pool).await;
    let router = Router::new()
        .route("/mcp", any(endpoint))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(executable)
            .args([&url, fixture.page.as_str()])
            .env("MCP_TEST_TOKEN", fixture.token)
            .output()
            .expect("run standalone SDK probe")
    })
    .await
    .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "SDK probe failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}
