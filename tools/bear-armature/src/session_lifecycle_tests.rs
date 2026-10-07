use super::*;
use axum::{response::Response, routing::post, Json, Router};
use session_lifecycle::{begin_session_restore, commit_restored_session, SessionInteractionKind};
use tests::{
    run_acp_request_for_test, test_adapter_state, test_runtime_config, test_shared_state, ENV_LOCK,
};

fn projection(id: &str, state: SessionAccessState, may_select_hat: bool) -> Value {
    let pending = state == SessionAccessState::AwaitingHat;
    json!({
        "client_session_id": id,
        "conversation_id": if pending { "new-acp-zed-pending" } else { "den-conv-canonical" },
        "resolved_conversation_id": if pending { Value::Null } else { json!("den-conv-canonical") },
        "history_conversation_id": if pending { Value::Null } else { json!("den-conv-canonical") },
        "access": SessionAccess { state, may_select_hat },
        "cwd": "/workspace",
        "current_mode": "ask",
        "conversation_title": "Canonical title",
        "updated_at": "2026-10-06T00:00:00Z"
    })
}

#[derive(Clone, Copy)]
enum Failure {
    Missing,
    Foreign,
    Rpc,
    Http,
    Slow,
}

#[derive(Clone)]
struct MockState {
    session: Arc<TokioMutex<Value>>,
    failure: Arc<TokioMutex<Option<(&'static str, Failure)>>>,
    delay: Arc<TokioMutex<Option<DelayedMethod>>>,
    requests: Arc<TokioMutex<Vec<Value>>>,
}

#[derive(Clone)]
struct DelayedMethod {
    method: &'static str,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl DelayedMethod {
    fn new(method: &'static str) -> Self {
        Self {
            method,
            started: Arc::default(),
            release: Arc::default(),
        }
    }

    async fn wait_started(&self) {
        timeout(Duration::from_secs(5), self.started.notified())
            .await
            .expect("mock request must reach delay");
    }
}

struct MockDen {
    url: String,
    state: MockState,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for MockDen {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn mock_rpc(State(state): State<MockState>, Json(request): Json<Value>) -> Response {
    state.requests.lock().await.push(request.clone());
    let method = request["method"].as_str().unwrap();
    let failure = *state.failure.lock().await;
    if let Some((failed_method, failure)) = failure {
        if method == failed_method {
            match failure {
                Failure::Http => return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response(),
                Failure::Slow => tokio::time::sleep(LOCAL_DEN_INSPECTION_TIMEOUT + Duration::from_secs(1)).await,
                Failure::Rpc => return Json(json!({"id": request["id"], "error": {"code": -32003, "message": "authorization denied"}})).into_response(),
                Failure::Missing => return Json(json!({"id": request["id"], "result": {"kind": "single", "session": null}})).into_response(),
                Failure::Foreign => {
                    let mut session = state.session.lock().await.clone();
                    session["client_session_id"] = json!("somebody-elses-session");
                    return Json(json!({"id": request["id"], "result": {"kind": "single", "session": session}})).into_response();
                }
            }
        }
    }
    let session_id = request
        .pointer("/params/session_id")
        .and_then(Value::as_str);
    let result = match method {
        "initialize" => {
            json!({"protocol": "bearwire", "version": 1, "capabilities": {"session_access": true, "expected_work_source": true}})
        }
        "session.state" => {
            if session_id.is_some() {
                json!({"kind": "single", "session": state.session.lock().await.clone()})
            } else {
                json!({"kind": "list", "sessions": [state.session.lock().await.clone()]})
            }
        }
        "session.open" => {
            let mut session = state.session.lock().await;
            session["client_session_id"] = json!(session_id.unwrap());
            json!({"ok": true, "session": session.clone()})
        }
        "hats.list" => json!({
            "ide_default_hat_id": null, "selected_hat_id": null,
            "hats": [{"id": "22222222-2222-2222-2222-222222222222", "name": "Security review"}]
        }),
        "session.hat.select" => {
            let session = projection(session_id.unwrap(), SessionAccessState::Executable, true);
            *state.session.lock().await = session.clone();
            json!({"ok": true, "hat_id": request["params"]["hat_id"], "session": session, "conversation_id": "den-conv-canonical"})
        }
        "conversation.surface_history" => json!({
            "surface_events": [{"kind": "message", "role": "user", "text": "historical owner message"}],
            "has_more": false, "next_before": null
        }),
        "resource.update" => json!({"ok": true}),
        _ => return Json(
            json!({"id": request["id"], "error": {"code": -32601, "message": "unexpected method"}}),
        )
        .into_response(),
    };
    let delay = state.delay.lock().await.clone();
    if let Some(delay) = delay.filter(|delay| delay.method == method) {
        // Capture the response before yielding so the test can deliver an old
        // positive access snapshot after a newer source is already projected.
        delay.started.notify_one();
        delay.release.notified().await;
    }
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result})).into_response()
}

