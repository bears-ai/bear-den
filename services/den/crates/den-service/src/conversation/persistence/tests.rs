use super::*;
use time::OffsetDateTime;

/// Serializes tests that read tool payloads against the one test that toggles
/// the process-wide `BEARS_STRICT_TYPED_PAYLOADS` env var.
static STRICT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn row(message_type: &str, role: Option<&str>, visibility: &str) -> PersistedConversationMessage {
    PersistedConversationMessage {
        sequence_no: 7,
        message_type: message_type.to_string(),
        role: role.map(str::to_string),
        visibility: visibility.to_string(),
        content_text: "hello transcript".to_string(),
        content_json: serde_json::json!({}),
        provider_message_id: Some("provider-7".to_string()),
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn tool_row(
    message_type: ConversationMessageType,
    visibility: ConversationMessageVisibility,
) -> PersistedConversationMessage {
    let content_json = match message_type {
        ConversationMessageType::ToolCall => serde_json::json!({
            "event": "tool_request",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "args": { "path": "pair/notes/private.md" }
        }),
        ConversationMessageType::ToolResult => serde_json::json!({
            "event": "tool_result",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "status": "ok",
            "content": "private result",
            "structured_content": { "content": "private result" },
            "output_summary": "Used memory_read (ok)"
        }),
        _ => unreachable!("only tool rows are used here"),
    };
    PersistedConversationMessage {
        sequence_no: 7,
        message_type: message_type.as_str().to_string(),
        role: Some(ConversationMessageRole::System.as_str().to_string()),
        visibility: visibility.as_str().to_string(),
        content_text: "tool event".to_string(),
        content_json,
        provider_message_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn transcript_projection_accepts_canonical_and_legacy_message_shapes() {
    for message in [
        row("user", None, "default"),
        row("assistant", None, "default"),
        row("message", Some("user"), "default"),
        row("message", Some("assistant"), "default"),
    ] {
        assert!(message.to_model_transcript_message().is_some());
        assert!(message.to_user_history_transcript_message().is_some());
    }
}

#[test]
fn transcript_projection_separates_model_replay_from_user_history_visibility() {
    let hidden = row("user", Some("user"), "hidden_from_user");
    assert!(hidden.to_model_transcript_message().is_some());
    assert!(hidden.to_user_history_transcript_message().is_none());

    let diagnostic = row("assistant", Some("assistant"), "diagnostic_only");
    assert!(diagnostic.to_model_transcript_message().is_none());
    assert!(diagnostic.to_user_history_transcript_message().is_none());
}

#[test]
fn transcript_projection_rejects_non_transcript_roles() {
    let workflow = row("workflow_event", Some("system"), "default");
    assert!(workflow.to_model_transcript_message().is_none());
    assert!(workflow.to_user_history_transcript_message().is_none());
}

#[test]
fn transcript_projection_includes_tool_records_for_model_replay_only() {
    let _guard = STRICT_ENV_LOCK.lock().unwrap();
    let tool_call = PersistedConversationMessage {
        sequence_no: 8,
        message_type: "tool_call".to_string(),
        role: Some("system".to_string()),
        visibility: ConversationMessageVisibility::HiddenFromUser
            .as_str()
            .to_string(),
        content_text: "Tool request: memory_read".to_string(),
        content_json: serde_json::json!({
            "event": "tool_request",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "args": { "path": "pair/notes/demo.md" }
        }),
        provider_message_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert!(matches!(
        tool_call.to_model_transcript_record(),
        Some(PersistedTranscriptRecord::ToolCall { tool_call_id, tool_name, .. })
        if tool_call_id == "call-1" && tool_name == "memory_read"
    ));
    assert!(tool_call.to_user_history_transcript_message().is_none());
    assert!(tool_call.to_user_history_record().is_none());
    assert!(tool_call.to_model_history_record().is_some());

    let tool_result = PersistedConversationMessage {
        sequence_no: 9,
        message_type: "tool_result".to_string(),
        role: Some("system".to_string()),
        visibility: ConversationMessageVisibility::HiddenFromUser
            .as_str()
            .to_string(),
        content_text: "Tool result: memory_read".to_string(),
        content_json: serde_json::json!({
            "event": "tool_result",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "status": "ok",
            "content": "file contents",
            "structured_content": { "content": "file contents" }
        }),
        provider_message_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert!(matches!(
        tool_result.to_model_transcript_record(),
        Some(PersistedTranscriptRecord::ToolResult { tool_call_id, status, .. })
        if tool_call_id.as_deref() == Some("call-1") && status.as_deref() == Some("ok")
    ));
    assert!(tool_result.to_user_history_transcript_message().is_none());
    assert!(tool_result.to_user_history_record().is_none());
    assert!(tool_result.to_model_history_record().is_some());
}

#[test]
fn typed_tool_payloads_decode_from_persisted_content_json() {
    let _guard = STRICT_ENV_LOCK.lock().unwrap();
    let tool_call_json = serde_json::json!({
        "event": "tool_request",
        "tool_call_id": "call-1",
        "tool_name": "memory_read",
        "args": { "path": "pair/notes/demo.md" },
        "approval_required": false
    });
    let tool_call =
        PersistedToolRequestPayload::try_from(&tool_call_json).expect("tool call payload");
    assert_eq!(tool_call.tool_call_id, "call-1");
    assert_eq!(tool_call.tool_name, "memory_read");

    let tool_result_json = serde_json::json!({
        "event": "tool_result",
        "tool_call_id": "call-1",
        "tool_name": "memory_read",
        "status": "ok",
        "content": "file contents",
        "structured_content": { "content": "file contents" },
        "output_summary": "Used memory_read (ok): file contents",
        "output_preview": "file contents"
    });
    let tool_result =
        PersistedToolResultPayload::try_from(&tool_result_json).expect("tool result payload");
    assert_eq!(
        tool_result.status,
        den_core::tools::result_compaction::ToolResultStatus::Ok
    );
    assert_eq!(tool_result.output_preview.as_deref(), Some("file contents"));
}

#[test]
fn strict_typed_payloads_require_output_summary_for_complete_tool_results() {
    let _guard = STRICT_ENV_LOCK.lock().unwrap();
    std::env::set_var("BEARS_STRICT_TYPED_PAYLOADS", "1");
    let tool_result_json = serde_json::json!({
        "event": "tool_result",
        "tool_call_id": "call-1",
        "tool_name": "memory_read",
        "status": "ok",
        "content": "file contents",
        "structured_content": { "content": "file contents" }
    });
    let err = PersistedToolResultPayload::try_from(&tool_result_json)
        .expect_err("strict mode should reject missing output_summary");
    std::env::remove_var("BEARS_STRICT_TYPED_PAYLOADS");

    assert!(err.to_string().contains("output_summary"));
}

#[test]
fn user_history_projection_includes_tool_records() {
    let _guard = STRICT_ENV_LOCK.lock().unwrap();
    let user = row("user", Some("user"), "default");
    assert!(matches!(
        user.to_user_history_record(),
        Some(PersistedUserHistoryMessage { role, content, .. })
        if role == "user" && content == "hello transcript"
    ));

    let tool_call = PersistedConversationMessage {
        sequence_no: 8,
        message_type: "tool_call".to_string(),
        role: Some("system".to_string()),
        visibility: ConversationMessageVisibility::Default.as_str().to_string(),
        content_text: "Tool request: memory_read".to_string(),
        content_json: serde_json::json!({
            "event": "tool_request",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "args": { "path": "pair/notes/demo.md" }
        }),
        provider_message_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert!(tool_call.to_model_transcript_record().is_some());
    // User-visible tool requests replay as pending tool cards.
    assert!(matches!(
        tool_call.to_user_history_record(),
        Some(PersistedUserHistoryMessage { kind, tool_call_id, tool_name, status, .. })
        if kind == "tool_call"
            && tool_call_id.as_deref() == Some("call-1")
            && tool_name.as_deref() == Some("memory_read")
            && status.as_deref() == Some("pending")
    ));

    let tool_result = PersistedConversationMessage {
        sequence_no: 9,
        message_type: "tool_result".to_string(),
        role: Some("system".to_string()),
        visibility: ConversationMessageVisibility::Default.as_str().to_string(),
        content_text: "Tool result: memory_read".to_string(),
        content_json: serde_json::json!({
            "event": "tool_result",
            "tool_call_id": "call-1",
            "tool_name": "memory_read",
            "status": "ok",
            "content": "file contents"
        }),
        provider_message_id: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert!(matches!(
        tool_result.to_user_history_record(),
        Some(PersistedUserHistoryMessage { role, content, .. })
        if role == "assistant" && content == "Used memory_read (ok): file contents"
    ));
}

#[test]
fn tool_projection_visibility_matrix() {
    let _guard = STRICT_ENV_LOCK.lock().unwrap();
    for message_type in [
        ConversationMessageType::ToolCall,
        ConversationMessageType::ToolResult,
    ] {
        for visibility in ConversationMessageVisibility::ALL {
            let row = tool_row(message_type, visibility);
            let model_visible = visibility.is_model_transcript_visible();
            let user_visible = visibility.is_user_history_visible();

            assert_eq!(
                row.is_model_transcript_visible(),
                model_visible,
                "{message_type:?} {visibility:?}"
            );
            assert_eq!(
                row.is_user_history_visible(),
                user_visible,
                "{message_type:?} {visibility:?}"
            );
            assert_eq!(
                row.to_model_transcript_record().is_some(),
                model_visible,
                "{message_type:?} {visibility:?}"
            );
            assert_eq!(
                row.to_model_history_record().is_some(),
                model_visible,
                "{message_type:?} {visibility:?}"
            );
            assert_eq!(
                row.to_user_history_record().is_some(),
                user_visible,
                "{message_type:?} {visibility:?}"
            );

            if model_visible {
                let history = row.to_model_history_record().unwrap();
                match message_type {
                    ConversationMessageType::ToolCall => {
                        assert_eq!(history.arguments, row.content_json["args"]);
                    }
                    ConversationMessageType::ToolResult => {
                        assert_eq!(history.raw_output, row.content_json["structured_content"]);
                    }
                    _ => unreachable!(),
                }
                match (message_type, row.to_model_transcript_record().unwrap()) {
                    (
                        ConversationMessageType::ToolCall,
                        PersistedTranscriptRecord::ToolCall { arguments, .. },
                    ) => {
                        assert_eq!(arguments, row.content_json["args"]);
                    }
                    (
                        ConversationMessageType::ToolResult,
                        PersistedTranscriptRecord::ToolResult {
                            structured_content, ..
                        },
                    ) => {
                        assert_eq!(structured_content, row.content_json["structured_content"]);
                    }
                    _ => panic!("wrong model tool projection for {message_type:?}"),
                }
            }
            if user_visible {
                let history = row.to_user_history_record().unwrap();
                match message_type {
                    ConversationMessageType::ToolCall => {
                        assert_eq!(history.arguments, row.content_json["args"]);
                    }
                    ConversationMessageType::ToolResult => {
                        assert_eq!(history.raw_output, row.content_json["structured_content"]);
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_history_page_skips_diagnostic_tools_across_raw_pages(pool: PgPool) {
    use crate::bears::db::{create_bear, BearParams};

    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: "toolprojectionpage",
            name: "Tool projection",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create bear");
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        None,
        "tool-projection-page",
        None,
        None,
    )
    .await
    .expect("create conversation");

    async fn persist_tool(
        pool: &PgPool,
        conversation_id: Uuid,
        sequence_no: i64,
        message_type: ConversationMessageType,
        visibility: ConversationMessageVisibility,
    ) {
        let row = tool_row(message_type, visibility);
        let write = ConversationMessageWrite::structured(
            message_type,
            Some(ConversationMessageRole::System),
            visibility,
            row.content_text,
            row.content_json,
        );
        insert_message_if_absent(pool, conversation_id, sequence_no, &write)
            .await
            .expect("insert tool");
    }

    persist_tool(
        &pool,
        conversation.id,
        7,
        ConversationMessageType::ToolResult,
        ConversationMessageVisibility::Default,
    )
    .await;
    persist_tool(
        &pool,
        conversation.id,
        8,
        ConversationMessageType::ToolResult,
        ConversationMessageVisibility::DiagnosticOnly,
    )
    .await;
    persist_tool(
        &pool,
        conversation.id,
        9,
        ConversationMessageType::ToolCall,
        ConversationMessageVisibility::HiddenFromUser,
    )
    .await;
    persist_tool(
        &pool,
        conversation.id,
        10,
        ConversationMessageType::ToolCall,
        ConversationMessageVisibility::Default,
    )
    .await;
    for sequence_no in 11..=114 {
        let message_type = if sequence_no % 2 == 0 {
            ConversationMessageType::ToolCall
        } else {
            ConversationMessageType::ToolResult
        };
        persist_tool(
            &pool,
            conversation.id,
            sequence_no,
            message_type,
            ConversationMessageVisibility::DiagnosticOnly,
        )
        .await;
    }

    let raw = list_messages_page(&pool, conversation.id, None, 100)
        .await
        .unwrap();
    assert_eq!(raw.len(), 100);
    assert!(raw
        .iter()
        .all(|row| row.storage_visibility().unwrap()
            == ConversationMessageVisibility::DiagnosticOnly));

    let first = list_projected_messages_page(
        &pool,
        conversation.id,
        None,
        1,
        ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert_eq!(
        first.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
        vec![10]
    );
    let next = list_projected_messages_page(
        &pool,
        conversation.id,
        Some(first[0].sequence_no),
        2,
        ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert_eq!(
        next.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
        vec![7]
    );
    assert!(next
        .iter()
        .all(|row| row.to_user_history_record().is_some()));

    let model = list_projected_messages_page(
        &pool,
        conversation.id,
        None,
        3,
        ConversationHistoryProjection::ModelTranscript,
    )
    .await
    .unwrap();
    assert_eq!(
        model.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
        vec![10, 9, 7]
    );
}
