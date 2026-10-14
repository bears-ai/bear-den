use super::{payload, Fixture, ReadHook, Revoke};
use axum::http::{Method, StatusCode};
use den_service::bears::db::revoke_membership;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres and test-only JWT_SECRET; do not run against the live stack"]
async fn page_access_does_not_grant_file_citations_or_byte_io(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    revoke_membership(&pool, f.user, f.bear).await.unwrap();
    let response = f
        .request(
            Method::POST,
            None,
            json!({
                "jsonrpc":"2.0", "id":1, "method":"tools/call",
                "params":{"name":"cabinet_read", "arguments":{"cabinet_ref":f.page}}
            }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = payload(response).await;
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(
        body["result"]["structuredContent"]["attachments"],
        json!([])
    );
    assert!(!body.to_string().contains(f.attachment.as_str()));
    assert!(!body.to_string().contains("text.txt"));

    let hook = ReadHook {
        fixture: &f,
        revoke: Revoke::None,
        called: false.into(),
    };
    let response = f.request(Method::POST, Some(&hook), json!({
        "jsonrpc":"2.0", "id":2, "method":"tools/call",
        "params":{"name":"cabinet_read", "arguments":{"cabinet_ref":f.page, "attachment_ref":f.attachment}}
    })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = payload(response).await;
    assert_eq!(body["result"]["isError"], true);
    assert!(!body.to_string().contains("PRIVATE ATTACHMENT BYTES"));
    assert!(!hook.called.load(std::sync::atomic::Ordering::SeqCst));
}
