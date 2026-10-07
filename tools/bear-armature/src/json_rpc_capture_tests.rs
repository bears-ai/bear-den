use crate::json_rpc::{capture_json_output_for_test, wait_for_json_response_for_test, write_json};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn response_wait_matches_terminal_id_and_accepts_already_buffered_completion() {
    let (result, output) = capture_json_output_for_test(|| async {
        let id = json!("wanted-response");
        write_json(json!({"id": id, "method": "client/request", "params": {}})).await?;
        let writer = tokio::spawn(async {
            tokio::task::yield_now().await;
            write_json(json!({"method": "session/update", "params": {}})).await?;
            write_json(json!({"id": "unrelated-response", "result": {}})).await?;
            tokio::task::yield_now().await;
            write_json(json!({"id": "wanted-response", "result": {"ok": true}})).await
        });
        let response = wait_for_json_response_for_test(&id, Duration::from_secs(10)).await?;
        writer.await.expect("response writer completes")?;
        assert_eq!(response["result"]["ok"], true);
        assert_eq!(
            wait_for_json_response_for_test(&id, Duration::from_secs(10)).await?,
            response,
            "a reply written before the wait must not require another notification"
        );
        Ok::<(), anyhow::Error>(())
    })
    .await;
    result.unwrap();
    assert_eq!(output.len(), 4);
}

#[tokio::test]
async fn response_wait_is_bounded_and_accepts_matching_error_responses() {
    let (result, output) = capture_json_output_for_test(|| async {
        let id = json!("missing-response");
        write_json(json!({"id": id, "method": "client/request"})).await?;
        let error = wait_for_json_response_for_test(&id, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing-response"));
        assert!(error.to_string().contains("captured 1 frames"));
        write_json(json!({"id": id, "error": {"code": -32003, "message": "denied"}})).await?;
        let response = wait_for_json_response_for_test(&id, Duration::from_secs(10)).await?;
        assert_eq!(response["error"]["message"], "denied");
        Ok::<(), anyhow::Error>(())
    })
    .await;
    result.unwrap();
    assert_eq!(output.len(), 2);
}

#[tokio::test]
async fn capture_panic_releases_global_capture_before_the_next_test() {
    let task = tokio::spawn(capture_json_output_for_test::<_, _, ()>(|| async {
        write_json(json!({"phase": "panicking-capture"}))
            .await
            .unwrap();
        panic!("intentional capture cleanup regression");
    }));
    assert!(task.await.unwrap_err().is_panic());
    let (result, output) = capture_json_output_for_test(|| async {
        write_json(json!({"phase": "after-panic"})).await
    })
    .await;
    result.unwrap();
    assert_eq!(output, vec![json!({"phase": "after-panic"})]);
}

#[tokio::test]
async fn capture_cancellation_releases_global_capture_before_the_next_test() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(capture_json_output_for_test(|| async {
        write_json(json!({"phase": "cancelled-capture"}))
            .await
            .unwrap();
        entered_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    }));
    entered_rx
        .await
        .expect("capture installed before cancellation");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let (result, output) = capture_json_output_for_test(|| async {
        write_json(json!({"phase": "after-cancellation"})).await
    })
    .await;
    result.unwrap();
    assert_eq!(output, vec![json!({"phase": "after-cancellation"})]);
}
