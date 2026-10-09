use super::access_tests::{app_with_runtime, login, request, seed};
use super::chat_model_access_tests::{assert_json_error, raw_request};
use super::*;
use crate::web_chat_runtime::{WebChatRuntime, WebChatRuntimeRequest, WebChatRuntimeStream};
use den_core::ThinkingEffort;
use den_service::bears::model_configurations;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};

const QUESTION: &str = "First question under the configured hat";
const ANSWER: &str = "Saved first answer under the configured hat";

#[derive(Default)]
struct FirstTurnRuntime {
    requests: Mutex<Vec<WebChatRuntimeRequest>>,
}

impl WebChatRuntime for FirstTurnRuntime {
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
                        text: ANSWER.into(),
                    },
                )),
                Ok(RuntimeStreamEvent::Semantic(
                    RuntimeSemanticEvent::TurnCompleted { turn: None },
                )),
            ])) as WebChatRuntimeStream)
        })
    }
}

async fn stored_chat_counts(pool: &PgPool, bear: Uuid, users: &[i32]) -> (usize, i64, usize) {
    let conversations = conversation_persistence::list_conversations_for_bear(pool, bear, 200)
        .await
        .unwrap();
    // Messages reference canonical conversations, so counting every Bear source
    // also proves no messages exist when the fresh Bear has no conversations.
    let mut messages = 0;
    for conversation in &conversations {
        messages += sqlx::query_scalar!(
            "SELECT count(*) AS \"count!\" FROM conversation_messages WHERE conversation_id = $1",
            conversation.id,
        )
        .fetch_one(pool)
        .await
        .unwrap();
    }
    let bear = bears_db::get_bear(pool, bear).await.unwrap().unwrap();
    let mut sessions = 0;
    for &user_id in users {
        sessions += client_sessions::list_for_user_bear(
            pool,
            client_sessions::SessionListParams {
                user_id,
                bear_slug: &bear.slug,
                include_closed: true,
                cwd_filter: None,
                limit: 100,
                cursor_updated_at: None,
                cursor_id: None,
            },
        )
        .await
        .unwrap()
        .len();
    }
    (conversations.len(), messages, sessions)
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_bear_default_preview_hat_creation_first_send_and_history_reload(pool: PgPool) {
    let (bear, users @ [owner, other, admin]) = seed(&pool).await;
    let bear_id = BearId::new(bear);
    let configured = model_configurations::create(
        &pool,
        bear_id,
        "Chosen default",
        "openai/gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    model_configurations::set_default(&pool, bear_id, Some(configured.id))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        bear_id,
        UserId::new(admin),
        "Configured chat hat",
        "Answer this Bear's questions",
    )
    .await
    .unwrap();
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (0, 0, 0));

    let runtime = Arc::new(FirstTurnRuntime::default());
    let app = app_with_runtime(&pool, runtime.clone()).await;
    let cookie = login(&app, owner).await;
    let model_uri = format!("/v1/chat/model?bear_id={bear}&conversation_id=default");
    let (status, preview) = request(&app, &cookie, "GET", &model_uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["source"], "bear_default");
    assert_eq!(preview["configuration_id"], json!(configured.id));
    assert_eq!(preview["configuration_name"], "Chosen default");
    assert_eq!(preview["effective_model"], "openai/gpt-5");
    assert_eq!(preview["thinking_effort"], "high");
    assert!(preview["selected_model"].is_null());
    assert!(preview["requested_model"].is_null());
    assert!(preview["error"].is_null());
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (0, 0, 0));

    let list_uri = format!("/v1/chat/conversations?bear_id={bear}");
    let (status, list) = request(&app, &cookie, "GET", &list_uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["conversations"].as_array().unwrap().is_empty());
    let hats = list["hats"].as_array().unwrap();
    assert_eq!(hats.len(), 1);
    assert_eq!(hats[0]["id"], json!(hat.id.as_uuid()));
    assert_eq!(hats[0]["name"], "Configured chat hat");

    let rejected_send = json!({
        "bear_id": bear,
        "conversation_id": "default",
        "message": "Unbound default must not start a turn",
    })
    .to_string();
    let error = assert_json_error(
        raw_request(&app, &cookie, "POST", "/v1/chat/send", &rejected_send).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(error["code"], "conversation_read_only");
    assert!(runtime.requests.lock().unwrap().is_empty());
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (0, 0, 0));

    let (status, created) = request(
        &app,
        &cookie,
        "POST",
        "/v1/chat/conversations",
        json!({"bear_id": bear, "hat_id": hat.id.as_uuid()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let external = created["id"].as_str().unwrap();
    Uuid::parse_str(external.strip_prefix("conv-").unwrap()).unwrap();
    assert_eq!(created["hat_id"], json!(hat.id.as_uuid()));
    let canonical =
        conversation_persistence::get_conversation_for_external_id(&pool, bear, external)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        hats::bindings::conversation_hat(&pool, bear_id, canonical.id)
            .await
            .unwrap(),
        Some(hat.id),
    );
    assert!(conversation_viewer(&pool, bear, owner)
        .await
        .unwrap()
        .may_read_own_source(&pool, canonical.id)
        .await
        .unwrap());
    assert!(!conversation_viewer(&pool, bear, other)
        .await
        .unwrap()
        .may_access_id(&pool, canonical.id)
        .await
        .unwrap());
    den_service::conversation::viewer::require_ordinary_tool_source(
        &pool,
        bear_id,
        UserId::new(owner),
        external,
    )
    .await
    .unwrap();
    let primary = den_service::model_selection::resolve_conversation_primary_model(
        &pool,
        bear_id,
        canonical.id,
        "openai/gpt-4.1",
    )
    .await
    .unwrap();
    assert_eq!(primary.configuration_id, Some(configured.id));
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (1, 0, 0));

    let (status, list) = request(&app, &cookie, "GET", &list_uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let listed = list["conversations"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], external);
    assert_eq!(listed[0]["hat_id"], json!(hat.id.as_uuid()));
    assert_eq!(listed[0]["own_notes_available"], true);

    let response = raw_request(
        &app,
        &cookie,
        "POST",
        "/v1/chat/send",
        &json!({"bear_id": bear, "conversation_id": external, "message": QUESTION}).to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream; charset=utf-8"
    );
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let sse = std::str::from_utf8(&body).unwrap();
    let events = sse
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| event["content"] == ANSWER),
        "{sse}"
    );
    {
        let requests = runtime.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let turn = &requests[0];
        assert_eq!(turn.prompt, QUESTION);
        assert_eq!(turn.bear_id, bear);
        assert_eq!(turn.user_id, owner);
        assert_eq!(turn.conversation_id, external);
        assert_eq!(
            turn.session_id,
            browser_client_session_id(owner, bear, external)
        );
        assert_eq!(turn.request_id, request_id);
        assert_eq!(
            turn.turn_binding_id,
            hats::turn_binding::NativeTurnSource::Conversation(canonical.id).binding_id(bear_id),
        );
    }

    let history_uri = format!("/v1/chat/history?bear_id={bear}&conversation_id={external}");
    for _ in 0..2 {
        let (status, history) = request(&app, &cookie, "GET", &history_uri, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["has_more"], false);
        let messages = history["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["text"], QUESTION);
        assert_eq!(messages[1]["role"], "ai");
        assert_eq!(messages[1]["text"], ANSWER);
    }
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (1, 2, 0));
    let reloaded =
        conversation_persistence::get_conversation_for_external_id(&pool, bear, external)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(reloaded.id, canonical.id);
    assert_eq!(
        hats::bindings::conversation_hat(&pool, bear_id, reloaded.id)
            .await
            .unwrap(),
        Some(hat.id),
    );

    assert_json_error(
        raw_request(&app, &cookie, "POST", "/v1/chat/send", &rejected_send).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(stored_chat_counts(&pool, bear, &users).await, (1, 2, 0));
    assert_eq!(runtime.requests.lock().unwrap().len(), 1);
}