async fn mock_den(session: Value) -> MockDen {
    let state = MockState {
        session: Arc::new(TokioMutex::new(session)),
        failure: Arc::new(TokioMutex::new(None)),
        delay: Arc::new(TokioMutex::new(None)),
        requests: Arc::new(TokioMutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/bearwire/v1/rpc", post(mock_rpc))
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    MockDen { url, state, server }
}

async fn seeded(
    state: SessionAccessState,
    may_select_hat: bool,
) -> (AdapterState, AdapterSharedState) {
    let mut adapter = test_adapter_state("editor", Path::new("/workspace"));
    let shared = test_shared_state();
    apply_den_session_projection(
        &mut adapter,
        &shared,
        "editor",
        &projection("editor", state, may_select_hat),
    )
    .await
    .unwrap();
    (adapter, shared)
}

fn request(id: &str, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[tokio::test]
async fn lifecycle_projection_updates_both_caches_without_inventing_access() {
    let (mut adapter, shared) = seeded(SessionAccessState::AwaitingHat, true).await;
    assert!(adapter.session_contexts["editor"]
        .resolved_conversation_id
        .is_none());
    let mut executable = projection("editor", SessionAccessState::Executable, false);
    executable["conversation_id"] = json!("opaque/source-id");
    executable["resolved_conversation_id"] = json!("opaque/source-id");
    executable["history_conversation_id"] = json!("opaque/source-id");
    apply_den_session_projection(&mut adapter, &shared, "editor", &executable)
        .await
        .unwrap();
    let contexts = shared.session_contexts.lock().await;
    assert_eq!(
        contexts["editor"].resolved_conversation_id.as_deref(),
        Some("opaque/source-id")
    );
    assert_eq!(
        adapter.session_contexts["editor"].access,
        contexts["editor"].access
    );
    assert_eq!(
        adapter.session_contexts["editor"].raw["den_acp_session"],
        executable
    );
    assert!(SessionContext::default().access.is_none());
}

#[tokio::test]
async fn lifecycle_malformed_or_foreign_projection_preserves_known_binding() {
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
    let original = adapter.session_contexts["editor"].raw.clone();
    let mut malformed = projection("editor", SessionAccessState::Executable, false);
    malformed.as_object_mut().unwrap().remove("access");
    let mut bad_pending = projection("editor", SessionAccessState::AwaitingHat, true);
    bad_pending["history_conversation_id"] = json!("old-history");
    let mut blank = projection("editor", SessionAccessState::Executable, false);
    blank["resolved_conversation_id"] = json!(" ");
    let mut missing_history = projection("editor", SessionAccessState::Executable, false);
    missing_history
        .as_object_mut()
        .unwrap()
        .remove("history_conversation_id");
    let mut mismatched_history = projection("editor", SessionAccessState::Executable, false);
    mismatched_history["history_conversation_id"] = json!("another-canonical-source");
    for session in [
        malformed,
        bad_pending,
        blank,
        missing_history,
        mismatched_history,
        projection("foreign", SessionAccessState::Executable, false),
        projection("editor", SessionAccessState::ReadOnly, true),
    ] {
        assert!(
            apply_den_session_projection(&mut adapter, &shared, "editor", &session)
                .await
                .is_err()
        );
        assert_eq!(adapter.session_contexts["editor"].raw, original);
        assert_eq!(shared.session_contexts.lock().await["editor"].raw, original);
    }
    assert!(apply_den_session_projection(
        &mut adapter,
        &shared,
        "unknown",
        &projection("unknown", SessionAccessState::Executable, false)
    )
    .await
    .is_err());
    assert!(!shared.session_contexts.lock().await.contains_key("unknown"));
}

#[tokio::test]
async fn lifecycle_new_projects_pending_default_and_history_access() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("BEARS_BEARWIRE", "true");
    for access in [
        SessionAccessState::AwaitingHat,
        SessionAccessState::Executable,
        SessionAccessState::ReadOnly,
    ] {
        let den = mock_den(projection(
            "unused",
            access,
            access != SessionAccessState::ReadOnly,
        ))
        .await;
        let mut runtime = test_runtime_config(den.url.clone());
        let mut adapter = AdapterState::default();
        let shared = test_shared_state();
        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &reqwest::Client::new(),
                &mut runtime,
                &mut adapter,
                &shared,
                request("new", "session/new", json!({"cwd": "/workspace"})),
            )
            .await
        })
        .await;
        result.unwrap();
        let response = output.iter().find(|frame| frame["id"] == "new").unwrap();
        assert_eq!(
            response["result"]["_meta"]["bears"]["access"]["state"],
            json!(access)
        );
        let id = response["result"]["sessionId"].as_str().unwrap();
        assert_eq!(adapter.session_contexts[id].access.unwrap().state, access);
        assert_eq!(
            shared.session_contexts.lock().await[id]
                .access
                .unwrap()
                .state,
            access
        );
        if access == SessionAccessState::AwaitingHat {
            assert!(adapter.session_contexts[id]
                .resolved_conversation_id
                .is_none());
        }
        if access == SessionAccessState::ReadOnly {
            assert_eq!(response["result"]["configOptions"], json!([]));
            assert!(response["result"].get("modes").is_none());
        }
    }
}

