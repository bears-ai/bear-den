//! Actual native web-chat request stream -> root dispatcher -> live service
//! policy -> authenticated HTTP adapter -> canonical ingester -> fresh turn.
use super::{
    native_fixture::{self, Fixture},
    native_provider::{self, Api, CALL, CANARY, SHA},
};
use crate::core::tools::runtime_invoker::DenRuntimeToolInvoker;
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
use den_runtime::native_runtime::{
    start_native_web_chat_turn_event_stream, NativeRuntimeDeps, NativeWebChatTurnParams,
};
use den_service::conversation::persistence::{
    self, ConversationHistoryProjection, PersistedTranscriptRecord,
};
use futures::StreamExt;
use serde_json::Value;
use sqlx::PgPool;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use uuid::Uuid;

fn safe_diagnostic(f: &Fixture, text: &str) -> String {
    let mut safe = text.to_owned();
    for secret in [
        CANARY.to_owned(),
        f.credential.reference.secret_id().to_string(),
        f.credential.reference.backend_binding_id().to_string(),
        f.state.config.den_secret_encryption_key.clone(),
        "sk-bf-native-repository-test".to_owned(),
    ] {
        if !secret.is_empty() {
            safe = safe.replace(&secret, "[redacted]");
        }
    }
    safe.chars().take(2000).collect()
}

async fn turn(f: &Fixture, invoker: Arc<DenRuntimeToolInvoker>, prompt: &str) {
    let deps = NativeRuntimeDeps {
        pool: &f.state.sqlx_pool,
        config: f.state.config.as_ref(),
        stores: &f.state.memory_stores,
    };
    let mut stream = start_native_web_chat_turn_event_stream(NativeWebChatTurnParams {
        deps: &deps,
        bear_id: f.bear.as_uuid(),
        bear_slug: &f.slug,
        turn_binding_id: &f.binding,
        user_id: f.actor.get(),
        username: None,
        membership_role: Some("admin"),
        conversation_id: &f.conversation,
        session_id: &f.session,
        prompt,
        request_id: Uuid::new_v4(),
        tool_invoker: invoker,
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut completed = false;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap_or_else(|error| panic!("native stream failed: {}; events={events:?}", safe_diagnostic(f, &error.to_string())));
            let rendered = format!("{event:?}");
            assert!(!rendered.contains(CANARY), "native event exposed a credential canary");
            events.push(safe_diagnostic(f, &rendered));
            match event {
                RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. }) => {
                    completed = true;
                }
                RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed { category, message, .. }) => {
                    panic!("native turn must complete: category={category:?}; cause={}; events={events:?}", safe_diagnostic(f, &message))
                }
                _ => {}
            }
        }
        assert!(completed, "native turn ended without completion; events={events:?}");
    })
    .await
    .expect("native mock turn is bounded");
}

async fn wait_for_ingesters(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows = persistence::list_messages_page(&f.state.sqlx_pool, f.canonical, None, 100)
                .await
                .unwrap();
            let mut call = false;
            let mut result = false;
            for row in &rows {
                let value = row.content_json.to_string();
                assert!(!value.contains(CANARY));
                assert!(!value.contains(&f.credential.reference.secret_id().to_string()));
                assert!(!value.contains(&f.credential.reference.backend_binding_id().to_string()));
                match row.to_model_transcript_record() {
                    Some(PersistedTranscriptRecord::ToolCall { tool_call_id, .. })
                        if tool_call_id == CALL =>
                    {
                        call = true;
                    }
                    Some(PersistedTranscriptRecord::ToolResult {
                        tool_call_id: Some(id),
                        content: Some(content),
                        status,
                        ..
                    }) if id == CALL => {
                        assert_eq!(status.as_deref(), Some("ok"));
                        let payload: Value = serde_json::from_str(&content).unwrap();
                        assert_eq!(payload.as_object().unwrap().len(), 2);
                        assert_eq!(payload["commit_sha"], SHA);
                        assert_eq!(payload["work_surface_id"], serde_json::json!(f.surface));
                        result = true;
                    }
                    _ => {}
                }
            }
            if call && result {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic canonical tool persistence must be model replayable");
}

fn assert_model_tool_result(body: &Value, api: Api) {
    match api {
        Api::Chat => {
            let messages = body["messages"].as_array().unwrap();
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["role"] == "tool"
                        && message["tool_call_id"] == CALL
                        && message["content"]
                            .as_str()
                            .is_some_and(|text| text.contains(SHA)))
                    .count(),
                1
            );
            assert_eq!(
                messages
                    .iter()
                    .filter_map(|message| message["tool_calls"].as_array())
                    .flatten()
                    .filter(
                        |call| call["id"] == CALL && call["function"]["name"] == "repository_head"
                    )
                    .count(),
                1
            );
        }
        Api::Responses => {
            let input = body["input"].as_array().unwrap();
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "function_call_output"
                        && item["call_id"] == CALL
                        && item["output"]
                            .as_str()
                            .is_some_and(|text| text.contains(SHA)))
                    .count(),
                1
            );
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "function_call"
                        && item["call_id"] == CALL
                        && item["name"] == "repository_head")
                    .count(),
                1
            );
        }
    }
}

async fn exercise(pool: PgPool, api: Api) {
    let (gateway_server, gateway) = native_provider::gateway(api).await;
    let f = native_fixture::fixture(&pool, &format!("http://{}/v1", gateway_server.address)).await;
    gateway.set_surface(f.surface);
    let (repository_server, provider) = native_provider::repository_server().await;
    let resolver = Arc::new(native_provider::Resolver {
        expected: f.credential.clone(),
        calls: AtomicUsize::new(0),
    });
    let adapter = Arc::new(
        den_repository::test_util::LoopbackRepository::new(
            repository_server.address,
            resolver.clone(),
        )
        .unwrap(),
    );
    let invoker = Arc::new(DenRuntimeToolInvoker::with_repository_test_adapter(
        f.state.clone(),
        adapter,
    ));
    turn(&f, invoker.clone(), "Read the configured repository head.").await;
    wait_for_ingesters(&f).await;
    assert!(provider.authenticated.load(Ordering::SeqCst));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    let prior = gateway.requests.lock().unwrap().len();
    assert_eq!(
        prior, 2,
        "initial inference plus automatic tool-result continuation"
    );
    turn(
        &f,
        invoker,
        "Summarize the already-read head without another provider operation.",
    )
    .await;
    let requests = { gateway.requests.lock().unwrap().clone() };
    assert_model_tool_result(&requests[1], api);
    assert_model_tool_result(&requests[prior], api);
    for request in &requests {
        let text = request.to_string();
        for secret in [
            CANARY,
            &f.credential.reference.secret_id().to_string(),
            &f.credential.reference.backend_binding_id().to_string(),
        ] {
            assert!(!text.contains(secret));
        }
    }

    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "historical replay does not repeat the effect"
    );
    let history = persistence::list_projected_messages_page(
        &pool,
        f.canonical,
        None,
        100,
        ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert!(history.iter().all(|row| row
        .to_model_transcript_record()
        .is_none_or(|record| matches!(record, PersistedTranscriptRecord::Message(_)))));
}

#[sqlx::test]
async fn authenticated_repository_head_replays_through_native_chat_requests(pool: PgPool) {
    exercise(pool, Api::Chat).await;
}
#[sqlx::test]
async fn authenticated_repository_head_replays_through_native_responses_requests(pool: PgPool) {
    exercise(pool, Api::Responses).await;
}
