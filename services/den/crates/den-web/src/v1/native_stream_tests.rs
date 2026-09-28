use super::access_tests::{app_with_runtime, conversation, login, seed};
use super::*;
use crate::web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream};
use axum::{body::Body, http::Request};
use den_protocol::RuntimeConversationRef;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

struct ResolvedRuntime(String);

impl WebChatRuntime for ResolvedRuntime {
    fn stream_chat(
        &self,
        _state: &AppState,
        _request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        let id = self.0.clone();
        Box::pin(async move {
            Ok(Box::pin(futures::stream::iter([
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::ConversationResolved {
                        conversation: RuntimeConversationRef { id },
                    },
                )),
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::AssistantTextDelta {
                        text: "answer".to_string(),
                    },
                )),
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnCompleted { turn: None },
                )),
            ])) as WebChatRuntimeStream)
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn late_native_resolution_cannot_persist_into_other_members_conversation(pool: PgPool) {
    let (bear, [one, two, _]) = seed(&pool).await;
    den_service::bears::db::ensure_bear_profile_binding_rows(&pool, bear)
        .await
        .unwrap();
    let foreign = conversation(&pool, bear, Some(two), "conv-guessed-foreign").await;
    let app = app_with_runtime(
        &pool,
        Arc::new(ResolvedRuntime("conv-guessed-foreign".to_string())),
    )
    .await;
    let cookie = login(&app, one).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/send")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"bear_id":bear,"conversation_id":"conv-owned-send","message":"hello"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let events = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(events.contains("error"), "expected stream denial: {events}");
    let rows = conversation_persistence::list_projected_messages_page(
        &pool,
        foreign,
        None,
        10,
        conversation_persistence::ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "foreign conversation must not receive the reply"
    );

    let owned = conversation(&pool, bear, Some(one), "conv-own-target").await;
    let owned_app = app_with_runtime(
        &pool,
        Arc::new(ResolvedRuntime("conv-own-target".to_string())),
    )
    .await;
    let owned_cookie = login(&owned_app, one).await;
    let response = owned_app.clone().oneshot(Request::builder()
        .method("POST").uri("/v1/chat/send")
        .header(header::COOKIE, owned_cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"bear_id":bear,"conversation_id":"conv-owned-send","message":"hello again"}).to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let events = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(
        events.contains("conversation_resolved"),
        "expected owner resolution: {events}"
    );
    let rows = conversation_persistence::list_projected_messages_page(
        &pool,
        owned,
        None,
        10,
        conversation_persistence::ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert!(rows.iter().any(|row| row.content_text == "answer"));

    let (_, default_id) = checked_chat_id(&pool, bear, two, "default").await.unwrap();
    let default_app = app_with_runtime(&pool, Arc::new(ResolvedRuntime(default_id.clone()))).await;
    let default_cookie = login(&default_app, two).await;
    let response = default_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/send")
                .header(header::COOKIE, default_cookie.clone())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"bear_id":bear,"conversation_id":"default","message":"my default"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let events = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(
        !events.contains(&default_id),
        "default stays a UI alias: {events}"
    );
    let (_, history) = super::access_tests::request(
        &default_app,
        &default_cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id=default"),
        Value::Null,
    )
    .await;
    assert!(history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "answer"));
}
