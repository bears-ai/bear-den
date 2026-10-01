use super::*;
use crate::agent_loop::{create_native_approval, decide_native_approval, NativeApprovalDecision};
use crate::llm::ChatToolCallFunction;
use den_service::bears::db::{create_bear, BearParams};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn one_shot_web_approval_binds_only_the_original_call(pool: PgPool) {
    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "oneshotwebbinding",
            name: "One shot web binding",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let original_url = "https://example.com/source";
    let approval_id = create_native_approval(
        &pool,
        bear_id,
        "den-conv-one-shot",
        "client-session-one",
        "call-one",
        "web_fetch",
        &json!({"url": original_url}),
    )
    .await
    .unwrap();
    decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
        .await
        .unwrap();
    let call = |id: &str, url: &str| ChatToolCall {
        id: id.to_string(),
        call_type: "function".into(),
        function: ChatToolCallFunction {
            name: "web_fetch".into(),
            arguments: json!({"url": url}).to_string(),
        },
    };
    let request_id = Uuid::new_v4();
    for forged in [
        call("call-other", original_url),
        call("call-one", "https://example.com/other"),
    ] {
        assert!(bind_web_fetch_approval_to_continuation(
            &pool,
            bear_id,
            "den-conv-one-shot",
            "client-session-one",
            &approval_id,
            request_id,
            &forged,
        )
        .await
        .is_err());
    }
    assert!(bind_web_fetch_approval_to_continuation(
        &pool,
        Uuid::new_v4(),
        "den-conv-one-shot",
        "client-session-one",
        &approval_id,
        request_id,
        &call("call-one", original_url),
    )
    .await
    .is_err());
    bind_web_fetch_approval_to_continuation(
        &pool,
        bear_id,
        "den-conv-one-shot",
        "client-session-one",
        &approval_id,
        request_id,
        &call("call-one", original_url),
    )
    .await
    .unwrap();
    assert!(bind_web_fetch_approval_to_continuation(
        &pool,
        bear_id,
        "den-conv-one-shot",
        "client-session-one",
        &approval_id,
        Uuid::new_v4(),
        &call("call-one", original_url),
    )
    .await
    .is_err());
}