#[tokio::test]
async fn lifecycle_list_uses_real_den_client_session_id() {
    let den = mock_den(projection("editor", SessionAccessState::Executable, false)).await;
    let mut adapter = AdapterState::default();
    let mut runtime = test_runtime_config(den.url.clone());
    let shared = test_shared_state();
    let (result, output) = capture_json_output_for_test(|| async {
        run_acp_request_for_test(
            &reqwest::Client::new(),
            &mut runtime,
            &mut adapter,
            &shared,
            request("list", "session/list", json!({})),
        )
        .await
    })
    .await;
    result.unwrap();
    let response = output.iter().find(|frame| frame["id"] == "list").unwrap();
    assert_eq!(response["result"]["sessions"][0]["sessionId"], "editor");
    assert_eq!(
        response["result"]["sessions"][0]["title"],
        "Canonical title"
    );
}

#[tokio::test]
async fn lifecycle_failed_load_and_resume_never_create_pending_sessions() {
    for method in ["session/load", "session/resume"] {
        for failure in [
            Failure::Missing,
            Failure::Foreign,
            Failure::Rpc,
            Failure::Http,
            Failure::Slow,
        ] {
            let den = mock_den(projection("editor", SessionAccessState::Executable, false)).await;
            *den.state.failure.lock().await = Some(("session.state", failure));
            let mut runtime = test_runtime_config(den.url.clone());
            let mut adapter = AdapterState::default();
            let shared = test_shared_state();
            let (result, output) = capture_json_output_for_test(|| async {
                let http = reqwest::Client::new();
                run_acp_request_for_test(&http, &mut runtime, &mut adapter, &shared,
                    request("restore", method, json!({"sessionId": "editor", "cwd": "/workspace"}))).await?;
                run_acp_request_for_test(&http, &mut runtime, &mut adapter, &shared,
                    request("later", "session/prompt", json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "do work"}]}))).await
            }).await;
            result.unwrap();
            for id in ["restore", "later"] {
                let response = output.iter().find(|frame| frame["id"] == id).unwrap();
                assert!(response.get("error").is_some(), "{method}: {response}");
            }
            assert!(adapter.session_contexts.is_empty());
            assert!(shared.session_contexts.lock().await.is_empty());
            assert!(shared.prompted_sessions.lock().await.is_empty());
            assert!(!den
                .state
                .requests
                .lock()
                .await
                .iter()
                .any(|request| request["method"] == "session.open"));
        }
    }
}

#[tokio::test]
async fn lifecycle_failed_history_load_preserves_known_caches() {
    let den = mock_den(projection("editor", SessionAccessState::ReadOnly, false)).await;
    *den.state.failure.lock().await = Some(("conversation.surface_history", Failure::Rpc));
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
    let original = adapter.session_contexts["editor"].raw.clone();
    assert!(handle_session_load(
        &reqwest::Client::new(),
        runtime_config(&den).config.as_ref().unwrap(),
        &mut adapter,
        &shared,
        json!("load"),
        &json!({"sessionId": "editor"})
    )
    .await
    .is_err());
    assert_eq!(adapter.session_contexts["editor"].raw, original);
    assert_eq!(shared.session_contexts.lock().await["editor"].raw, original);
}

fn runtime_config(den: &MockDen) -> RuntimeConfig {
    test_runtime_config(den.url.clone())
}

#[tokio::test]
async fn lifecycle_pending_restore_has_no_history_and_uses_den_selection_eligibility() {
    for method in ["session/load", "session/resume"] {
        let den = mock_den(projection("editor", SessionAccessState::AwaitingHat, true)).await;
        let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
        shared
            .prompted_sessions
            .lock()
            .await
            .insert("editor".into());
        let mut runtime = runtime_config(&den);
        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &reqwest::Client::new(),
                &mut runtime,
                &mut adapter,
                &shared,
                request("pending", method, json!({"sessionId": "editor"})),
            )
            .await
        })
        .await;
        result.unwrap();
        let response = output
            .iter()
            .find(|frame| frame["id"] == "pending")
            .unwrap();
        assert_eq!(
            response["result"]["_meta"]["bears"]["access"]["state"],
            "awaiting_hat"
        );
        assert!(adapter.session_contexts["editor"]
            .resolved_conversation_id
            .is_none());
        assert!(!shared.prompted_sessions.lock().await.contains("editor"));
        assert!(!den
            .state
            .requests
            .lock()
            .await
            .iter()
            .any(|request| request["method"] == "conversation.surface_history"));
        assert!(reserve_session_interaction(
            &adapter,
            &shared,
            "editor",
            SessionInteractionKind::SelectHat
        )
        .await
        .is_ok());
    }
}

