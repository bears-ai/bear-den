//! Exercise the actual browser edge against the same canonical lease used by
//! native chat and BearWire, including its detached disconnect flush.
use super::{access_tests, *};
use crate::web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream};
use den_service::tool_turns::ConversationTurnLease;
use futures::StreamExt;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tower::ServiceExt;

#[derive(Default)]
struct HoldingRuntime(AtomicUsize);
impl WebChatRuntime for HoldingRuntime {
    fn stream_chat(
        &self,
        _state: &AppState,
        _request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(Box::pin(
                futures::stream::iter([Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::AssistantTextDelta {
                        text: "partial browser answer".into(),
                    },
                ))])
                .chain(futures::stream::pending()),
            ) as WebChatRuntimeStream)
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn browser_and_native_share_exclusion_through_disconnected_browser_flush(pool: PgPool) {
    let (bear, [owner, ..]) = access_tests::seed(&pool).await;
    let external = "conv-cross-edge-admission";
    let canonical = access_tests::bound_conversation(&pool, bear, owner, external).await;
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let runtime = Arc::new(HoldingRuntime::default());
    let app = access_tests::app_with_runtime(&single, runtime.clone()).await;
    let cookie = access_tests::login(&app, owner).await;
    let native_lease = ConversationTurnLease::acquire(&pool, canonical)
        .await
        .unwrap();
    let (status, _) = access_tests::request(
        &app,
        &cookie,
        "POST",
        "/v1/chat/send",
        json!({"bear_id":bear,"conversation_id":external,"message":"must not be written"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
    assert!(conversation_persistence::list_projected_messages_page(
        &pool,
        canonical,
        None,
        100,
        conversation_persistence::ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap()
    .is_empty());
    drop(native_lease);

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/send")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"bear_id":bear,"conversation_id":external,"message":"browser question"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert!(std::str::from_utf8(&frame)
        .unwrap()
        .contains("partial browser answer"));
    assert!(ConversationTurnLease::acquire(&pool, canonical)
        .await
        .is_err());

    // Delay the asynchronous disconnect write without a new SQL/test query.
    let blocker = single.acquire().await.unwrap();
    drop(body);
    assert!(
        ConversationTurnLease::acquire(&pool, canonical)
            .await
            .is_err(),
        "the detached flush, not only the HTTP body, must retain the source lease"
    );
    drop(blocker);
    let next = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(lease) = ConversationTurnLease::acquire(&pool, canonical).await {
                break lease;
            }
        }
    })
    .await
    .unwrap();
    let history = conversation_persistence::list_projected_messages_page(
        &pool,
        canonical,
        None,
        100,
        conversation_persistence::ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|row| row.content_text == "partial browser answer")
            .count(),
        1
    );
    drop(next);
}
