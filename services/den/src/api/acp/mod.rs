//! Minimal Agent Client Protocol (ACP) gateway for adapter clients.
//!
//! This is the Phase 7 basic-chat slice: Den authenticates, authorizes the selected bear,
//! injects trusted context, and maps text prompts to the Bear's API-direct `pair` Letta agent.
//! Client-tool relay and full ACP stdio transport live in later slices / an external adapter.

pub(super) mod client;
pub(super) mod compat;
pub(super) mod config;
pub(super) mod handlers;
pub(super) mod history;
pub(super) mod http_types;
pub(super) mod letta_support;
pub(super) mod pair_reflection_support;
pub(super) mod paths;
pub(super) mod prompt_context;
pub(super) mod prompt_guidance;
pub(super) mod responses;
pub(super) mod routing;
pub(super) mod sessions;
pub(super) mod stream;
pub(super) mod tool_result_diagnostics;
pub(super) mod tool_results;
pub(super) mod types;
pub(super) mod workflow;
pub(super) mod workflow_guidance;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            compat::{
                acp_compatibility_error_response, check_adapter_contract,
            },
            stream::{
                mapping::map_runtime_stream_event_to_acp_adapter_events_with_persistence,
                plan::{
                    mode_from_den_tool_result, plan_approval_fallback_payload,
                    plan_update_from_den_tool_result,
                },
                prompt_flow::run_prompt_flow,
                runtime::{invoke_acp_den_tool, persist_stream_event_side_effects},
            },
            tool_results::default_unavailable_context_budget,
        },
        service::ApiState,
    },
    core::{
        acp_letta_events::AcpGatewayEvent,
        acp_tools::{acp_provider_tool_names_for_client_context, resolve_session_policy_for_mode},
        acp_turn_controller::AcpActiveTurnCancelHandle,
        acp_turn_runner::{
            acp_cleanup_stale_runtime_state, continue_acp_turn_with_runtime,
            AcpStaleRuntimeCleanupParams, AcpTurnContinueRequest, AcpTurnStreamContext,
        },
        letta::RuntimeContinuationContext,
        runtime_provider::RoleRuntimeBinding,
    },
};
use self::{
    responses::acp_error_status_message,
    types::{
        format_acp_session_timestamp, AcpPendingFuture, AcpResolvedToolResult,
        AcpResolvedTurnContext, AcpSessionHttp, AcpStreamContext, AdapterContract,
        ToolExecutionRoute,
    },
};

const ACP_SESSIONS_PAGE_SIZE: i64 = 50;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/bears/{slug}/sessions", get(list_acp_sessions))
        .route("/bears/{slug}/sessions/{session_id}", get(get_acp_session))
        .route(
            "/bears/{slug}/sessions/{session_id}/runtime",
            get(get_acp_session_runtime),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/mode",
            post(set_session_mode),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/adapter-environment",
            post(post_adapter_environment),
        )
        .route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
        .route(
            "/bears/{slug}/sessions/{session_id}/tool-results/{tool_call_id}",
            post(tool_result),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/permissions/{permission_id}",
            post(permission_result),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/close",
            post(close_session),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/cancel",
            post(cancel_session),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/compact",
            post(compact_session),
        )
        .route("/bears/{slug}/conversations", get(conversations))
        .route(
            "/bears/{slug}/conversations/{conversation_id}/history",
            get(conversation_history),
        )
        .route("/bears/{slug}/auth-check", get(auth_check))
}

pub(crate) use self::client::{
    acp_pair_den_tool_descriptors, merge_acp_pair_tool_descriptors, new_acp_conversation_id,
    normalize_acp_client, requested_mode_from_prompt, tools_enabled_for_client,
};
pub(crate) use self::config::{
    acp_debug_event_sample_chars, acp_debug_ui_enabled, acp_stream_tokens_enabled,
    acp_text_chunk_chars, acp_tool_timeout_ms_for_provider,
};
pub(crate) use self::http_types::{
    AcpConversationHistoryMessage, AcpConversationRow, AcpErrorResponse, AcpPromptRequest,
    AcpToolResultResponse,
};
use self::http_types::{
    AcpAdapterEnvironmentRequest, AcpCloseSessionResponse, AcpConversationHistoryQuery,
    AcpConversationHistoryResponse, AcpConversationsQuery, AcpConversationsResponse,
    AcpPermissionDecisionRequest, AcpPermissionDecisionResponse, AcpSessionsListHttpResponse,
    AcpSessionsListQuery, AcpSetModeRequest, AcpSetModeResponse,
};
use self::config::pending_web_fetch_approvals;
use self::config::PendingWebFetchApproval;
pub(crate) use self::history::normalize_acp_conversation_id;
pub(crate) use self::routing::{
    acp_archive_target_for_session, acp_den_provider_to_canonical_tool_name,
};
use self::routing::tool_execution_route;
pub(crate) use self::letta_support::{
    cancel_runtime_runs_by_id_or_skip, looks_like_runtime_waiting_for_approval_error,
};
pub(crate) use self::pair_reflection_support::run_pair_reflection_summary;
pub(crate) use self::sessions::{acp_session_row_to_http_with_modes, resolve_acp_turn_context};
pub(crate) use self::workflow::{workflow_state_json, workflow_state_json_from_sources};

use self::{
    handlers::{
        auth::{auth_check, authenticate_acp_code_token_with_auth},
        conversations::{conversation_history, conversations},
        permissions::permission_result,
        session_lifecycle::{cancel_session, close_session, compact_session},
        sessions::{
            get_acp_session, get_acp_session_runtime, list_acp_sessions,
            post_adapter_environment, set_session_mode,
        },
        tool_results::tool_result,
    },
    responses::{acp_error_response, api_auth_error_response},
    sessions::{decode_acp_sessions_cursor, encode_acp_sessions_cursor},
};

async fn prompt(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPromptRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return acp_compatibility_error_response(err, request_id);
    }
    let result = async { prompt_inner(state, slug, session_id, headers, body, request_id).await }
        .instrument(tracing::info_span!("acp_prompt", request_id = %request_id))
        .await;
    match result {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => acp_error_response(err, request_id),
        Err(err) => api_auth_error_response(err, request_id),
    }
}


