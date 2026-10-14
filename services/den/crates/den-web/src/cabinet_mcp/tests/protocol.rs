use super::{auth::Principal, protocol, tools};
use axum::{
    body::to_bytes,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use den_cabinet::{ActorScope, CabinetError, CabinetItemRef};
use den_core::{ids::UserId, tools::descriptor::builtin_den_tool_descriptor_for_provider_name};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;

async fn payload(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2025-06-18"),
    );
    headers
}
fn pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap()
}
fn principal(user: i32) -> Principal {
    Principal {
        scope: ActorScope::user(UserId(user)),
        client_id: "registered-client".into(),
        token_id: 17,
    }
}

#[tokio::test]
async fn stateless_streamable_http_handshake_lists_tools_and_pings() {
    let pool = pool();
    let principal = principal(41);
    let mut headers = headers();
    headers.remove("mcp-protocol-version");
    protocol::validate_headers(&headers).unwrap();
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test-client", "version": "1.0"}
    }});
    let response = protocol::handle(
        &pool,
        None,
        &principal,
        &headers,
        initialize.to_string().as_bytes(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()["mcp-protocol-version"], "2025-06-18");
    assert!(!response.headers().contains_key("mcp-session-id"));
    let body = payload(response).await;
    assert_eq!(body["id"], 1);
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        body["result"]["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    assert!(body["result"].get("instructions").is_none());
    headers.insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2025-06-18"),
    );
    let response = protocol::handle(
        &pool,
        None,
        &principal,
        &headers,
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(to_bytes(response.into_body(), 100)
        .await
        .unwrap()
        .is_empty());
    let response = protocol::handle(
        &pool,
        None,
        &principal,
        &headers,
        br#"{"jsonrpc":"2.0","id":"list-1","method":"tools/list"}"#,
    )
    .await;
    let body = payload(response).await;
    assert_eq!(body["id"], "list-1");
    let advertised = body["result"]["tools"].as_array().unwrap();
    assert_eq!(advertised.len(), 3);
    for tool in advertised {
        let descriptor =
            builtin_den_tool_descriptor_for_provider_name(tool["name"].as_str().unwrap()).unwrap();
        assert_eq!(tool["inputSchema"], descriptor.input_schema);
        assert_eq!(tool["description"], descriptor.description);
        assert_eq!(
            tool["_meta"]["den"]["permissions"],
            json!(descriptor.permissions)
        );
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(
            tool["_meta"]["den"]["projection"],
            "delegated_human_read_only"
        );
    }
    let response = protocol::handle(
        &pool,
        None,
        &principal,
        &headers,
        br#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
    )
    .await;
    assert_eq!(
        payload(response).await,
        json!({"jsonrpc":"2.0","id":3,"result":{}})
    );
    let response = protocol::handle(&pool, None, &principal, &headers,
        br#"{"jsonrpc":"2.0","id":4,"method":"ping","params":{"_meta":{"progressToken":"progress-4"}}}"#).await;
    assert_eq!(
        payload(response).await,
        json!({"jsonrpc":"2.0","id":4,"result":{}})
    );
}

#[test]
fn version_negotiation_and_http_media_contract() {
    for (requested, expected) in [
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2099-01-01", "2025-06-18"),
    ] {
        let (_, result) = protocol::initialize(json!({"protocolVersion":requested,"capabilities":{},"clientInfo":{"name":"sdk","version":"1"}})).unwrap();
        assert_eq!(result["protocolVersion"], expected);
    }
    let mut headers = headers();
    assert!(protocol::validate_headers(&headers).is_ok());
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    assert!(protocol::validate_headers(&headers).is_err());
    for accept in [
        "application/json, text/event-stream;q=0",
        "application/json, text/event-stream;Q=0",
    ] {
        headers.insert(header::ACCEPT, HeaderValue::from_static(accept));
        assert!(protocol::validate_headers(&headers).is_err());
    }
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert!(protocol::validate_headers(&headers).is_err());
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2099-01-01"),
    );
    assert!(protocol::validate_headers(&headers).is_err());
    headers.remove("mcp-protocol-version");
    assert_eq!(
        protocol::version(&headers).unwrap(),
        protocol::Version::March2025
    );
    headers.append(
        "mcp-protocol-version",
        HeaderValue::from_static("2025-03-26"),
    );
    headers.append(
        "mcp-protocol-version",
        HeaderValue::from_static("2025-06-18"),
    );
    assert!(protocol::version(&headers).is_err());
}

#[tokio::test]
async fn protocol_errors_and_notifications_never_dispatch_tools() {
    let pool = pool();
    let principal = principal(41);
    for (body, code) in [
        ("{", -32700),
        ("[]", -32600),
        (r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#, -32600),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":null}"#,
            -32600,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":[]}"#,
            -32600,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"actor":"spoofed"}}"#,
            -32602,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"_meta":"bad"}}"#,
            -32602,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","result":{}}"#,
            -32600,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#,
            -32600,
        ),
        (r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":1,"method":"unknown"}"#, -32601),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            -32602,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":"foreign"}}"#,
            -32602,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cabinet_delete"}}"#,
            -32602,
        ),
    ] {
        let response = protocol::handle(&pool, None, &principal, &headers(), body.as_bytes()).await;
        assert_eq!(payload(response).await["error"]["code"], code, "{body}");
    }
    // These would reach a database if mistakenly dispatched. The lazy pool has
    // no database; accepted notifications must not execute even read operations.
    let response = protocol::handle(&pool, None, &principal, &headers(), br#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"cabinet_search","arguments":{"query":""}}}"#).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    for body in [
        br#"{"jsonrpc":"2.0","id":1,"result":{}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Unknown"}}"#.as_slice(),
    ] {
        let response = protocol::handle(&pool, None, &principal, &headers(), body).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty());
    }
}

