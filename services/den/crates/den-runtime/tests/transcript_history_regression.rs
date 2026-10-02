use den_runtime::agent_loop::load_transcript_messages;
use den_service::conversation::persistence::{append_message, ensure_conversation_for_external_id};
use den_service::conversation_message_types::{
    ConversationMessageRole, ConversationMessageType, ConversationMessageVisibility,
    ConversationMessageWrite,
};
use uuid::Uuid;

async fn create_user_and_bear(pool: &sqlx::PgPool) -> (i32, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("hist{}", &suffix[..16]);
    let email = format!("{username}@example.test");
    let (user_id,): (i32,) = sqlx::query_as(
        r"
        INSERT INTO users (email, username, display_name, passhash)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        ",
    )
    .bind(email)
    .bind(&username)
    .bind("History Regression")
    .bind("test-passhash")
    .fetch_one(pool)
    .await
    .expect("create user");

    let bear_id = Uuid::new_v4();
    let slug = format!("history-{}", &suffix[..16]);
    sqlx::query(
        r"
        INSERT INTO bears (id, slug, name)
        VALUES ($1, $2, $3)
        ",
    )
    .bind(bear_id)
    .bind(slug)
    .bind("History Regression Bear")
    .execute(pool)
    .await
    .expect("create bear");

    (user_id, bear_id)
}

#[sqlx::test(migrations = "../../migrations")]
async fn transcript_loader_preserves_complete_history_for_turn_budgeting(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure conversation");

    for index in 0..240 {
        let role = if index % 2 == 0 {
            ConversationMessageRole::User
        } else {
            ConversationMessageRole::Assistant
        };
        let message_type = if role == ConversationMessageRole::User {
            ConversationMessageType::User
        } else {
            ConversationMessageType::Assistant
        };
        append_message(
            &pool,
            conversation.id,
            &ConversationMessageWrite {
                message_type,
                role: Some(role),
                visibility: ConversationMessageVisibility::Default,
                content_text: format!("message-{index}"),
                content_json: serde_json::json!({ "index": index }),
                provider_message_id: Some(format!("msg-{index}")),
                source_event_id: Some(format!("src-{index}")),
                created_at: None,
            },
        )
        .await
        .expect("append message");
    }

    // Transcript loading preserves history; turn assembly owns context budgeting.
    let transcript = load_transcript_messages(&pool, bear_id, &conversation_id)
        .await
        .expect("load transcript");
    let texts = transcript
        .iter()
        .filter_map(|message| message.content.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(texts.len(), 240);
    assert_eq!(texts.first(), Some(&"message-0"));
    assert_eq!(texts.last(), Some(&"message-239"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn transcript_loader_prefers_tool_result_output_preview(pool: sqlx::PgPool) {
    let (user_id, bear_id) = create_user_and_bear(&pool).await;
    let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
    let session_id = format!("session-{}", Uuid::new_v4().simple());
    let conversation = ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user_id),
        &conversation_id,
        Some(&session_id),
        None,
    )
    .await
    .expect("ensure conversation");

    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::ToolCall,
            role: Some(ConversationMessageRole::System),
            visibility: ConversationMessageVisibility::DiagnosticOnly,
            content_text: "Tool request: fs_read_text_file".to_string(),
            content_json: serde_json::json!({
                "event": "tool_request",
                "tool_call_id": "call-preview",
                "tool_name": "fs_read_text_file",
                "args": { "path": "/workspace/docs/roadmap/PLAN.md" }
            }),
            provider_message_id: None,
            source_event_id: Some("tool-call-preview".to_string()),
            created_at: None,
        },
    )
    .await
    .expect("append tool call");

    append_message(
        &pool,
        conversation.id,
        &ConversationMessageWrite {
            message_type: ConversationMessageType::ToolResult,
            role: Some(ConversationMessageRole::System),
            visibility: ConversationMessageVisibility::DiagnosticOnly,
            content_text: "Tool result: fs_read_text_file".to_string(),
            content_json: serde_json::json!({
                "event": "tool_result",
                "tool_call_id": "call-preview",
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "raw content should not win",
                "output_preview": "preferred preview",
                "output_summary": "Used fs_read_text_file (ok): preferred preview"
            }),
            provider_message_id: None,
            source_event_id: Some("tool-result-preview".to_string()),
            created_at: None,
        },
    )
    .await
    .expect("append tool result");

    let transcript = load_transcript_messages(&pool, bear_id, &conversation_id)
        .await
        .expect("load transcript");
    let tool_message = transcript
        .iter()
        .find(|message| message.role == "tool")
        .expect("tool message");
    assert_eq!(tool_message.tool_call_id.as_deref(), Some("call-preview"));
    assert_eq!(tool_message.content.as_deref(), Some("preferred preview"));
}
