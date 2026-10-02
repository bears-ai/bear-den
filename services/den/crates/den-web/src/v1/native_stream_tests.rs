use super::access_tests::{app_with_runtime, conversation, login, request, seed};
use super::*;
use crate::web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream};
use axum::{body::Body, http::Request};
use den_protocol::RuntimeConversationRef;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Default)]
struct NoResolutionRuntime {
    requests: Mutex<Vec<WebChatRuntimeRequest>>,
}

impl WebChatRuntime for NoResolutionRuntime {
    fn stream_chat(
        &self,
        _state: &AppState,
        request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        self.requests.lock().unwrap().push(request);
        Box::pin(async {
            Ok(Box::pin(futures::stream::iter([
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::AssistantTextDelta {
                        text: "saved answer".to_string(),
                    },
                )),
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnCompleted { turn: None },
                )),
            ])) as WebChatRuntimeStream)
        })
    }
}

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
async fn first_new_send_resolves_durable_conversation_and_reloads(pool: PgPool) {
    let (bear, [owner, other, _admin]) = seed(&pool).await;
    assert!(
        den_service::bears::db::profile_binding_id(&pool, bear, BearProfile::Chat)
            .await
            .unwrap()
            .is_none()
    );
    let runtime = Arc::new(NoResolutionRuntime::default());
    let app = app_with_runtime(&pool, runtime.clone()).await;
    let owner_cookie = login(&app, owner).await;
    let other_cookie = login(&app, other).await;
    let placeholder = "new-pending123";
    let response = app.clone().oneshot(Request::builder()
        .method("POST").uri("/v1/chat/send")
        .header(header::COOKIE, &owner_cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"bear_id":bear,"conversation_id":placeholder,"message":"first question"}).to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let events = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|json| serde_json::from_str::<Value>(json).unwrap())
        .collect::<Vec<_>>();
    assert!(!events.is_empty(), "expected SSE events: {body}");
    assert_eq!(events[0]["message_type"], "conversation_resolved");
    let durable = events[0]["conversation_id"].as_str().unwrap();
    assert!(durable.starts_with("conv-"));
    assert!(Uuid::parse_str(durable.strip_prefix("conv-").unwrap()).is_ok());
    assert!(
        !body.contains(placeholder),
        "placeholder must not reach the browser: {body}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["message_type"] == "text" || event["content"] == "saved answer"),
        "assistant SSE missing: {body}"
    );
    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].conversation_id, durable);
    assert!(requests[0].session_id.ends_with(durable));
    drop(requests);

    let saved = conversation_persistence::get_conversation_for_external_id(&pool, bear, durable)
        .await
        .unwrap()
        .expect("durable canonical conversation");
    assert!(conversation_viewer(&pool, bear, owner)
        .await
        .unwrap()
        .may_access_id(&pool, saved.id)
        .await
        .unwrap());
    assert_eq!(
        runtime.requests.lock().unwrap()[0].turn_binding_id,
        hats::turn_binding::NativeTurnSource::Conversation(saved.id).binding_id(BearId::new(bear))
    );
    assert!(
        conversation_persistence::get_conversation_for_external_id(&pool, bear, placeholder)
            .await
            .unwrap()
            .is_none()
    );
    let (_, list) = request(
        &app,
        &owner_cookie,
        "GET",
        &format!("/v1/chat/conversations?bear_id={bear}"),
        Value::Null,
    )
    .await;
    assert!(list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == durable));
    assert!(!list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == placeholder));
    let history_uri = format!("/v1/chat/history?bear_id={bear}&conversation_id={durable}");
    let (status, history) = request(&app, &owner_cookie, "GET", &history_uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let messages = history["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|m| m["role"] == "user" && m["text"] == "first question"));
    assert!(messages
        .iter()
        .any(|m| m["role"] == "ai" && m["text"] == "saved answer"));
    assert_eq!(
        request(&app, &other_cookie, "GET", &history_uri, Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &other_cookie,
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear,"conversation_id":durable,"message":"guess"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    conversation(&pool, bear, Some(other), "new-guessed123").await;
    assert_eq!(
        request(
            &app,
            &owner_cookie,
            "POST",
            "/v1/chat/send",
            json!({"bear_id":bear,"conversation_id":"new-guessed123","message":"guess"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn first_send_does_not_replay_placeholder_history(pool: PgPool) {
    let (bear, [owner, _other, _admin]) = seed(&pool).await;
    den_service::bears::db::ensure_bear_profile_binding_rows(&pool, bear)
        .await
        .unwrap();
    let placeholder = "new-oldplaceholder";
    let old = conversation(&pool, bear, Some(owner), placeholder).await;
    conversation_persistence::append_message(
        &pool,
        old,
        &den_service::conversation::message_types::ConversationMessageWrite::user_turn(
            "stale placeholder turn",
            json!({"type":"user_input", "text":"stale placeholder turn"}),
            None,
        ),
    )
    .await
    .unwrap();
    let app = app_with_runtime(&pool, Arc::new(NoResolutionRuntime::default())).await;
    let cookie = login(&app, owner).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/send")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"bear_id":bear,"conversation_id":placeholder,"message":"fresh turn"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let sse = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let event: Value = serde_json::from_str(
        sse.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    let durable = event["conversation_id"].as_str().unwrap();
    assert_ne!(durable, placeholder);
    let (_, history) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/history?bear_id={bear}&conversation_id={durable}"),
        Value::Null,
    )
    .await;
    let messages = history["messages"].as_array().unwrap();
    assert!(messages.iter().any(|row| row["text"] == "fresh turn"));
    assert!(messages.iter().any(|row| row["text"] == "saved answer"));
    assert!(!messages
        .iter()
        .any(|row| row["text"] == "stale placeholder turn"));
    let (_, list) = request(
        &app,
        &cookie,
        "GET",
        &format!("/v1/chat/conversations?bear_id={bear}"),
        Value::Null,
    )
    .await;
    assert!(!list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == placeholder));
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_responses_also_resolve_new_chats(pool: PgPool) {
    let (bear, [owner, _other, _admin]) = seed(&pool).await;
    den_service::bears::db::ensure_bear_profile_binding_rows(&pool, bear)
        .await
        .unwrap();
    let runtime = Arc::new(NoResolutionRuntime::default());
    let app = app_with_runtime(&pool, runtime.clone()).await;
    let cookie = login(&app, owner).await;
    for (placeholder, message) in [
        ("new-direct-title", "rename conversation to First title"),
        ("new-direct-tools", "list capabilities"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/send")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"bear_id":bear,"conversation_id":placeholder,"message":message})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let first = body
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        let event: Value = serde_json::from_str(first).unwrap();
        assert_eq!(event["message_type"], "conversation_resolved");
        let durable = event["conversation_id"].as_str().unwrap();
        assert!(Uuid::parse_str(durable.strip_prefix("conv-").unwrap()).is_ok());
        assert!(
            conversation_persistence::get_conversation_for_external_id(&pool, bear, durable)
                .await
                .unwrap()
                .is_some()
        );
        assert!(conversation_persistence::get_conversation_for_external_id(
            &pool,
            bear,
            placeholder
        )
        .await
        .unwrap()
        .is_none());
        let (_, list) = request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/conversations?bear_id={bear}"),
            Value::Null,
        )
        .await;
        assert!(list["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == durable));
        if placeholder == "new-direct-title" {
            assert_eq!(
                conversation_persistence::get_conversation_for_external_id(&pool, bear, durable)
                    .await
                    .unwrap()
                    .unwrap()
                    .current_title
                    .as_deref(),
                Some("First title")
            );
        } else {
            let (_, history) = request(
                &app,
                &cookie,
                "GET",
                &format!("/v1/chat/history?bear_id={bear}&conversation_id={durable}"),
                Value::Null,
            )
            .await;
            assert!(history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["text"] == "list capabilities"));
            assert!(history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["role"] == "ai"));
        }
    }
    assert!(
        runtime.requests.lock().unwrap().is_empty(),
        "direct paths must bypass the native runtime"
    );
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