#[test]
fn typed_arguments_reject_identity_spoofing_and_unadvertised_mutations() {
    let page = CabinetItemRef::mint();
    for key in [
        "scope",
        "actor",
        "actor_kind",
        "user_id",
        "bear_id",
        "run_id",
        "conversation_id",
        "client_id",
    ] {
        for (name, mut args) in [
            ("cabinet_read", json!({"cabinet_ref": page})),
            ("cabinet_history", json!({"cabinet_ref": page})),
            ("cabinet_search", json!({"query": ""})),
        ] {
            args[key] = json!("spoofed");
            assert!(
                tools::parse_call(json!({"name":name,"arguments":args})).is_err(),
                "{name}: {key}"
            );
        }
    }
    for name in [
        "cabinet_create",
        "cabinet_update",
        "cabinet_source_link",
        "cabinet_lifecycle",
        "cabinet_delete",
        "session_info",
    ] {
        assert!(
            tools::parse_call(json!({"name":name,"arguments":{}})).is_err(),
            "{name}"
        );
    }
    for args in [
        json!({"cabinet_ref":"bad"}),
        json!({"cabinet_ref":page,"limit_chars":0}),
        json!({"cabinet_ref":page,"limit_chars":24001}),
        json!({"cabinet_ref":page,"offset_chars":-1}),
        json!({"cabinet_ref":page,"version_ref":den_cabinet::CabinetVersionRef::mint(),"attachment_ref":den_cabinet::CabinetAttachmentRef::mint()}),
    ] {
        assert!(tools::parse_call(json!({"name":"cabinet_read","arguments":args})).is_err());
    }
    assert!(tools::parse_call(
        json!({"name":"cabinet_search","arguments":{"query":"","lifecycle":"deleted"}})
    )
    .is_err());
    assert!(tools::parse_call(
        json!({"name":"cabinet_search","arguments":{"query":"x".repeat(4097)}})
    )
    .is_err());
    assert!(
        tools::parse_call(json!({"name":"cabinet_read","arguments":{"cabinet_ref":page}})).is_ok()
    );
    // Canonical compatibility names resolve through the shared resolver, not an
    // independent MCP alias table.
    let descriptor = builtin_den_tool_descriptor_for_provider_name("cabinet_read").unwrap();
    assert!(
        tools::parse_call(json!({"name":descriptor.name,"arguments":{"cabinet_ref":page}})).is_ok()
    );
}

#[tokio::test]
async fn structured_tool_errors_are_not_protocol_errors_or_storage_leaks() {
    for (error, kind) in [
        (CabinetError::NotFound, "not_found"),
        (
            CabinetError::Storage("postgres password secret".into()),
            "unavailable",
        ),
        (CabinetError::Policy("private title".into()), "policy"),
    ] {
        let result = tools::tool_result(Err(error)).unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["kind"], kind);
        let encoded = result.to_string();
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("private title"));
        let response = protocol::response(
            Some(protocol::Id::Number(5)),
            protocol::Version::June2025,
            Ok(result),
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert!(payload(response).await.get("error").is_none());
    }
    let result = tools::tool_result(Ok(json!({"items": []}))).unwrap();
    let fallback: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(fallback, result["structuredContent"]);
    let oversized = tools::tool_result(Ok(json!({"content": "x".repeat(256 * 1024)}))).unwrap();
    assert_eq!(oversized["isError"], true);
    assert_eq!(
        oversized["structuredContent"]["error"]["kind"],
        "validation"
    );
    let response = protocol::response(
        Some(protocol::Id::Number(1)),
        protocol::Version::March2025,
        Ok(result),
    );
    let body = payload(response).await;
    assert!(body["result"].get("structuredContent").is_none());
    assert!(body["result"]["content"].is_array());
}

#[tokio::test]
async fn every_method_requires_bearer_and_cookie_does_not_authenticate() {
    let pool = pool();
    let mut headers = headers();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("id=browser-session"),
    );
    for method in [
        Method::GET,
        Method::POST,
        Method::DELETE,
        Method::HEAD,
        Method::OPTIONS,
    ] {
        let response = super::serve(
            &pool,
            None,
            &method,
            &Uri::from_static("/mcp"),
            &headers,
            b"{}",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    }
}