#[tokio::test]
async fn lifecycle_read_only_load_replays_but_blocks_all_productive_and_config_paths() {
    let den = mock_den(projection("editor", SessionAccessState::ReadOnly, false)).await;
    let mut runtime = runtime_config(&den);
    let mut adapter = AdapterState::default();
    let shared = test_shared_state();
    let (result, output) = capture_json_output_for_test(|| async {
        let http = reqwest::Client::new();
        run_acp_request_for_test(&http, &mut runtime, &mut adapter, &shared,
            request("history", "session/load", json!({"sessionId": "editor"}))).await?;
        for (id, method, params) in [
            ("mode", "session/set_mode", json!({"sessionId": "editor", "modeId": "write"})),
            ("config-mode", "session/set_config_option", json!({"sessionId": "editor", "configId": "mode", "value": "write"})),
            ("model", "session/set_config_option", json!({"sessionId": "editor", "configId": "model", "value": "auto"})),
            ("prompt", "session/prompt", json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "work"}]})),
        ] {
            run_acp_request_for_test(&http, &mut runtime, &mut adapter, &shared, request(id, method, params)).await?;
        }
        for command in [LocalSlashCommand::Compact, LocalSlashCommand::Focus] {
            let params = json!({"sessionId": "editor", "prompt": [{"type": "text", "text": if command == LocalSlashCommand::Compact { "/compact" } else { "/focus" }}]});
            assert!(handle_local_slash_prompt(Some(&http), runtime.config.as_ref(), &mut adapter, &shared, json!("slash"), params, command).await.is_err());
        }
        Ok::<_, anyhow::Error>(())
    }).await;
    result.unwrap();
    let loaded = output
        .iter()
        .position(|frame| frame["id"] == "history")
        .unwrap();
    assert!(output[..loaded]
        .iter()
        .any(|frame| frame.to_string().contains("historical owner message")));
    assert_eq!(
        output[loaded]["result"]["_meta"]["bears"]["access"]["state"],
        "read_only"
    );
    for id in ["mode", "config-mode", "model", "prompt"] {
        assert!(output
            .iter()
            .find(|frame| frame["id"] == id)
            .unwrap()
            .get("error")
            .is_some());
    }
    let selected = hat_report(
        &reqwest::Client::new(),
        runtime.config.as_ref().unwrap(),
        &mut adapter,
        &shared,
        "editor",
        "/hat Security review",
    )
    .await;
    assert!(selected.contains("read-only history"));
    assert!(!den
        .state
        .requests
        .lock()
        .await
        .iter()
        .any(|request| matches!(
            request["method"].as_str(),
            Some(
                "session.open"
                    | "run.start"
                    | "session.model.set"
                    | "session.hat.select"
                    | "session.compact"
            )
        )));
}

#[tokio::test]
async fn lifecycle_hat_failures_and_listing_leave_selection_retryable() {
    let den = mock_den(projection("editor", SessionAccessState::AwaitingHat, true)).await;
    let runtime = runtime_config(&den);
    let config = runtime.config.as_ref().unwrap();
    let (mut adapter, shared) = seeded(SessionAccessState::AwaitingHat, true).await;
    let http = reqwest::Client::new();
    let listed = hat_report(&http, config, &mut adapter, &shared, "editor", "/hat").await;
    assert!(listed.contains("Security review"));
    let invalid = hat_report(
        &http,
        config,
        &mut adapter,
        &shared,
        "editor",
        "/hat invalid",
    )
    .await;
    assert!(invalid.contains("No hat named"));
    for method in ["hats.list", "session.hat.select"] {
        *den.state.failure.lock().await = Some((method, Failure::Rpc));
        let failed = hat_report(
            &http,
            config,
            &mut adapter,
            &shared,
            "editor",
            "/hat Security review",
        )
        .await;
        assert!(failed.contains("Could not"));
        assert!(shared.prompted_sessions.lock().await.is_empty());
        assert!(adapter.session_contexts["editor"]
            .interaction_reservation
            .lock()
            .unwrap()
            .is_none());
    }
    *den.state.failure.lock().await = None;
    let chosen = hat_report(
        &http,
        config,
        &mut adapter,
        &shared,
        "editor",
        "/hat security REVIEW",
    )
    .await;
    assert!(chosen.contains("Wearing Security review"), "{chosen}");
    assert_eq!(
        adapter.session_contexts["editor"]
            .resolved_conversation_id
            .as_deref(),
        Some("den-conv-canonical")
    );
    assert!(shared.prompted_sessions.lock().await.contains("editor"));
    let again = hat_report(
        &http,
        config,
        &mut adapter,
        &shared,
        "editor",
        "/hat Security review",
    )
    .await;
    assert!(again.contains("already succeeded"), "{again}");
}

#[tokio::test]
async fn lifecycle_failed_prompt_and_diagnostics_do_not_consume_hat_selection() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("BEARS_BEARWIRE", "true");
    let den = mock_den(projection("editor", SessionAccessState::Executable, true)).await;
    *den.state.failure.lock().await = Some(("initialize", Failure::Rpc));
    let mut runtime = runtime_config(&den);
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, true).await;
    let (result, _) = capture_json_output_for_test(|| async {
        run_acp_request_for_test(
            &reqwest::Client::new(),
            &mut runtime,
            &mut adapter,
            &shared,
            request(
                "denied",
                "session/prompt",
                json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "work"}]}),
            ),
        )
        .await?;
        handle_local_slash_prompt(
            None,
            None,
            &mut adapter,
            &shared,
            json!("diagnostic"),
            json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "/conversation"}]}),
            LocalSlashCommand::Conversation,
        )
        .await
    })
    .await;
    result.unwrap();
    assert!(shared.prompted_sessions.lock().await.is_empty());
    let chosen = hat_report(
        &reqwest::Client::new(),
        runtime.config.as_ref().unwrap(),
        &mut adapter,
        &shared,
        "editor",
        "/hat Security review",
    )
    .await;
    assert!(chosen.contains("Wearing"), "{chosen}");
}

