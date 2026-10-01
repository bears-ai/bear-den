use super::*;
use den_core::tools::{arguments::DenToolChannelContext, context::DenToolInvocationContext};
use den_runtime::agent_loop::{
    create_native_approval, decide_native_approval, NativeApprovalDecision,
};
use den_service::bears::{db, BearProfile};
use serde_json::{json, Value};

struct MockTransport<'a>(DenWebFetcher<'a>);

impl WebFetcher for MockTransport<'_> {
    async fn decide_fetch_approval(
        &self,
        context: &DenToolInvocationContext,
        raw_url: &str,
    ) -> Result<(WebUrl, WebApproval), DenError> {
        self.0.decide_fetch_approval(context, raw_url).await
    }

    async fn record_fetch_attempt(&self, _audit: WebFetchAudit<'_>) -> Result<(), DenError> {
        Ok(())
    }

    async fn http_get(&self, url: &str) -> Result<WebHttpResponse, DenError> {
        Ok(WebHttpResponse {
            final_url: url.to_string(),
            final_host: "example.com".into(),
            status: 200,
            content_type: "text/plain".into(),
            body: b"Synthetic response".to_vec(),
            total_bytes: 18,
            body_truncated: false,
        })
    }

    async fn preferred_hosts(&self, _bear_id: Uuid) -> Result<Vec<String>, DenError> {
        Ok(Vec::new())
    }

    fn normalize_host(&self, _url: &str) -> Option<String> {
        None
    }

    fn default_search_max_results(&self) -> usize {
        5
    }

    async fn provider_search(&self, _query: &str, _limit: usize) -> Result<Value, DenError> {
        Err(DenError::Authorization(
            "search is not part of this test".into(),
        ))
    }
}

fn context(bear_id: Uuid, session_id: &str, request_id: Uuid) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id,
        bear_slug: "one-shot-web".into(),
        binding_id: "den-native:one-shot-web:pair".into(),
        profile: Some(BearProfile::Pair),
        user_id: 7,
        username: None,
        membership_role: None,
        conversation_id: "den-conv-one-shot".into(),
        session_id: session_id.into(),
        work_run_id: None,
        client_session_id: Some(session_id.into()),
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(request_id.to_string()),
        channel: DenToolChannelContext::default(),
    }
}

#[sqlx::test]
async fn exact_web_fetch_approval_is_consumed_once_without_a_bear_wide_grant(pool: PgPool) {
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "oneshotwebpolicy",
            name: "One shot web policy",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let url = "https://example.com/first";
    let approval_id = create_native_approval(
        &pool,
        bear_id,
        "den-conv-one-shot",
        "client-session-one",
        "call-one",
        "web_fetch",
        &json!({"url": url}),
    )
    .await
    .unwrap();
    decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
        .await
        .unwrap();
    let request_id = Uuid::new_v4();
    sqlx::query!(
        "UPDATE runtime_approvals SET execution_request_id = $1 WHERE approval_id = $2",
        request_id,
        approval_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    let config = Config::test_stub();
    let fetcher = DenWebFetcher {
        pool: &pool,
        config: &config,
    };
    let correct = context(bear_id, "client-session-one", request_id);
    for (candidate, target) in [
        (context(bear_id, "client-session-two", request_id), url),
        (
            context(Uuid::new_v4(), "client-session-one", request_id),
            url,
        ),
        (correct.clone(), "https://example.com/other"),
        (context(bear_id, "client-session-one", Uuid::new_v4()), url),
    ] {
        assert_eq!(
            fetcher
                .decide_fetch_approval(&candidate, target)
                .await
                .unwrap()
                .1,
            WebApproval::RequiresApproval
        );
    }
    let response = den_core::tools::web::web_fetch(
        &MockTransport(DenWebFetcher {
            pool: &pool,
            config: &config,
        }),
        &correct,
        json!({"url": url}),
    )
    .await
    .unwrap();
    assert_eq!(response["approval"], "approved_once");
    assert_eq!(response["text_excerpt"], "Synthetic response");
    assert!(den_core::tools::web::web_fetch(
        &MockTransport(DenWebFetcher {
            pool: &pool,
            config: &config
        }),
        &correct,
        json!({"url": url}),
    )
    .await
    .is_err());
    assert_eq!(
        fetcher
            .decide_fetch_approval(&correct, url)
            .await
            .unwrap()
            .1,
        WebApproval::RequiresApproval
    );
    assert_eq!(
        web_policy::decide_web_fetch_approval(&pool, bear_id, url)
            .await
            .unwrap()
            .1,
        web_policy::WebApprovalDecision::RequiresApproval,
    );
    assert!(
        decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
            .await
            .is_err(),
        "a consumed approval cannot be rearmed by a replay"
    );
}
