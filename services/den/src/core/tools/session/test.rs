#[cfg(test)]
mod info_tests {
    include!("../tests/session_info.rs");
}

#[cfg(test)]
mod role_semantics_tests {
    include!("../tests/session_role_semantics.rs");
}

#[tokio::test]
async fn focus_tool_aliases_fail_before_database_or_lifecycle_access() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused")
        .expect("lazy test pool");
    pool.close().await;
    let config = crate::config::Config::test_stub();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let context = super::DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "focus-containment-test".to_string(),
        binding_id: "pair".to_string(),
        profile: Some(den_service::bears::BearProfile::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conversation-focus-containment".to_string(),
        session_id: "session-focus-containment".to_string(),
        work_run_id: None,
        client_session_id: Some("session-focus-containment".to_string()),
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
        request_id: Some("request-focus-containment".to_string()),
        channel: Default::default(),
    };

    for tool_name in [
        den_core::tools::constants::DEN_TASK_FOCUS,
        den_core::tools::constants::DEN_TASK_FOCUS_PROVIDER,
    ] {
        let error = super::invoke_den_tool(
            &pool,
            &config,
            &stores,
            tool_name,
            serde_json::json!({}),
            context.clone(),
        )
        .await
        .expect_err("focus must be contained before lifecycle access");
        let payload = match error {
            crate::errors::CustomError::Session(payload) => payload,
            other => panic!("focus containment must return a typed session error: {other}"),
        };
        let diagnostic: serde_json::Value =
            serde_json::from_str(&payload).expect("structured containment diagnostic");

        assert_eq!(
            diagnostic["code"],
            "focus_current_task_temporarily_unavailable"
        );
        assert_eq!(diagnostic["operation"], "den.task.focus");
        assert_eq!(diagnostic["retryable"], true);
        assert_eq!(diagnostic["mutation_applied"], false);
        assert_eq!(diagnostic["reason"], "canonical_runtime_state_unavailable");
    }
}