#[tokio::test]
async fn lifecycle_initial_prompt_and_hat_are_serialized_and_failures_release_reservations() {
    let (adapter, shared) = seeded(SessionAccessState::Executable, true).await;
    let reservation = reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive,
    )
    .await
    .unwrap();
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::SelectHat
    )
    .await
    .is_err());
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive
    )
    .await
    .is_err());
    drop(reservation);
    let reservation = reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::SelectHat,
    )
    .await
    .unwrap();
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive
    )
    .await
    .is_err());
    drop(reservation);
    let reservation = reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive,
    )
    .await
    .unwrap();
    mark_session_productive_interaction(&shared, "editor").await;
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::SelectHat
    )
    .await
    .is_err());
    assert!(
        reserve_session_interaction(
            &adapter,
            &shared,
            "editor",
            SessionInteractionKind::Productive
        )
        .await
        .unwrap()
        .is_none(),
        "steering must remain available after admission"
    );
    drop(reservation);
}

#[tokio::test]
async fn lifecycle_opaque_conversation_binding_is_stale_turn_gated() {
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
    let config = Config {
        api_url: "http://127.0.0.1".into(),
        bear: "test-bear".into(),
        token: "test".into(),
        client: "zed".into(),
    };
    let token = Uuid::new_v4();
    register_prompt_turn_for_session(
        &shared,
        "editor",
        token,
        None,
        PromptResponseGuard::new(Value::Null),
    )
    .await;
    for id in ["den-conv-next", "opaque/source-next"] {
        // Avoid an unrelated environment publish in this narrow event test.
        adapter
            .session_contexts
            .get_mut("editor")
            .unwrap()
            .thread_title = None;
        shared
            .session_contexts
            .lock()
            .await
            .get_mut("editor")
            .unwrap()
            .thread_title = None;
        handle_conversation_resolved_projection(
            &config,
            &mut adapter,
            &shared,
            "editor",
            token,
            id,
        )
        .await
        .unwrap();
        assert_eq!(
            adapter.session_contexts["editor"]
                .resolved_conversation_id
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            shared.session_contexts.lock().await["editor"]
                .resolved_conversation_id
                .as_deref(),
            Some(id)
        );
    }
    handle_conversation_resolved_projection(
        &config,
        &mut adapter,
        &shared,
        "editor",
        Uuid::new_v4(),
        "old-conversation",
    )
    .await
    .unwrap();
    assert_eq!(
        adapter.session_contexts["editor"]
            .resolved_conversation_id
            .as_deref(),
        Some("opaque/source-next")
    );
    for id in ["", " ", "bad\nidentifier"] {
        assert!(handle_conversation_resolved_projection(
            &config,
            &mut adapter,
            &shared,
            "editor",
            token,
            id
        )
        .await
        .is_err());
    }
    assert_eq!(
        adapter.session_contexts["editor"].access.unwrap().state,
        SessionAccessState::Executable
    );
}

#[tokio::test]
async fn lifecycle_den_rejected_turn_does_not_lock_hat_selection() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("BEARS_BEARWIRE", "true");
    let den = mock_den(projection("editor", SessionAccessState::Executable, true)).await;
    *den.state.failure.lock().await = Some(("run.start", Failure::Rpc));
    let runtime = runtime_config(&den);
    let config = runtime.config.as_ref().unwrap();
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, true).await;
    let reservation = reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive,
    )
    .await
    .unwrap();
    let token = Uuid::new_v4();
    let response = PromptResponseGuard::new(json!("denied-turn"));
    register_prompt_turn_for_session(&shared, "editor", token, None, response.clone()).await;
    let result = handle_prompt(
        &reqwest::Client::new(),
        config,
        &mut adapter,
        &shared,
        response,
        json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "must be denied"}]}),
        token,
    )
    .await;
    assert!(result.is_err());
    drop(reservation);
    assert!(shared.prompted_sessions.lock().await.is_empty());
    let selected = hat_report(
        &reqwest::Client::new(),
        config,
        &mut adapter,
        &shared,
        "editor",
        "/hat Security review",
    )
    .await;
    assert!(selected.contains("Wearing"), "{selected}");
}

#[tokio::test]
async fn lifecycle_admitted_launch_locks_selection_even_before_terminal_delivery() {
    let (adapter, shared) = seeded(SessionAccessState::Executable, true).await;
    let reservation = reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::Productive,
    )
    .await
    .unwrap();
    let token = Uuid::new_v4();
    register_prompt_turn_for_session(
        &shared,
        "editor",
        token,
        None,
        PromptResponseGuard::new(Value::Null),
    )
    .await;
    assert!(!bind_prompt_turn_run(&shared, "editor", Uuid::new_v4(), "stale-run").await);
    assert!(shared.prompted_sessions.lock().await.is_empty());
    assert!(bind_prompt_turn_run(&shared, "editor", token, "admitted-run").await);
    assert!(shared.prompted_sessions.lock().await.contains("editor"));
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::SelectHat
    )
    .await
    .is_err());
    assert!(adapter.session_contexts["editor"]
        .interaction_reservation
        .lock()
        .unwrap()
        .is_none());
    drop(reservation);
}

