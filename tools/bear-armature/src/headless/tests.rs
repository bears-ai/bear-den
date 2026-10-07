use super::*;
use crate::bearwire::test_support::{allowed_checkout, MockDen, MockDenOptions};

fn env(work_order_id: Uuid) -> HeadlessEnv {
    HeadlessEnv {
        work_order_id,
        workspace: "/workspace".to_string(),
        deadline: Duration::from_secs(60),
    }
}

async fn reject_checkout(work_order_id: Uuid, checkout: Value) -> String {
    let den = MockDen::start(MockDenOptions {
        checkout,
        ..Default::default()
    })
    .await;
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        run_headless_turn(&reqwest::Client::new(), &den.config, &env(work_order_id)),
    )
    .await
    .expect("bounded startup")
    .expect_err("checkout must prevent startup");
    assert_eq!(
        den.methods().await,
        ["initialize", "session.state", "work.checkout"]
    );
    format!("{error:#}")
}

#[test]
fn work_order_env_requires_an_exact_uuid_not_a_prefixed_id() {
    let id = Uuid::new_v4();
    assert_eq!(parse_work_order_id(&id.to_string()).unwrap(), id);
    for value in [
        String::new(),
        "not-a-uuid".to_string(),
        format!("work-{id}"),
        Uuid::nil().to_string(),
    ] {
        assert!(parse_work_order_id(&value).is_err(), "{value:?}");
    }
}

#[tokio::test]
async fn denied_checkout_never_opens_or_starts_a_session() {
    let run_id = Uuid::new_v4();
    let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
    checkout["ok"] = json!(false);
    assert!(reject_checkout(run_id, checkout).await.contains("denied"));

    let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
    checkout["gate"] = json!({
        "status": "rejected", "reason": "checkpoint_required", "disposition": "require_checkpoint",
    });
    assert!(reject_checkout(run_id, checkout)
        .await
        .contains("did not allow dispatch"));
}

#[tokio::test]
async fn wrong_work_run_or_gate_binding_never_starts() {
    let run_id = Uuid::new_v4();
    let checkout = allowed_checkout(Uuid::new_v4(), Uuid::new_v4(), 4);
    assert!(reject_checkout(run_id, checkout)
        .await
        .contains("different Work run"));

    let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
    checkout["gate"]["binding"]["work_run_id"] = json!(Uuid::new_v4());
    assert!(reject_checkout(run_id, checkout)
        .await
        .contains("different Work run"));

    let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
    checkout["gate"]["binding"] = json!({ "kind": "pair_session", "job_run_id": Uuid::new_v4() });
    assert!(reject_checkout(run_id, checkout)
        .await
        .contains("did not allow dispatch"));
}

#[tokio::test]
async fn invalid_fence_attempt_or_prompt_never_starts() {
    let run_id = Uuid::new_v4();
    for fence in [json!(0), json!(-1), Value::Null, json!("4"), json!(1.5)] {
        let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
        checkout["execution_attempt_fence_epoch"] = fence;
        reject_checkout(run_id, checkout).await;
    }
    for attempt in [Value::Null, json!("attempt-bad"), json!(Uuid::nil())] {
        let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
        checkout["execution_attempt_id"] = attempt;
        reject_checkout(run_id, checkout).await;
    }
    for prompt in [json!(""), json!(" \n\t"), Value::Null] {
        let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
        checkout["prompt"] = prompt;
        reject_checkout(run_id, checkout).await;
    }
}

#[tokio::test]
async fn missing_or_invalid_checkout_authorization_never_starts() {
    let run_id = Uuid::new_v4();
    for gate in [Value::Null, json!({}), json!({ "status": "future_allow" })] {
        let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
        checkout["gate"] = gate;
        reject_checkout(run_id, checkout).await;
    }
    let mut checkout = allowed_checkout(run_id, Uuid::new_v4(), 4);
    checkout.as_object_mut().unwrap().remove("ok");
    reject_checkout(run_id, checkout).await;
}

#[tokio::test]
async fn absent_or_false_expected_source_support_prevents_even_checkout() {
    for capabilities in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!({ "session_access": true })),
        Some(json!({ "expected_work_source": false })),
        Some(json!({ "expected_work_source": "true" })),
        Some(json!({ "expected_work_source": null })),
    ] {
        let mut initialize = json!({ "protocol": "bearwire", "version": 1 });
        if let Some(capabilities) = capabilities {
            initialize["capabilities"] = capabilities;
        }
        let den = MockDen::start(MockDenOptions {
            initialize,
            ..Default::default()
        })
        .await;
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            run_headless_turn(&reqwest::Client::new(), &den.config, &env(Uuid::new_v4())),
        )
        .await
        .expect("bounded startup")
        .expect_err("unsupported Den must fail closed");
        let error = format!("{error:#}");
        assert!(error.contains("expected_work_source"), "{error}");
        assert!(error.contains("upgrade Den"), "{error}");
        assert_eq!(den.methods().await, ["initialize"]);
    }
}

#[test]
fn typed_checkout_keeps_prompt_and_bounds_deadline() {
    let run_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let mut result = allowed_checkout(run_id, attempt_id, 7);
    result["prompt"] = json!("  exact Den prompt\n");
    result["deadline_secs"] = json!(900);
    let checkout = checkout::decode(result, run_id, Duration::from_secs(60)).unwrap();
    assert_eq!(checkout.source.work_run_id, run_id);
    assert_eq!(checkout.source.execution_attempt_id, attempt_id);
    assert_eq!(checkout.source.fence_epoch, 7);
    assert_eq!(checkout.prompt, "  exact Den prompt\n");
    assert_eq!(checkout.deadline, Duration::from_secs(60));
}

#[tokio::test]
async fn headless_carries_the_exact_source_to_open_and_start_without_control_text() {
    let run_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let den = MockDen::start(MockDenOptions {
        checkout: allowed_checkout(run_id, attempt_id, 9),
        ..Default::default()
    })
    .await;
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        run_headless_turn(&reqwest::Client::new(), &den.config, &env(run_id)),
    )
    .await
    .expect("bounded startup")
    .expect_err("mock stops after recording run.start");
    assert!(format!("{error:#}").contains("test stop after run.start"));
    let requests = den.requests.lock().await;
    let open = requests
        .iter()
        .find(|r| r["method"] == "session.open")
        .unwrap();
    let start = requests
        .iter()
        .find(|r| r["method"] == "run.start")
        .unwrap();
    let expected = json!({
        "work_run_id": run_id,
        "execution_attempt_id": attempt_id,
        "fence_epoch": 9,
    });
    for request in [open, start] {
        assert_eq!(request["params"]["expected_work_source"], expected);
        assert!(request["params"]["client_context"]
            .get("expected_work_source")
            .is_none());
        assert!(request["params"]["client_context"]
            .get("work_run_id")
            .is_none());
        assert!(request["params"]["client_context"]
            .get("execution_attempt_id")
            .is_none());
        Uuid::parse_str(request["params"]["session_id"].as_str().unwrap())
            .expect("plain UUID session ID, no headless prefix");
    }
    assert_eq!(open["params"]["session_id"], start["params"]["session_id"]);
    assert_eq!(start["params"]["prompt"], "Den-rendered Work prompt");
    assert_eq!(start["params"]["prompt_context"], json!({}));
}