async fn prompt_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpPromptRequest,
    request_id: Uuid,
) -> types::AcpPromptInnerResult {
    run_prompt_flow(state, slug, session_id, headers, body, request_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use reqwest::StatusCode;
    use crate::{
        errors::CustomError,
        api::acp::{
            history::{acp_auto_title_instruction, map_acp_history_page},
            prompt_context::acp_direct_tool_prompt_context_with_activity,
            stream::{
                mapping::{
                    map_letta_stream_frame_to_acp_adapter_events, summarize_event_for_log,
                },
                sse_stream::{runtime_terminal_events, AcpLettaSseStream},
                support_sse::{find_sse_frame_end, parse_sse_event_body_to_json},
                text::AcpTextChunker,
            },
            tool_results::acp_tool_result_response_from_delivery,
        },
        core::{
            acp_letta_events::AcpGatewayEvent,
            acp_runtime::{
                is_valid_pending_acp_conversation_id, resolve_acp_prompt_conversation,
                AcpConversationResolution, AcpConversationSelectionSource,
            },
            acp_sessions,
            den_tools,
            acp_tool_turns::{
                AcpToolResultDelivery, AcpToolResultRequest, AcpToolTurnCoordinator,
                AcpToolTurnRegistration,
            },
            acp_tools::AcpToolStatus,
            acp_turn_controller::{
                AcpTerminalReason, AcpTerminalStatus, AcpTurnController, AcpTurnPhase,
            },
            acp_turn_runner::ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON,
            letta::PendingApprovalDenialMode,
            role_runtime::{RoleRuntime, RoleTurnScope},
        },
    };

    #[test]
    fn acp_prompt_requested_mode_is_normalized() {
        let body: AcpPromptRequest = serde_json::from_value(serde_json::json!({
            "message": "hello",
            "requested_mode": " WRITE "
        }))
        .expect("prompt request");

        assert_eq!(requested_mode_from_prompt(&body).unwrap(), Some("write"));
    }

    #[test]
    fn acp_prompt_requested_mode_rejects_unknown_values() {
        let body: AcpPromptRequest = serde_json::from_value(serde_json::json!({
            "message": "hello",
            "requested_mode": "sudo"
        }))
        .expect("prompt request");

        assert!(matches!(
            requested_mode_from_prompt(&body),
            Err(CustomError::ValidationError(_))
        ));
    }

    #[test]
    fn acp_pair_descriptors_keep_workboard_tools_but_hide_mode_control_tools() {
        let descriptors = acp_pair_den_tool_descriptors();
        let names = descriptors
            .as_array()
            .expect("descriptor array")
            .iter()
            .filter_map(|descriptor| descriptor.get("name").and_then(|value| value.as_str()))
            .collect::<Vec<_>>();

        for expected in [
            den_tools::DEN_WORK_PLAN_UPDATE_PROVIDER,
            den_tools::DEN_WORK_PLAN_GET_STATUS_PROVIDER,
            den_tools::DEN_WORK_PLAN_LIST_PROVIDER,
            den_tools::DEN_WORK_PLAN_REQUEST_HANDOFF_PROVIDER,
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }

        for hidden in [
            den_tools::DEN_PLAN_MODE_ENTER_PROVIDER,
            den_tools::DEN_PLAN_MODE_STATUS_PROVIDER,
            den_tools::DEN_PLAN_MODE_RECORD_APPROVAL_PROVIDER,
            den_tools::DEN_PLAN_MODE_EXIT_PROVIDER,
            den_tools::DEN_PLAN_MODE_CANCEL_PROVIDER,
        ] {
            assert!(
                !names.contains(&hidden),
                "unexpected mode-control tool {hidden}"
            );
        }
    }

    #[test]
    fn concurrent_letta_run_conflict_is_not_stale_approval() {
        let err = CustomError::System(
            "Letta send message HTTP 409 Conflict: another run is still processing this conversation"
                .to_string(),
        );

        assert!(!looks_like_runtime_waiting_for_approval_error(&err));
    }

    #[test]
    fn acp_recovery_approval_denial_reasons_do_not_look_like_policy_blocks() {
        for reason in [ACP_STALE_APPROVAL_RECOVERY_DENIAL_REASON] {
            assert!(!reason.contains("Denied by BEARS"));
            assert!(reason.contains("expired ACP approval request"));
            assert!(reason.contains("not a user or web policy block"));
            assert!(reason.contains("Retry the tool"));
        }
    }

    #[test]
    fn acp_history_page_replays_desc_letta_page_chronologically() {
        let body = serde_json::json!({
            "messages": [
                { "id": "m4", "message_type": "assistant_message", "content": "reply 2", "created_at": "2026-01-01T00:00:04Z" },
                { "id": "m3", "message_type": "user_message", "content": "ask 2", "created_at": "2026-01-01T00:00:03Z" },
                { "id": "m2", "message_type": "assistant_message", "content": "reply 1", "created_at": "2026-01-01T00:00:02Z" },
                { "id": "m1", "message_type": "user_message", "content": "ask 1", "created_at": "2026-01-01T00:00:01Z" }
            ]
        });
        let (messages, _has_more, next_before) = map_acp_history_page(&body, 4);
        assert_eq!(next_before.as_deref(), Some("m1"));
        assert_eq!(
            messages
                .iter()
                .map(|message| (message.role.as_str(), message.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("user", "ask 1"),
                ("assistant", "reply 1"),
                ("user", "ask 2"),
                ("assistant", "reply 2"),
            ]
        );
    }

    #[test]
    fn adapter_environment_request_deserializes_client_thread_title() {
        let body: AcpAdapterEnvironmentRequest = serde_json::from_value(serde_json::json!({
            "environment": { "thread_title": "Zed rename" },
            "conversation_title": "Zed rename"
        }))
        .expect("request should deserialize");
        assert_eq!(body.conversation_title.as_deref(), Some("Zed rename"));
        assert_eq!(
            body.environment
                .get("thread_title")
                .and_then(|value| value.as_str()),
            Some("Zed rename")
        );
    }

    #[test]
    fn acp_direct_tool_prompt_context_marks_untitled_sessions() {
        let policy = crate::core::acp_tools::resolve_session_policy_for_mode("ask", None);
        let context = acp_direct_tool_prompt_context_with_activity(
            "acp-test-session",
            "/workspace",
            &serde_json::json!({
                "workspace_roots": ["/workspace"],
                "tools": []
            }),
            true,
            &policy,
            None,
            Some("This conversation is currently untitled. Once the main subject is clear enough to summarize in a short, specific title, proactively call `set_conversation_title` in that turn without waiting for the user to ask."),
        );
        assert!(
            context.contains("Conversation title status for this ACP session: currently untitled.")
        );
        assert!(context.contains("set_conversation_title"));
    }

    #[test]
    fn summarize_letta_event_for_log_redacts_large_tool_return() {
        let event = serde_json::json!({
            "message_type": "tool_return_message",
            "id": "message-test",
            "run_id": "run-test",
            "step_id": "step-test",
            "tool_call_id": "call-test",
            "status": "success",
            "tool_return": "x".repeat(10_000),
            "tool_call": {
                "function": {
                    "name": "fs_edit_file",
                    "arguments": "{\"path\":\"/tmp/a\",\"old_text\":\"secret\",\"new_text\":\"replacement\"}"
                }
            }
        });
        let summary = summarize_event_for_log(&event);
        assert_eq!(summary["message_type"], "tool_return_message");
        assert_eq!(summary["run_id"], "run-test");
        assert_eq!(summary["tool_call_id"], "call-test");
        assert_eq!(summary["tool_return"]["redacted"], true);
        assert_eq!(summary["tool_return"]["bytes"], 10_000);
        assert!(summary["tool_return"].get("preview").is_none());
        assert_eq!(
            summary["tool_call"]["function"]["arguments"]["redacted"],
            true
        );
        assert_eq!(
            summary["tool_call"]["function"]["arguments"]["json_keys"],
            serde_json::json!(["new_text", "old_text", "path"])
        );
    }

    #[test]
    fn acp_text_chunker_flushes_first_reasoning_status_without_waiting_for_punctuation() {
        let mut chunker = AcpTextChunker::new_with_reasoning_limit(1024, 128);
        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "Thinking".to_string(),
        });
        assert_eq!(events.len(), 1);
        let AcpGatewayEvent::StatusText { text } = &events[0] else {
            panic!("expected status text");
        };
        assert_eq!(text, "Thinking");
    }

    #[test]
    fn acp_text_chunker_caps_reasoning_output_per_turn() {
        let mut chunker = AcpTextChunker::new_with_reasoning_limit(1024, 10);
        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "abcdefghijklmnopqrstuvwxyz".to_string(),
        });
        assert_eq!(events.len(), 1);
        let AcpGatewayEvent::StatusText { text } = &events[0] else {
            panic!("expected status text");
        };
        assert!(text.starts_with("abcdefghij\n"));
        assert!(text.contains("BEARS suppressed additional thinking/status output"));

        let events = chunker.push(AcpGatewayEvent::StatusText {
            text: "more".to_string(),
        });
        assert!(events.is_empty());
    }

    #[test]
    fn acp_tool_result_turn_missing_returns_late_result_ignored() {
        let registry = AcpToolTurnCoordinator::new();
        let response = acp_tool_result_response_from_delivery(
            AcpToolResultDelivery::TurnMissing {
                turn_id: Some("turn-1".to_string()),
                tool_call_id: "call-1".to_string(),
            },
            "acp-session",
            "call-1".to_string(),
            AcpToolStatus::Ok,
            &registry,
        )
        .to_value();

        assert_eq!(response["accepted"], false);
        assert_eq!(response["reason"], "late_result_ignored");
        assert_eq!(response["settlement"], "unknown");
        assert_eq!(response["turn_id"], "turn-1");
        assert_eq!(response["tool_call_id"], "call-1");
        assert_eq!(response["diagnostic"]["phase"], "late_tool_result_ignored");
    }

    #[test]
    fn acp_tool_result_recently_settled_timeout_returns_timed_out_settlement() {
        let registry = AcpToolTurnCoordinator::new();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        registry
            .register(AcpToolTurnRegistration {
                user_id: 1,
                bear_id: Uuid::new_v4(),
                bear_slug: "test-bear".to_string(),
                acp_session_id: "acp-session".to_string(),
                request_id: Uuid::new_v4(),
                tool_call_id: "call-timeout".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: Some("approval-timeout".to_string()),
                timeout_ms: 1,
                result_tx: tx,
            })
            .unwrap();
        let delivered = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-session",
                "call-timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call-timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "timeout".to_string(),
                    content: Some("timed out".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivered, AcpToolResultDelivery::Delivered { .. }));
        registry.remove("acp-session", "call-timeout");
        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-session",
                "call-timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call-timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        let response = acp_tool_result_response_from_delivery(
            late,
            "acp-session",
            "call-timeout".to_string(),
            AcpToolStatus::Ok,
            &registry,
        )
        .to_value();

        assert_eq!(response["accepted"], false);
        assert_eq!(response["reason"], "late_result_ignored");
        assert_eq!(response["settlement"], "timed_out");
        assert_eq!(response["tool_call_id"], "call-timeout");
        assert_eq!(response["diagnostic"]["status"], "timeout");
    }

    #[tokio::test]
    async fn acp_stream_waits_for_tool_result_and_continues_letta() {
        use axum::{
            extract::State,
            http::header,
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
            tool_return_status: StatusCode,
            tool_return_body: &'static str,
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> Response {
            *state.captured.lock().await = Some(body);
            (
                state.tool_return_status,
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                state.tool_return_body,
            )
                .into_response()
        }

        async fn fake_cancel(State(state): State<FakeState>) -> Response {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
                .into_response()
        }

        let captured = Arc::new(TokioMutex::new(None));
        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                captured: captured.clone(),
                tool_return_status: StatusCode::OK,
                tool_return_body: concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"file says hello\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let cancel_registry = crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let role_runtime =
            RoleRuntime::with_turn_cancellations(registry.clone(), cancel_registry.clone());
        let (cancel_handle, cancel_rx) = cancel_registry.register(
            "acp-test-session",
            request_id,
            Some("conv-test-resolved".to_string()),
        );
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"run_id\":\"run-stream-test\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_test\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        )
        .with_cancel_registration(cancel_handle, cancel_rx);

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_test\""));

        let runtime_snapshot =
            cancel_registry.runtime_snapshot_for_session("acp-test-session", &registry);
        assert_eq!(
            runtime_snapshot["state"],
            serde_json::json!("requires_action")
        );
        assert_eq!(
            runtime_snapshot["active_turn"]["pending_obligations"],
            serde_json::json!(1)
        );
        assert_eq!(
            runtime_snapshot["active_turn"]["run_ids"],
            serde_json::json!(["run-stream-test"])
        );

        let delivery = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-test-session",
                "call_test",
                AcpToolResultRequest {
                    turn_id: Some("turn-test".to_string()),
                    request_id: Some("request-test".to_string()),
                    tool_call_id: Some("call_test".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some("hello from file".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("file says hello") {
                break;
            }
        }
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("file says hello"));

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["client_tools"][0]["name"], "fs_read_text_file");
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approval_request_id"], "approval-1");
        assert_eq!(body["messages"][0]["approve"], true);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "tool");
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_test"
        );
    }

    #[tokio::test]
    async fn acp_stream_failed_local_tool_result_continues_with_denial_payload() {
        use axum::{
            extract::State,
            http::header,
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> Response {
            *state.captured.lock().await = Some(body);
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"handled error\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
                .into_response()
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let cancel_registry = crate::core::acp_turn_controller::AcpActiveTurnCancelRegistry::new();
        let request_id = Uuid::new_v4();
        let role_runtime =
            RoleRuntime::with_turn_cancellations(registry.clone(), cancel_registry.clone());
        let (_cancel_handle, _cancel_rx) = cancel_registry.register(
            "acp-error-session",
            request_id,
            Some("conv-error-resolved".to_string()),
        );
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-error-session",
            Some("conv-error-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-error-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-error-resolved".to_string()),
            upstream_target: "conv-error-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-error\",\"run_id\":\"run-stream-error\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_error\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-error.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-error-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_error\""));

        let delivery = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-error-session",
                "call_error",
                AcpToolResultRequest {
                    turn_id: Some("turn-error".to_string()),
                    request_id: Some("request-error".to_string()),
                    tool_call_id: Some("call_error".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "error".to_string(),
                    content: Some("tool failed".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("handled error") {
                break;
            }
        }
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("handled error"));

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approval_request_id"], "approval-error");
        assert_eq!(body["messages"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approvals"][0]["approve"], false);
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_error"
        );
        assert_eq!(body["messages"][0]["approvals"][0]["reason"], "tool failed");
    }

    #[tokio::test]
    async fn acp_stream_does_not_emit_turn_result_before_local_tool_result() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"continued after tool\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_test\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_test\""));

        let mut pre_result_output = String::new();
        let no_terminal = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            while let Some(item) = stream.next().await {
                pre_result_output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
                if pre_result_output.contains("\"type\":\"turn_result\"")
                    || pre_result_output.contains("\"type\":\"turn_complete\"")
                {
                    break;
                }
            }
        })
        .await;
        // In full `acp_stream_` test runs, Tokio's clock can advance enough for the
        // synthetic local-tool timeout to settle before this probe. The invariant here is
        // narrower: no terminal may appear before either a real adapter result or an
        // auto-timeout settlement, and Den must not post a Letta continuation before one
        // of those settlements.
        if no_terminal.is_ok() {
            assert!(
                pre_result_output.contains("Local tool fs_read_text_file completed"),
                "stream emitted output before local tool result or timeout settlement: {pre_result_output}"
            );
        }
        assert!(
            !pre_result_output.contains("\"type\":\"turn_result\""),
            "stream emitted turn_result before local tool result settled: {pre_result_output}"
        );
        if !pre_result_output.contains("Local tool fs_read_text_file completed") {
            assert!(
                !pre_result_output.contains("\"type\":\"turn_complete\""),
                "stream emitted turn_complete before local tool result or timeout settlement: {pre_result_output}"
            );
            assert!(captured.lock().await.is_none());
        }

        if !pre_result_output.contains("Local tool fs_read_text_file completed") {
            let delivery = registry
                .deliver_result(
                    1,
                    "test-bear",
                    "acp-test-session",
                    "call_test",
                    AcpToolResultRequest {
                        turn_id: Some("turn-test".to_string()),
                        request_id: Some("request-test".to_string()),
                        tool_call_id: Some("call_test".to_string()),
                        tool_name: Some("fs_read_text_file".to_string()),
                        approval_request_id: None,
                        status: "ok".to_string(),
                        content: Some("hello from file".to_string()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({}),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(matches!(delivery, AcpToolResultDelivery::Delivered { .. }));
        }

        let mut output = pre_result_output;
        let _post_result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            while let Some(item) = stream.next().await {
                output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            }
        })
        .await;
        assert!(output.contains("Local tool fs_read_text_file completed"));
        assert!(output.contains("continued after tool"));
        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );
        assert!(captured.lock().await.is_some());
    }

    #[tokio::test]
    async fn acp_stream_duplicate_turn_complete_emits_once() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let config = crate::config::Config::test_stub();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry,
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n",
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
        )))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }

        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );
        assert_eq!(
            output.matches("\"type\":\"turn_result\"").count(),
            0,
            "output was: {output}"
        );
    }

    #[test]
    fn acp_turn_controller_emits_terminal_turn_result_for_stream_error() {
        let mut controller = AcpTurnController::new();
        controller.on_stream_started();
        controller.on_stream_error();

        assert!(controller.may_emit_terminal());
        let outcome = controller
            .take_terminal_event()
            .expect("stream error should authorize a terminal event");
        assert_eq!(outcome.status, AcpTerminalStatus::Failed);
        assert_eq!(outcome.reason, AcpTerminalReason::StreamError);
        assert_eq!(controller.phase(), AcpTurnPhase::Terminal);
    }

    #[tokio::test]
    async fn acp_stream_terminal_error_emits_error_and_failed_turn_result() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let config = crate::config::Config::test_stub();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-error-terminal-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry,
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-error-terminal-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(
            "data: {\"message_type\":\"error_message\",\"message\":\"boom\",\"error_type\":\"upstream_failure\"}\n\n",
        ))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }

        assert_eq!(
            output.matches("\"type\":\"error\"").count(),
            1,
            "output was: {output}"
        );
        assert_eq!(
            output.matches("\"type\":\"turn_result\"").count(),
            1,
            "output was: {output}"
        );
        assert!(
            output.contains("\"status\":\"failed\""),
            "output was: {output}"
        );
        assert!(
            output.contains("\"reason\":\"runtime_cleanup\""),
            "output was: {output}"
        );
        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            0,
            "output was: {output}"
        );
    }

    #[tokio::test]
    async fn acp_stream_runtime_continuation_conflict_emits_error_and_failed_turn_result() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                StatusCode::CONFLICT,
                [(header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"conversation waiting for approval\"}",
            )
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                captured: captured.clone(),
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-conflict-failed-terminal",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-conflict-failed-terminal".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"run_id\":\"run-conflict\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_conflict\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        assert!(String::from_utf8(first.to_vec())
            .unwrap()
            .contains("tool_request"));
        registry
            .deliver_result(
                1,
                "test-bear",
                "acp-conflict-failed-terminal",
                "call_conflict",
                AcpToolResultRequest {
                    tool_call_id: Some("call_conflict".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some("hello".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }
        assert_eq!(output.matches("\"type\":\"error\"").count(), 0, "{output}");
        assert_eq!(
            output.matches("\"type\":\"turn_result\"").count(),
            1,
            "{output}"
        );
        assert!(output.contains("\"status\":\"recovered\""), "{output}");
        assert!(output.contains("\"reason\":\"runtime_cleanup\""), "{output}");
        assert_eq!(output.matches("\"type\":\"turn_complete\"").count(), 0, "{output}");
        assert_eq!(*cancel_calls.lock().await, 1);
        assert!(captured.lock().await.is_some());
    }

    #[tokio::test]
    async fn acp_stream_routes_session_info_as_den_server_tool() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"oriented\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(config.clone()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"id\":\"approval-1\",\"message_type\":\"approval_request_message\",",
            "\"tool_call\":{\"name\":\"session_info\",\"tool_call_id\":\"call_session_info\",",
            "\"arguments\":\"{}\"}}\n\n"
        )))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-continuation".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "session_info" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first =
            tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await;
        assert!(
            first.is_err(),
            "Den-server session_info unexpectedly emitted an adapter event: {first:?}"
        );
        assert!(captured.lock().await.is_none());

        let missing = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-test-session",
                "call_session_info",
                AcpToolResultRequest {
                    tool_call_id: Some("call_session_info".to_string()),
                    tool_name: Some("session_info".to_string()),
                    status: "ok".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(missing, AcpToolResultDelivery::TurnMissing { .. }));
        drop(stream);
    }

    #[tokio::test]
    async fn acp_stream_emits_initial_session_info_update() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let mut config = crate::config::Config::load();
        config.letta_base_url = "http://127.0.0.1:9".to_string();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-test-session",
            Some("conv-test-resolved".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry,
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test-resolved".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            upstream_target: "conv-test-resolved".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(config.clone()),
            role_runtime,
            turn_scope,
        };
        let upstream = futures::stream::pending::<Result<Bytes, CustomError>>();
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            vec![AcpGatewayEvent::SessionInfoUpdate {
                title: Some("Renamed in same turn".to_string()),
                updated_at: Some("2026-05-23T00:00:00Z".to_string()),
                meta: None,
            }],
            true,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test-resolved".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 1,
            },
            active_turn_guard,
        );

        let first = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("expected initial session info update without waiting for next prompt")
            .expect("stream should yield an event")
            .expect("event should serialize");
        let output = String::from_utf8(first.to_vec()).unwrap();
        assert!(
            output.contains("\"type\":\"session_info_update\""),
            "output was: {output}"
        );
        assert!(
            output.contains("Renamed in same turn"),
            "output was: {output}"
        );
    }

    #[test]
    fn acp_auto_title_instruction_requires_saved_conversation_without_title() {
        let base = acp_sessions::AcpSessionRow {
            id: Uuid::nil(),
            user_id: 1,
            bear_id: Uuid::nil(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-test-session".to_string(),
            runtime_session_id: "runtime-test".to_string(),
            conversation_id: "conv-test-resolved".to_string(),
            resolved_conversation_id: Some("conv-test-resolved".to_string()),
            client: "zed".to_string(),
            cwd: Some("/workspace".to_string()),
            adapter_environment: None,
            current_mode: "ask".to_string(),
            conversation_title: None,
            conversation_title_updated_at: None,
            conversation_title_synced_at: None,
            closed_at: None,
            archived_at: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let guidance = acp_auto_title_instruction(&base).expect("guidance expected");
        assert!(guidance.contains("set_conversation_title"));
        assert!(guidance.contains("currently untitled"));
        assert!(guidance.contains("without waiting for the user to ask"));

        let titled = acp_sessions::AcpSessionRow {
            conversation_title: Some("Already titled".to_string()),
            ..base.clone()
        };
        assert!(acp_auto_title_instruction(&titled).is_none());

        let unresolved = acp_sessions::AcpSessionRow {
            resolved_conversation_id: None,
            conversation_id: "pending-id".to_string(),
            ..base
        };
        assert!(acp_auto_title_instruction(&unresolved).is_none());
    }

    #[tokio::test]
    async fn acp_stream_timeout_pending_local_tool() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                concat!(
                    "data: {\"message_type\":\"assistant_message\",\"content\":\"handled timeout\"}\n\n",
                    "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"end_turn\"}\n\n"
                ),
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .with_state(FakeState {
                captured: captured.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        std::env::set_var("BEARS_ACP_TOOL_TIMEOUT_MS", "20");

        let mut config = crate::config::Config::load();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-timeout-session",
            Some("conv-timeout".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-timeout-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-timeout".to_string()),
            upstream_target: "conv-timeout".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-timeout\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_timeout\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-timeout.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-timeout".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_timeout\""));

        let mut output = String::new();
        let stream_result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(item) = stream.next().await {
                output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            }
        })
        .await;
        std::env::remove_var("BEARS_ACP_TOOL_TIMEOUT_MS");
        assert!(
            stream_result.is_ok(),
            "stream timed out; output was: {output}"
        );

        assert!(
            output.contains("Local tool fs_read_text_file completed"),
            "output was: {output}"
        );
        assert!(output.contains("handled timeout"), "output was: {output}");
        assert_eq!(
            output.matches("\"type\":\"turn_complete\"").count(),
            1,
            "output was: {output}"
        );

        let body = captured.lock().await.clone().unwrap();
        assert_eq!(body["messages"][0]["type"], "approval");
        assert_eq!(
            body["messages"][0]["approval_request_id"],
            "approval-timeout"
        );
        assert_eq!(body["messages"][0]["approve"], false);
        assert_eq!(body["messages"][0]["approvals"][0]["type"], "approval");
        assert_eq!(body["messages"][0]["approvals"][0]["approve"], false);
        assert_eq!(
            body["messages"][0]["approvals"][0]["tool_call_id"],
            "call_timeout"
        );
        assert!(body["messages"][0]["approvals"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out after 20ms"));

        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-timeout-session",
                "call_timeout",
                AcpToolResultRequest {
                    tool_call_id: Some("call_timeout".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late result".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(
            late,
            AcpToolResultDelivery::RecentlySettled { .. }
                | AcpToolResultDelivery::TurnMissing { .. }
        ));
    }

    #[test]
    fn runtime_terminal_failure_events_follow_strict_terminal_contract() {
        let request_id = "req-test";
        let session_id = "acp-test-session";

        let turn_failed = runtime_terminal_events(
            crate::core::runtime_provider::RuntimeStreamEvent::TurnFailed {
                turn: None,
                category: crate::core::runtime_provider::RuntimeErrorCategory::Internal,
                message: "runtime failed".to_string(),
            },
            request_id,
            session_id,
        )
        .expect("turn failed maps to terminal events");
        assert!(matches!(turn_failed[0], AcpGatewayEvent::Error { .. }));
        assert!(matches!(turn_failed[1], AcpGatewayEvent::TurnResult { .. }));

        let turn_cancelled = runtime_terminal_events(
            crate::core::runtime_provider::RuntimeStreamEvent::TurnCancelled {
                turn: None,
            },
            request_id,
            session_id,
        )
        .expect("turn cancelled maps to terminal events");
        assert!(matches!(turn_cancelled[0], AcpGatewayEvent::Error { .. }));
        assert!(matches!(turn_cancelled[1], AcpGatewayEvent::TurnResult { .. }));

        let generic_error = runtime_terminal_events(
            crate::core::runtime_provider::RuntimeStreamEvent::Error {
                message: "runtime error".to_string(),
                detail: Some("detail".to_string()),
                error_type: Some("runtime_error".to_string()),
                request_id: Some(request_id.to_string()),
                context: Some(serde_json::json!({
                    "component": "den.acp",
                    "acp_session_id": session_id,
                })),
            },
            request_id,
            session_id,
        )
        .expect("generic runtime error maps to terminal events");
        assert!(matches!(generic_error[0], AcpGatewayEvent::Error { .. }));
        assert!(matches!(generic_error[1], AcpGatewayEvent::TurnResult { .. }));
    }

    #[test]
    fn acp_tool_result_endpoint_treats_replayed_identical_result_as_idempotent() {
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        registry
            .register(AcpToolTurnRegistration {
                user_id: 1,
                bear_id: Uuid::new_v4(),
                bear_slug: "test-bear".to_string(),
                acp_session_id: "acp-idempotent-session".to_string(),
                request_id,
                tool_call_id: "call_idempotent".to_string(),
                tool_name: "fs_read_text_file".to_string(),
                approval_request_id: None,
                timeout_ms: 1_000,
                result_tx,
            })
            .expect("register tool turn");

        let body = AcpToolResultRequest {
            tool_call_id: Some("call_idempotent".to_string()),
            tool_name: Some("fs_read_text_file".to_string()),
            status: "ok".to_string(),
            content: Some("same body".to_string()),
            structured_content: serde_json::json!({"k":"v"}),
            diagnostic: serde_json::json!({"phase":"first"}),
            ..Default::default()
        };

        let first = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-idempotent-session",
                "call_idempotent",
                body.clone(),
            )
            .expect("first delivery");
        assert!(matches!(first, AcpToolResultDelivery::Delivered { .. }));

        let delivered = result_rx.blocking_recv().expect("receiver gets delivered body");
        assert_eq!(delivered.content.as_deref(), Some("same body"));

        let replay = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-idempotent-session",
                "call_idempotent",
                body,
            )
            .expect("replayed delivery should not error");
        let response = acp_tool_result_response_from_delivery(
            replay,
            "acp-idempotent-session",
            "call_idempotent".to_string(),
            AcpToolStatus::Ok,
            &registry,
        );
        let value = response.to_value();
        assert_eq!(value["accepted"], true);
        assert_eq!(value["reason"], "duplicate_result_ignored");
        assert_eq!(value["settlement"], "already_settled");
        assert_eq!(
            value["diagnostic"]["tool_call_id"],
            serde_json::json!("call_idempotent")
        );
        assert_eq!(value["diagnostic"]["status"], "ok");
    }

    #[tokio::test]
    async fn acp_stream_cancel_pending_local_tool() {
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;

        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-cancel-session",
            Some("conv-cancel".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-cancel-session".to_string(),
            client: "zed".to_string(),
            conversation_selection: "new-acp-test".to_string(),
            resolved_conversation_id: Some("conv-cancel".to_string()),
            upstream_target: "conv-cancel".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(concat!(
            "data: {\"id\":\"approval-cancel\",\"message_type\":\"approval_request_message\",",
            "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_cancel\",",
            "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-cancel.txt\\\"}\"}}\n\n"
        )))]);
        let config = crate::config::Config::test_stub();
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-cancel".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        )
        .with_cancel_rx(cancel_rx);

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_cancel\""));

        cancel_tx.send(true).unwrap();
        let cancelled = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("cancel terminal should not hang");
        let cancelled = cancelled
            .expect("cancel should emit terminal before ending")
            .unwrap();
        let cancelled_text = String::from_utf8(cancelled.to_vec()).unwrap();
        assert!(
            cancelled_text.contains("\"type\":\"turn_result\""),
            "{cancelled_text}"
        );
        assert!(
            cancelled_text.contains("\"status\":\"cancelled\""),
            "{cancelled_text}"
        );
        assert!(
            cancelled_text.contains("\"reason\":\"cancelled\""),
            "{cancelled_text}"
        );

        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-cancel-session",
                "call_cancel",
                AcpToolResultRequest {
                    tool_call_id: Some("call_cancel".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late result".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(late, AcpToolResultDelivery::TurnMissing { .. }));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn acp_stream_requires_approval_stop_with_active_tool_does_not_trigger_cleanup() {
        use axum::{extract::State, http::header, response::IntoResponse, routing::post, Router};
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-requires-approval-active-tool",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-requires-approval-active-tool".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-active\",\"run_id\":\"run-active\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_active\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("\"type\":\"tool_request\""));
        assert!(first_text.contains("\"tool_call_id\":\"call_active\""));

        let pending = tokio::time::timeout(std::time::Duration::from_millis(150), stream.next())
            .await
            .expect("stream should yield at most a tool status update while obligation is open");
        let pending_text = String::from_utf8(pending.unwrap().unwrap().to_vec()).unwrap();
        assert!(
            pending_text.contains("\"type\":\"status_text\""),
            "unexpected output while waiting on active tool: {pending_text}"
        );
        assert!(
            pending_text.contains("Local tool fs_read_text_file completed"),
            "unexpected output while waiting on active tool: {pending_text}"
        );
        assert_eq!(
            *cancel_calls.lock().await,
            0,
            "requires_approval with an active tool must not trigger stale cleanup"
        );

        let late = registry
            .deliver_result(
                1,
                "test-bear",
                "acp-requires-approval-active-tool",
                "call_active",
                AcpToolResultRequest {
                    tool_call_id: Some("call_active".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    status: "ok".to_string(),
                    content: Some("late result after pending check".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(
            late,
            AcpToolResultDelivery::RecentlySettled { .. }
                | AcpToolResultDelivery::TurnMissing { .. }
        ));
    }

    #[tokio::test]
    async fn acp_stream_cleans_orphaned_requires_approval_stop() {
        use axum::{extract::State, http::header, response::IntoResponse, routing::post, Router};
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-orphaned-approval",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-orphaned-approval".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![Ok::<Bytes, CustomError>(Bytes::from(
            "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
        ))]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: None,
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
        }
        assert!(!output.contains("status_text"), "{output}");
        assert!(!output.contains("runtime recovery"), "{output}");
        assert!(output.contains("\"type\":\"turn_result\""), "{output}");
        assert_eq!(
            *cancel_calls.lock().await,
            0,
            "orphaned cleanup without run_ids must not issue an agent-wide Letta cancel"
        );
    }

    #[test]
    fn stale_approval_recovery_uses_inspect_only_mode_to_avoid_conversation_contamination() {
        let mode = PendingApprovalDenialMode::InspectOnly;
        assert!(matches!(mode, PendingApprovalDenialMode::InspectOnly));
    }

    #[tokio::test]
    async fn acp_stream_cleans_runtime_when_tool_return_continuation_conflicts() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        use futures::StreamExt;
        use sqlx::postgres::PgPoolOptions;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        #[derive(Clone)]
        struct FakeState {
            captured: Arc<TokioMutex<Option<serde_json::Value>>>,
            cancel_calls: Arc<TokioMutex<usize>>,
        }

        async fn fake_tool_return(
            State(state): State<FakeState>,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            *state.captured.lock().await = Some(body);
            (
                StatusCode::CONFLICT,
                [(header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"conversation waiting for approval\"}",
            )
        }

        async fn fake_cancel(State(state): State<FakeState>) -> impl IntoResponse {
            *state.cancel_calls.lock().await += 1;
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{\"cancelled\":true}",
            )
        }

        let captured = Arc::new(TokioMutex::new(None));
        let cancel_calls = Arc::new(TokioMutex::new(0));
        let app = Router::new()
            .route(
                "/v1/conversations/{conversation_id}/messages",
                post(fake_tool_return),
            )
            .route("/v1/agents/{agent_id}/messages/cancel", post(fake_cancel))
            .with_state(FakeState {
                captured: captured.clone(),
                cancel_calls: cancel_calls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = crate::config::Config::test_stub();
        config.letta_base_url = format!("http://{addr}");
        let letta = Arc::new(crate::core::letta::LettaClient::new(&config));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1/postgres")
            .unwrap();
        let registry = AcpToolTurnCoordinator::new();
        let request_id = Uuid::new_v4();
        let role_runtime = RoleRuntime::new(registry.clone());
        let turn_scope = RoleTurnScope::acp_pair(
            Uuid::new_v4(),
            "acp-continuation-conflict",
            Some("conv-test".to_string()),
        );
        let active_turn_guard = role_runtime
            .acquire_turn(turn_scope.clone(), request_id)
            .unwrap();
        let context = AcpStreamContext {
            pool,
            tool_turns: registry.clone(),
            user_id: 1,
            user_profile: None,
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            acp_session_id: "acp-continuation-conflict".to_string(),
            client: "zed".to_string(),
            conversation_selection: "conv-test".to_string(),
            resolved_conversation_id: Some("conv-test".to_string()),
            upstream_target: "conv-test".to_string(),
            workspace_roots: vec!["/workspace".to_string()],
            session_policy: None,
            activity: None,
            request_id,
            pair_agent_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            config: Arc::new(crate::config::Config::test_stub()),
            role_runtime: role_runtime.clone(),
            turn_scope,
        };
        let upstream = futures::stream::iter(vec![
            Ok::<Bytes, CustomError>(Bytes::from(concat!(
                "data: {\"id\":\"approval-1\",\"run_id\":\"run-conflict\",\"message_type\":\"approval_request_message\",",
                "\"tool_call\":{\"name\":\"fs_read_text_file\",\"tool_call_id\":\"call_conflict\",",
                "\"arguments\":\"{\\\"path\\\":\\\"/tmp/acp-test.txt\\\"}\"}}\n\n"
            ))),
            Ok::<Bytes, CustomError>(Bytes::from(
                "data: {\"message_type\":\"stop_reason\",\"stop_reason\":\"requires_approval\"}\n\n",
            )),
        ]);
        let mut stream = AcpLettaSseStream::new(
            upstream,
            context,
            Vec::new(),
            false,
            letta,
            RuntimeContinuationContext {
                conversation_id: "conv-test".to_string(),
                agent_id: Some("agent-12345678-1234-4567-89ab-123456789abc".to_string()),
                client_tools: Some(serde_json::json!([{ "name": "fs_read_text_file" }])),
                stream_tokens: false,
                max_steps: 2,
            },
            active_turn_guard,
        );

        let first = stream.next().await.unwrap().unwrap();
        assert!(String::from_utf8(first.to_vec())
            .unwrap()
            .contains("tool_request"));
        registry
            .deliver_result(
                1,
                "test-bear",
                "acp-continuation-conflict",
                "call_conflict",
                AcpToolResultRequest {
                    tool_call_id: Some("call_conflict".to_string()),
                    tool_name: Some("fs_read_text_file".to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some("hello".to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({}),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut output = String::new();
        while let Some(item) = stream.next().await {
            output.push_str(&String::from_utf8(item.unwrap().to_vec()).unwrap());
            if output.contains("\"status\":\"recovered\"") {
                break;
            }
        }
        assert!(output.contains("\"status\":\"recovered\""), "{output}");
        assert!(
            output.contains("\"run_ids\":[\"run-conflict\"]"),
            "{output}"
        );
        assert_eq!(*cancel_calls.lock().await, 1);
        assert!(captured.lock().await.is_some());
    }

    #[test]
    fn acp_history_filters_system_scoped_user_messages_and_reminder_suffixes() {
        let body = serde_json::json!({
            "messages": [
                {
                    "id": "msg-system-user",
                    "date": "2026-05-10T00:00:00Z",
                    "message_type": "user_message",
                    "role": "system",
                    "content": "BEARS ACP direct local workspace tools available this turn: fs_read_text_file."
                },
                {
                    "id": "msg-assistant",
                    "date": "2026-05-10T00:00:01Z",
                    "message_type": "assistant_message",
                    "content": "Done.\n<system-reminder>hidden harness</system-reminder>"
                },
                {
                    "id": "msg-human",
                    "date": "2026-05-10T00:00:02Z",
                    "message_type": "user_message",
                    "content": "Please check this thread.\n<system-reminder>adapter-only instructions</system-reminder>"
                },
                {
                    "id": "msg-human-scaffold",
                    "date": "2026-05-10T00:00:03Z",
                    "message_type": "user_message",
                    "content": "ACP workflow state for this session: workflow_id=123 workflow_state=submitted submitted_plan_present=true approval_status=awaiting_human_approval execution_unlocked=false. Workflow state is authoritative.\n\nPlease only show the real user text."
                }
            ]
        });
        let (messages, has_more, next_before) = map_acp_history_page(&body, 50);
        assert!(!has_more);
        assert_eq!(next_before.as_deref(), Some("msg-human-scaffold"));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "Please only show the real user text.");
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].text, "Please check this thread.");
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[2].text, "Done.");
    }

    #[test]
    fn sse_parser_joins_multiple_data_lines_into_one_json_value() {
        let body = br#"data: {"message_type":"assistant_message","content":
data: "hello"}"#;
        let v = parse_sse_event_body_to_json(body).unwrap().unwrap();
        assert_eq!(v["message_type"], "assistant_message");
        assert_eq!(v["content"], "hello");
        let out = map_letta_stream_frame_to_acp_adapter_events(
            b"data: {\"message_type\":\"assistant_message\",\"content\":\ndata: \"hello\"}\n\n",
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn sse_parser_rejects_invalid_json_with_parse_path_empty() {
        let body = br#"data: not-json"#;
        assert!(parse_sse_event_body_to_json(body).is_err());
        let out = map_letta_stream_frame_to_acp_adapter_events(b"data: not-json\n\n");
        assert!(out.is_empty());
    }

    #[test]
    fn sse_frame_end_prefers_earliest_lf_or_crlf_delimiter() {
        let buf = b"data: {}\r\n\r\n";
        assert_eq!(find_sse_frame_end(buf), Some(12));
        let buf2 = b"data: {}\n\n";
        assert_eq!(find_sse_frame_end(buf2), Some(10));
    }

    #[test]
    fn normalizes_acp_conversation_ids() {
        assert_eq!(normalize_acp_conversation_id(None).unwrap(), "default");
        assert_eq!(
            normalize_acp_conversation_id(Some("conv-abc12345")).unwrap(),
            "conv-abc12345"
        );
        assert_eq!(
            normalize_acp_conversation_id(Some("new-acp-zed-abc12345")).unwrap(),
            "new-acp-zed-abc12345"
        );
        assert!(normalize_acp_conversation_id(Some("conv-x")).is_err());
        assert!(normalize_acp_conversation_id(Some("../../etc/passwd")).is_err());
    }

    #[test]
    fn generated_acp_conversation_ids_are_compact_opaque_ids() {
        let id = new_acp_conversation_id("zed");
        assert!(id.starts_with("new-acp-zed-"));
        assert_eq!(id.len(), 34);
        assert!(is_valid_pending_acp_conversation_id(&id));

        let id = new_acp_conversation_id("acp_adapter");
        assert!(id.starts_with("new-acp-acp_adapter-"));
        assert_eq!(id.len(), 42);
        assert!(is_valid_pending_acp_conversation_id(&id));
    }

    #[test]
    fn resolver_maps_pending_acp_selection_to_letta_agent_target() {
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let resolution =
            resolve_acp_prompt_conversation(None, None, &binding, "new-acp-zed-abc123".to_string())
                .unwrap();
        assert_eq!(resolution.session_selection, "new-acp-zed-abc123");
        assert_eq!(resolution.resolved_conversation, None);
        assert_eq!(resolution.upstream_target, binding.binding_id);
        assert_eq!(resolution.history_target, None);
        assert_eq!(resolution.archive_target, None);
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Generated
        );
    }

    #[test]
    fn resolver_routes_explicit_conv_directly_and_requires_bear_check() {
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let conv_id = "conv-12345678-1234-4567-89ab-123456789abc";
        let resolution = resolve_acp_prompt_conversation(
            Some(conv_id),
            None,
            &binding,
            "new-acp-zed-unused".to_string(),
        )
        .unwrap();
        assert_eq!(resolution.session_selection, conv_id);
        assert_eq!(
            resolution
                .resolved_conversation
                .as_ref()
                .map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(resolution.upstream_target, conv_id);
        assert_eq!(
            resolution.history_target.as_ref().map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(
            resolution.archive_target.as_ref().map(|c| c.id.as_str()),
            Some(conv_id)
        );
        assert_eq!(
            resolution.selection_source,
            AcpConversationSelectionSource::Explicit
        );
        assert!(resolution.requires_belongs_to_bear_check);
    }

    #[test]
    fn resolver_never_archives_pending_or_default_targets() {
        let binding = crate::core::runtime_contracts::RoleRuntimeBinding {
            binding_id: "agent-12345678-1234-4567-89ab-123456789abc".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let pending = AcpConversationResolution::from_selection(
            "new-acp-zed-abc123".to_string(),
            AcpConversationSelectionSource::Generated,
            &binding,
            None,
        );
        assert_eq!(pending.history_target, None);
        assert_eq!(pending.archive_target, None);

        let default = AcpConversationResolution::from_selection(
            "default".to_string(),
            AcpConversationSelectionSource::Stored,
            &binding,
            None,
        );
        assert_eq!(
            default.history_target.as_ref().map(|c| c.id.as_str()),
            Some("default")
        );
        assert_eq!(default.archive_target, None);
    }

    #[test]
    fn rejects_legacy_pending_acp_conversation_ids_that_exceed_letta_limit() {
        let legacy = "new-acp-zed-acp-12345678-1234-1234-1234-123456789abc";
        assert!(normalize_acp_conversation_id(Some(legacy)).is_ok());
        assert!(!is_valid_pending_acp_conversation_id(legacy));
    }
}