#[tokio::test]
async fn lifecycle_stale_adapter_access_cannot_override_shared_denials_or_reopen_removal() {
    let den = mock_den(projection("editor", SessionAccessState::Executable, false)).await;
    let mut runtime = runtime_config(&den);
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
    let mut newer_adapter = adapter.clone();
    apply_den_session_projection(
        &mut newer_adapter,
        &shared,
        "editor",
        &projection("editor", SessionAccessState::ReadOnly, false),
    )
    .await
    .unwrap();
    for removed in [false, true] {
        if removed {
            shared.session_contexts.lock().await.remove("editor");
        }
        for kind in [
            SessionInteractionKind::Productive,
            SessionInteractionKind::Configure,
            SessionInteractionKind::SelectHat,
        ] {
            assert!(
                require_session_interaction(&adapter, &shared, "editor", kind)
                    .await
                    .is_err()
            );
        }
        let (result, output) = capture_json_output_for_test(|| async {
            // Exercise the real request handler without the fixture helper that
            // seeds caches; a surviving adapter snapshot is deliberately stale.
            handle_request(&reqwest::Client::new(), &mut runtime, &mut adapter, &shared,
                request_from_value(request("stale", "session/prompt", json!({"sessionId": "editor", "prompt": [{"type": "text", "text": "do not reopen"}]})))?).await
        }).await;
        result.unwrap();
        assert!(output
            .iter()
            .find(|frame| frame["id"] == "stale")
            .unwrap()
            .get("error")
            .is_some());
        assert!(den.state.requests.lock().await.is_empty());
        assert_eq!(
            adapter.session_contexts["editor"].access.unwrap().state,
            SessionAccessState::Executable
        );
    }
    assert!(shared.session_contexts.lock().await.is_empty());
}

#[tokio::test]
async fn lifecycle_binding_events_never_promote_pending_unverified_or_read_only_access() {
    let config = Config {
        api_url: "http://127.0.0.1".into(),
        bear: "test-bear".into(),
        token: "test".into(),
        client: "zed".into(),
    };
    for access in [
        None,
        Some(SessionAccess {
            state: SessionAccessState::AwaitingHat,
            may_select_hat: true,
        }),
        Some(SessionAccess {
            state: SessionAccessState::ReadOnly,
            may_select_hat: false,
        }),
    ] {
        let mut adapter = AdapterState::default();
        let shared = test_shared_state();
        let context = SessionContext {
            access,
            raw: json!({}),
            ..Default::default()
        };
        adapter
            .session_contexts
            .insert("editor".into(), context.clone());
        shared
            .session_contexts
            .lock()
            .await
            .insert("editor".into(), context);
        let token = Uuid::new_v4();
        register_prompt_turn_for_session(
            &shared,
            "editor",
            token,
            None,
            PromptResponseGuard::new(Value::Null),
        )
        .await;
        handle_conversation_resolved_projection(
            &config,
            &mut adapter,
            &shared,
            "editor",
            token,
            "den-conv-event-only",
        )
        .await
        .unwrap();
        assert_eq!(adapter.session_contexts["editor"].access, access);
        assert_eq!(
            shared.session_contexts.lock().await["editor"].access,
            access
        );
        assert!(require_session_interaction(
            &adapter,
            &shared,
            "editor",
            SessionInteractionKind::Productive
        )
        .await
        .is_err());
        adapter.session_contexts.clear();
        shared.session_contexts.lock().await.clear();
        assert!(handle_conversation_resolved_projection(
            &config,
            &mut adapter,
            &shared,
            "editor",
            token,
            "den-conv-unknown"
        )
        .await
        .is_err());
        assert!(adapter.session_contexts.is_empty());
        assert!(shared.session_contexts.lock().await.is_empty());
    }
}

