use serde_json::json;

use crate::core::den_tools::{
    builtin_den_tool_descriptor_for_provider_name, invoke_den_tool, validate_memory_write_entry_semantics, DenToolInvocationContext,
};

#[test]
fn descriptor_exposes_turn_state_domain_metadata() {
    let descriptor = builtin_den_tool_descriptor_for_provider_name("exit_plan_mode").unwrap();
    assert_eq!(descriptor.domain, "workplan");
    assert_eq!(descriptor.content_class, Some("workplan_artifact"));

    let descriptor = builtin_den_tool_descriptor_for_provider_name("update_plan").unwrap();
    assert_eq!(descriptor.domain, "activity");
    assert_eq!(descriptor.content_class, Some("activity_status"));

    let descriptor = builtin_den_tool_descriptor_for_provider_name("memory_write_entry").unwrap();
    assert_eq!(descriptor.domain, "memory");
    assert_eq!(descriptor.content_class, Some("semantic_memory"));
}

#[test]
fn memory_write_entry_semantics_reject_non_memory_domain_before_db_access() {
    let args: crate::core::den_tools::MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "note",
        "title": "workflow-ish",
        "body": "do thing",
        "domain": "workplan"
    }))
    .unwrap();

    let err = validate_memory_write_entry_semantics(&args)
        .unwrap_err()
        .to_string();
    assert!(err.contains("workflow plan") || err.contains("plan-mode"));
}

#[tokio::test]
async fn memory_write_entry_rejects_non_memory_domain_without_db_access() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        role_agent_id: "agent".to_string(),
        agent_role: Some(crate::core::bears::BearAgentRole::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        acp_session_id: Some("acp-test".to_string()),
        conversation_selection: None,
        runtime_target: None,
        request_id: None,
        channel: Default::default(),
    };

    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let result = invoke_den_tool(
        &pool,
        &config,
        "den.memory.write_entry",
        json!({
            "kind": "note",
            "title": "workflow-ish",
            "body": "do thing",
            "domain": "workplan"
        }),
        context,
    )
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("workflow plan") || err.contains("plan-mode"));
}

#[test]
fn memory_write_entry_semantics_reject_activity_content_class_before_db_access() {
    let args: crate::core::den_tools::MemoryWriteEntryArguments = serde_json::from_value(json!({
        "kind": "summary",
        "title": "activity-ish",
        "body": "status changed",
        "content_class": "activity_status"
    }))
    .unwrap();

    let err = validate_memory_write_entry_semantics(&args)
        .unwrap_err()
        .to_string();
    assert!(err.contains("activity") || err.contains("update_plan"));
}

#[tokio::test]
async fn memory_write_entry_rejects_activity_content_class_without_db_access() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        role_agent_id: "agent".to_string(),
        agent_role: Some(crate::core::bears::BearAgentRole::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        acp_session_id: Some("acp-test".to_string()),
        conversation_selection: None,
        runtime_target: None,
        request_id: None,
        channel: Default::default(),
    };

    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = crate::config::Config::test_stub();
    let result = invoke_den_tool(
        &pool,
        &config,
        "den.memory.write_entry",
        json!({
            "kind": "summary",
            "title": "activity-ish",
            "body": "status changed",
            "content_class": "activity_status"
        }),
        context,
    )
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("activity") || err.contains("update_plan"));
}
