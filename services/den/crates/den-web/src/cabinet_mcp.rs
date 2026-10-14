//! Cabinet-only, delegated-human MCP edge. Mount [`router`] in the Cabinet composition.
//!
//! # Credentials and release boundary
//!
//! Accepts existing Den OAuth authorization-code access JWTs (public clients use
//! PKCE), issuer `den-oauth`, audience `den-api`, with `data:read` in the JWT,
//! current token row, and current active client's grants. The registered token,
//! active client and live user are reloaded for **every HTTP request**; there is
//! no connection-level authority cache or cookie/armature-token fallback.
//! `JWT_SECRET` must be nonempty; this edge never uses the OAuth dev secret.
//! This is explicitly the existing Den resource audience, not Cabinet-specific
//! OAuth discovery/resource-indicator support. Configure the bearer manually.
//!
//! Only search/read/history are projected from native descriptors. Page reads
//! include source provenance and independently authorized attachment citations;
//! attachment text uses the same facade and optional injected byte reader.
//! Create/update/source mutations, lifecycle/delete/review and Bear identities
//! are not exposed. `ActorScope` currently cannot durably record OAuth client
//! delegation: enabling mutations needs that backend provenance extension and
//! an explicit resource-audience/write-scope decision, not a fabricated run ID.
//!
//! # Streamable HTTP client contract
//!
//! Supports MCP `2025-06-18` and `2025-03-26`, single-message JSON responses.
//! Newer/unknown initialize versions negotiate `2025-06-18` (the client must
//! accept it or disconnect). POST `/mcp` with `Authorization: Bearer <token>`,
//! `Content-Type: application/json`, and
//! `Accept: application/json, text/event-stream`:
//!
//! 1. `initialize` with an ID, `protocolVersion`, `capabilities`, `clientInfo`.
//! 2. `notifications/initialized` without an ID; response is empty HTTP 202.
//! 3. `tools/list`, `tools/call` or `ping` with IDs. Send the negotiated
//!    `MCP-Protocol-Version` on subsequent requests. Absent headers use the
//!    specification's backwards-compatible `2025-03-26` default.
//!
//! No `Mcp-Session-Id` is issued or required; any supplied ID is rejected rather
//! than interpreted as authentication. No SSE, resumability, server notifications,
//! batches, resources or prompts. Authenticated GET/DELETE/other methods return
//! 405 (`Allow: POST`); HEAD never opens a stream. JSON responses carry the MCP
//! version, `Cache-Control: no-store`, and `X-Content-Type-Options: nosniff`.
//! This native-client-only release rejects every `Origin` header and query
//! string (including query tokens); it enables no CORS or browser cookie flow.
//! Expose only over HTTPS at the trusted ingress. Protocol/auth tests exercise
//! this handshake. The optional standalone `rmcp` probe checks these read-only
//! flows over authenticated loopback HTTP, not production ingress/composition.

#[path = "cabinet_mcp/auth.rs"]
mod auth;
#[path = "cabinet_mcp/protocol.rs"]
mod protocol;
#[path = "cabinet_mcp/tools.rs"]
mod tools;

#[cfg(test)]
#[path = "cabinet_mcp/tests/auth.rs"]
mod auth_tests;
#[cfg(test)]
#[path = "cabinet_mcp/tests/protocol.rs"]
mod protocol_tests;
#[cfg(test)]
#[path = "cabinet_mcp/tests/security.rs"]
mod security_tests;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue, Method, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use den_service::artifacts::bytes::ArtifactByteReader;
use sqlx::PgPool;

/// Authenticated Cabinet MCP only; the parent owns composition and state.
pub fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/mcp", any(endpoint))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

async fn endpoint(
    State(state): State<crate::AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    serve(
        state.sqlx_pool(),
        state
            .media
            .as_ref()
            .map(|media| media as &dyn ArtifactByteReader),
        &method,
        &uri,
        &headers,
        &body,
    )
    .await
}

async fn serve(
    pool: &PgPool,
    reader: Option<&dyn ArtifactByteReader>,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let result = async {
        auth::protect_transport(uri, headers)?;
        let principal = auth::authenticate(pool, headers).await?;
        if method != Method::POST {
            let mut response = axum::http::StatusCode::METHOD_NOT_ALLOWED.into_response();
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST"));
            return Ok(response);
        }
        protocol::validate_headers(headers)?;
        let response = protocol::handle(pool, reader, &principal, headers, body).await;
        // A byte-store transfer may outlive its credential. Never release a
        // response using grants revoked/expired while assembling the result.
        auth::authenticate(pool, headers).await?;
        Ok(response)
    }
    .await;
    let mut response = result.unwrap_or_else(auth::AuthError::response);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