#[tokio::test]
async fn lifecycle_busy_restore_preserves_initial_reservation_and_fresh_restore_reuses_it() {
    for (kind, access) in [
        (
            SessionInteractionKind::Productive,
            SessionAccessState::Executable,
        ),
        (
            SessionInteractionKind::SelectHat,
            SessionAccessState::AwaitingHat,
        ),
    ] {
        let den = mock_den(projection("editor", access, true)).await;
        let runtime = runtime_config(&den);
        let config = runtime.config.as_ref().unwrap();
        let (mut adapter, shared) = seeded(access, true).await;
        let original = adapter.session_contexts["editor"].clone();
        let initial = reserve_session_interaction(&adapter, &shared, "editor", kind)
            .await
            .unwrap();
        assert!(initial.is_some());
        assert!(restore_session_from_den(
            &reqwest::Client::new(),
            config,
            &mut adapter,
            &shared,
            &json!({"sessionId": "editor"})
        )
        .await
        .is_err());
        assert!(handle_session_load(
            &reqwest::Client::new(),
            config,
            &mut adapter,
            &shared,
            json!("busy"),
            &json!({"sessionId": "editor"})
        )
        .await
        .is_err());
        assert!(
            den.state.requests.lock().await.is_empty(),
            "busy restore must fail before a network read"
        );
        assert_eq!(adapter.session_contexts["editor"].raw, original.raw);
        assert_eq!(
            shared.session_contexts.lock().await["editor"].raw,
            original.raw
        );
        assert!(Arc::ptr_eq(
            &shared.session_contexts.lock().await["editor"].interaction_reservation,
            &original.interaction_reservation
        ));
        assert!(!original.interaction_reservation.lock().unwrap().is_none());
        assert!(
            reserve_session_interaction(&adapter, &shared, "editor", kind)
                .await
                .is_err()
        );
        drop(initial);
        restore_session_from_den(
            &reqwest::Client::new(),
            config,
            &mut adapter,
            &shared,
            &json!({"sessionId": "editor"}),
        )
        .await
        .unwrap();
        assert!(Arc::ptr_eq(
            &adapter.session_contexts["editor"].interaction_reservation,
            &original.interaction_reservation
        ));
        assert!(Arc::ptr_eq(
            &shared.session_contexts.lock().await["editor"].interaction_reservation,
            &original.interaction_reservation
        ));
        assert!(original.interaction_reservation.lock().unwrap().is_none());
        assert!(
            reserve_session_interaction(&adapter, &shared, "editor", kind)
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn lifecycle_delayed_restore_cannot_overwrite_new_source_or_replay_stale_history() {
    for method in ["session/resume", "session/load"] {
        let den = mock_den(projection("editor", SessionAccessState::Executable, true)).await;
        let delay = DelayedMethod::new(if method == "session/load" {
            "conversation.surface_history"
        } else {
            "session.state"
        });
        *den.state.delay.lock().await = Some(delay.clone());
        let runtime = runtime_config(&den);
        let config = runtime.config.clone().unwrap();
        let (mut adapter, shared) = seeded(SessionAccessState::Executable, true).await;
        let original = adapter.session_contexts["editor"].clone();
        let mut restoring_adapter = adapter.clone();
        let restoring_shared = shared.clone();
        let task = tokio::spawn(async move {
            let (result, output) = capture_json_output_for_test(|| async {
                if method == "session/load" {
                    handle_session_load(
                        &reqwest::Client::new(),
                        &config,
                        &mut restoring_adapter,
                        &restoring_shared,
                        json!("stale-load"),
                        &json!({"sessionId": "editor"}),
                    )
                    .await
                } else {
                    restore_session_from_den(
                        &reqwest::Client::new(),
                        &config,
                        &mut restoring_adapter,
                        &restoring_shared,
                        &json!({"sessionId": "editor"}),
                    )
                    .await
                    .map(|_| ())
                }
            })
            .await;
            (result, output, restoring_adapter)
        });
        delay.wait_started().await;
        for kind in [
            SessionInteractionKind::Productive,
            SessionInteractionKind::SelectHat,
            SessionInteractionKind::Configure,
        ] {
            assert!(
                require_session_interaction(&adapter, &shared, "editor", kind)
                    .await
                    .is_err()
            );
        }
        assert!(begin_session_restore(&shared, "editor").await.is_err());
        let mut newer = projection("editor", SessionAccessState::Executable, false);
        for key in [
            "conversation_id",
            "resolved_conversation_id",
            "history_conversation_id",
        ] {
            newer[key] = json!("den-conv-new-source");
        }
        apply_den_session_projection(&mut adapter, &shared, "editor", &newer)
            .await
            .unwrap();
        mark_session_productive_interaction(&shared, "editor").await;
        assert!(
            !original.interaction_reservation.lock().unwrap().is_none(),
            "successful interaction must not clear a restore reservation"
        );
        assert!(
            reserve_session_interaction(
                &adapter,
                &shared,
                "editor",
                SessionInteractionKind::Productive
            )
            .await
            .is_err(),
            "completed marker must not bypass a live restore fence"
        );
        delay.release.notify_one();
        let (result, output, restored_adapter) = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(format!("{:#}", result.unwrap_err()).contains("Session changed"));
        assert!(
            !output
                .iter()
                .any(|frame| frame.to_string().contains("historical owner message")),
            "stale history must not be replayed: {output:#?}"
        );
        assert_eq!(
            restored_adapter.session_contexts["editor"].raw,
            original.raw
        );
        assert_eq!(
            adapter.session_contexts["editor"].raw["den_acp_session"],
            newer
        );
        assert_eq!(
            shared.session_contexts.lock().await["editor"].raw["den_acp_session"],
            newer
        );
        assert!(shared.prompted_sessions.lock().await.contains("editor"));
        assert!(original.interaction_reservation.lock().unwrap().is_none());
        *den.state.delay.lock().await = None;
        *den.state.session.lock().await = newer;
        restore_session_from_den(
            &reqwest::Client::new(),
            runtime.config.as_ref().unwrap(),
            &mut adapter,
            &shared,
            &json!({"sessionId": "editor"}),
        )
        .await
        .unwrap();
        assert_eq!(
            adapter.session_contexts["editor"]
                .resolved_conversation_id
                .as_deref(),
            Some("den-conv-new-source")
        );
        assert!(shared.prompted_sessions.lock().await.contains("editor"));
        assert!(reserve_session_interaction(
            &adapter,
            &shared,
            "editor",
            SessionInteractionKind::Productive
        )
        .await
        .unwrap()
        .is_none());
    }
}

#[tokio::test]
async fn lifecycle_unknown_restore_commit_is_fenced_without_placeholder_state() {
    let den = mock_den(projection("editor", SessionAccessState::Executable, true)).await;
    let runtime = runtime_config(&den);
    let shared = test_shared_state();
    let mut adapter = AdapterState::default();
    let first = begin_session_restore(&shared, "editor").await.unwrap();
    let second = begin_session_restore(&shared, "editor").await.unwrap();
    assert!(shared.session_contexts.lock().await.is_empty());
    let fresh = projection("editor", SessionAccessState::Executable, true);
    let context = session_context_from_den_session(&json!({}), &fresh).unwrap();
    commit_restored_session(
        runtime.config.as_ref().unwrap(),
        &mut adapter,
        &shared,
        &first,
        &fresh,
        context.clone(),
    )
    .await
    .unwrap();
    let reservation = adapter.session_contexts["editor"]
        .interaction_reservation
        .clone();
    assert!(!reservation.lock().unwrap().is_none());
    let mut stale = fresh.clone();
    stale["conversation_title"] = json!("stale replacement");
    assert!(commit_restored_session(
        runtime.config.as_ref().unwrap(),
        &mut adapter,
        &shared,
        &second,
        &stale,
        context
    )
    .await
    .is_err());
    assert_eq!(
        adapter.session_contexts["editor"].raw["den_acp_session"],
        fresh
    );
    assert_eq!(
        shared.session_contexts.lock().await["editor"].raw["den_acp_session"],
        fresh
    );
    assert!(Arc::ptr_eq(
        &shared.session_contexts.lock().await["editor"].interaction_reservation,
        &reservation
    ));
    drop(second);
    assert!(
        !reservation.lock().unwrap().is_none(),
        "stale restore's drop must not release the winner's reservation"
    );
    drop(first);
    assert!(reservation.lock().unwrap().is_none());
    assert!(reserve_session_interaction(
        &adapter,
        &shared,
        "editor",
        SessionInteractionKind::SelectHat
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn lifecycle_restore_generation_rejects_aba_source_updates_and_session_removal() {
    for remove in [false, true] {
        let den = mock_den(projection("editor", SessionAccessState::Executable, true)).await;
        let runtime = runtime_config(&den);
        let (mut adapter, shared) = seeded(SessionAccessState::Executable, true).await;
        let original = adapter.session_contexts["editor"].clone();
        let restore = begin_session_restore(&shared, "editor").await.unwrap();
        let fresh = projection("editor", SessionAccessState::Executable, true);
        if remove {
            shared.session_contexts.lock().await.remove("editor");
        } else {
            let changed = projection("editor", SessionAccessState::ReadOnly, false);
            apply_den_session_projection(&mut adapter, &shared, "editor", &changed)
                .await
                .unwrap();
            apply_den_session_projection(&mut adapter, &shared, "editor", &fresh)
                .await
                .unwrap();
            assert_eq!(
                adapter.session_contexts["editor"].raw, original.raw,
                "test must return to the identical projection"
            );
        }
        let context = session_context_from_den_session(&json!({}), &fresh).unwrap();
        assert!(commit_restored_session(
            runtime.config.as_ref().unwrap(),
            &mut adapter,
            &shared,
            &restore,
            &fresh,
            context
        )
        .await
        .is_err());
        if remove {
            assert!(
                shared.session_contexts.lock().await.is_empty(),
                "restore must not resurrect a closed session"
            );
        }
        drop(restore);
        assert!(original.interaction_reservation.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn lifecycle_transport_failure_preserves_known_state_on_load_and_resume() {
    let mut den = mock_den(projection("editor", SessionAccessState::Executable, false)).await;
    let url = den.url.clone();
    den.server.abort();
    (&mut den.server).await.unwrap_err();
    let (mut adapter, shared) = seeded(SessionAccessState::Executable, false).await;
    let original = adapter.session_contexts["editor"].raw.clone();
    let mut runtime = test_runtime_config(url);
    for method in ["session/load", "session/resume"] {
        let (result, output) = capture_json_output_for_test(|| async {
            run_acp_request_for_test(
                &reqwest::Client::new(),
                &mut runtime,
                &mut adapter,
                &shared,
                request("transport", method, json!({"sessionId": "editor"})),
            )
            .await
        })
        .await;
        result.unwrap();
        assert!(output
            .iter()
            .find(|frame| frame["id"] == "transport")
            .unwrap()
            .get("error")
            .is_some());
        assert_eq!(adapter.session_contexts["editor"].raw, original);
        assert_eq!(shared.session_contexts.lock().await["editor"].raw, original);
    }
}
