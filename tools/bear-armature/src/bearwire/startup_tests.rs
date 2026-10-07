use super::test_support::{MockDen, MockDenOptions, OpenControl};
use super::*;
use crate::SessionContext;
use std::sync::Arc;

async fn prompt_state(
    http: reqwest::Client,
    session_id: &str,
    conversation_id: Option<&str>,
) -> (
    AdapterState,
    AdapterSharedState,
    crate::PromptResponseGuard,
    Uuid,
) {
    let (mut state, shared) = crate::headless::headless_adapter_state(http);
    let context = SessionContext {
        cwd: "/workspace".to_string(),
        roots: vec!["/workspace".to_string()],
        raw: json!({ "cwd": "/workspace" }),
        conversation_id: conversation_id.map(str::to_string),
        ..Default::default()
    };
    state
        .session_contexts
        .insert(session_id.to_string(), context.clone());
    shared
        .session_contexts
        .lock()
        .await
        .insert(session_id.to_string(), context);
    let response = crate::PromptResponseGuard::new(Value::Null);
    let token = Uuid::new_v4();
    crate::register_prompt_turn_for_session(&shared, session_id, token, None, response.clone())
        .await;
    (state, shared, response, token)
}

async fn assert_canonical_start(session: Value, initial_id: Option<&str>, canonical_id: &str) {
    let den = MockDen::start(MockDenOptions {
        session,
        ..Default::default()
    })
    .await;
    let http = reqwest::Client::new();
    let session_id = Uuid::new_v4().to_string();
    let (mut state, shared, response, token) =
        prompt_state(http.clone(), &session_id, initial_id).await;
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        handle_prompt(
            &http,
            &den.config,
            &mut state,
            &shared,
            response,
            &session_id,
            "ordinary prompt",
            json!({}),
            json!({ "cwd": "/workspace" }),
            initial_id,
            crate::MODE_WRITE,
            token,
        ),
    )
    .await
    .expect("bounded prompt")
    .expect_err("mock stops after recording run.start");
    assert!(
        format!("{error:#}").contains("test stop after run.start"),
        "{error:#}"
    );
    let requests = den.requests.lock().await;
    let open = requests
        .iter()
        .find(|r| r["method"] == "session.open")
        .unwrap();
    let start = requests
        .iter()
        .find(|r| r["method"] == "run.start")
        .unwrap();
    assert_eq!(start["params"]["conversation_id"], canonical_id);
    assert_eq!(
        start["params"]["client_context"]["den_acp_session"]["conversation_id"],
        canonical_id
    );
    for request in [open, start] {
        assert!(request["params"].get("expected_work_source").is_none());
    }
    assert_eq!(
        state.session_contexts[&session_id]
            .conversation_id
            .as_deref(),
        Some(canonical_id)
    );
    assert_eq!(
        shared.session_contexts.lock().await[&session_id]
            .conversation_id
            .as_deref(),
        Some(canonical_id)
    );
}

#[tokio::test]
async fn ordinary_open_forwards_den_canonical_id_instead_of_the_stale_parameter() {
    let canonical = Uuid::new_v4().to_string();
    assert_canonical_start(
        json!({
            "conversation_id": canonical, "resolved_conversation_id": canonical,
            "history_conversation_id": canonical,
            "access": { "state": "executable", "may_select_hat": false },
        }),
        Some("stale-turn-conversation"),
        &canonical,
    )
    .await;
}

#[tokio::test]
async fn pending_open_forwards_the_same_den_selected_id_to_run_start() {
    let canonical = Uuid::new_v4().to_string();
    assert_canonical_start(
        json!({
            "conversation_id": canonical, "resolved_conversation_id": null,
            "history_conversation_id": null,
            "access": { "state": "awaiting_hat", "may_select_hat": true },
        }),
        None,
        &canonical,
    )
    .await;
}

#[tokio::test]
async fn conversation_identity_is_opaque_not_inferred_from_a_prefix() {
    let canonical = "pending-looking-but-executable";
    assert_canonical_start(
        json!({
            "conversation_id": canonical, "resolved_conversation_id": canonical,
            "history_conversation_id": canonical,
            "access": { "state": "executable", "may_select_hat": false },
        }),
        None,
        canonical,
    )
    .await;
}

#[tokio::test]
async fn ordinary_preflight_remains_compatible_with_den_without_additive_capabilities() {
    let den = MockDen::start(MockDenOptions {
        initialize: json!({ "protocol": "bearwire", "version": 1 }),
        ..Default::default()
    })
    .await;
    validate_code_token(&reqwest::Client::new(), &den.config)
        .await
        .unwrap();
    assert_eq!(den.methods().await, ["initialize", "session.state"]);
}

#[tokio::test]
async fn malformed_session_projection_fails_closed_before_run_start() {
    let den = MockDen::start(MockDenOptions {
        session: json!({ "conversation_id": Uuid::new_v4() }),
        ..Default::default()
    })
    .await;
    let http = reqwest::Client::new();
    let session_id = Uuid::new_v4().to_string();
    let (mut state, shared, response, token) = prompt_state(http.clone(), &session_id, None).await;
    let error = handle_prompt(
        &http,
        &den.config,
        &mut state,
        &shared,
        response,
        &session_id,
        "prompt",
        json!({}),
        json!({ "cwd": "/workspace" }),
        None,
        crate::MODE_WRITE,
        token,
    )
    .await
    .expect_err("missing session access must fail closed");
    assert!(format!("{error:#}").contains("session access projection"));
    assert_eq!(den.methods().await, ["session.open"]);
}

#[tokio::test]
async fn late_open_from_superseded_turn_never_overwrites_shared_binding_or_starts() {
    let control = Arc::new(OpenControl::default());
    let den = MockDen::start(MockDenOptions {
        open_control: Some(control.clone()),
        ..Default::default()
    })
    .await;
    let http = reqwest::Client::new();
    let session_id = Uuid::new_v4().to_string();
    let (mut state, shared, response, token) = prompt_state(http.clone(), &session_id, None).await;
    let new_canonical = Uuid::new_v4().to_string();
    let shared_for_rebind = shared.clone();
    let session_for_rebind = session_id.clone();
    let new_canonical_for_rebind = new_canonical.clone();
    let supersede = async {
        control.entered.notified().await;
        crate::register_prompt_turn_for_session(
            &shared_for_rebind,
            &session_for_rebind,
            Uuid::new_v4(),
            Some(new_canonical_for_rebind.clone()),
            crate::PromptResponseGuard::new(Value::Null),
        )
        .await;
        let mut contexts = shared_for_rebind.session_contexts.lock().await;
        let context = contexts.get_mut(&session_for_rebind).unwrap();
        context.conversation_id = Some(new_canonical_for_rebind.clone());
        context.resolved_conversation_id = Some(new_canonical_for_rebind.clone());
        drop(contexts);
        control.release.notify_one();
    };
    let prompt = handle_prompt(
        &http,
        &den.config,
        &mut state,
        &shared,
        response,
        &session_id,
        "prompt",
        json!({}),
        json!({ "cwd": "/workspace" }),
        None,
        crate::MODE_WRITE,
        token,
    );
    let (_, outcome) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(supersede, prompt)
    })
    .await
    .expect("bounded superseded open");
    outcome.unwrap();
    assert_eq!(den.methods().await, ["session.open"]);
    assert_eq!(
        shared.session_contexts.lock().await[&session_id]
            .resolved_conversation_id
            .as_deref(),
        Some(new_canonical.as_str()),
    );
}
