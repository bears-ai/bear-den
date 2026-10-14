//! Standalone interoperability probe, not a den-web Cargo target/dependency.
//! Compile against the existing bear-armature SDK artifacts with `rustc --extern`.
//! The ignored SDK test supplies a loopback URL, page ref, and test-only token.

use rmcp::{
    model::CallToolRequestParams,
    serde_json,
    transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    },
    ServiceExt,
};

async fn probe() {
    let args: Vec<String> = std::env::args().collect();
    let config = StreamableHttpClientTransportConfig::with_uri(args[1].clone())
        .auth_header(std::env::var("MCP_TEST_TOKEN").unwrap());
    let transport = StreamableHttpClientTransport::with_client(reqwest::Client::new(), config);
    let client = ().serve(transport).await.expect("SDK initialize/initialized");
    let version = client.peer_info().unwrap().protocol_version.to_string();
    assert_eq!(version, "2025-06-18");
    let tools = client.list_all_tools().await.expect("SDK tools/list");
    assert_eq!(tools.len(), 3);
    for (name, arguments) in [
        (
            "cabinet_search",
            serde_json::json!({"query":"MCP test page"}),
        ),
        ("cabinet_read", serde_json::json!({"cabinet_ref":args[2]})),
        (
            "cabinet_history",
            serde_json::json!({"cabinet_ref":args[2]}),
        ),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new(name)
                    .with_arguments(arguments.as_object().unwrap().clone()),
            )
            .await
            .expect("SDK tools/call");
        assert_eq!(result.is_error, Some(false), "{name}");
        assert!(result.structured_content.is_some(), "{name}");
        assert!(!result.content.is_empty(), "{name}");
    }
    client
        .cancel()
        .await
        .expect("SDK close stateless transport");
    println!("SDK initialize/initialized, negotiated {version}, tools/list, search/read/history, stateless close: passed");
}

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(20), probe())
                .await
                .expect("SDK probe timed out");
        });
}
