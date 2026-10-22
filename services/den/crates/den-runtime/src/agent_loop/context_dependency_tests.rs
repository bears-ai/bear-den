use super::*;
use den_memory::personal_facts::PersonalFactId;
use den_service::conversation::{
    dependencies::ContextDependencies,
    message_types::{ConversationMessageRole, ConversationMessageWrite},
    persistence::{self, AppendedConversationMessage},
};
use serde_json::json;

fn history_row(sequence: i64, message_type: &str, metadata: Value) -> TranscriptHistoryRow {
    TranscriptHistoryRow {
        message_id: format!("message-{sequence}"),
        sequence_no: sequence,
        message_type: ConversationMessageType::try_from_storage(message_type).unwrap(),
        visibility: ConversationMessageVisibility::Default,
        content_text: String::new(),
        content_json: metadata,
        tool_call_id: None,
    }
}

fn protected_metadata() -> Value {
    let mut metadata = json!({});
    ContextDependencies::personal_fact(Uuid::new_v4().into(), PersonalFactId(Uuid::new_v4()))
        .attach_to_metadata(&mut metadata)
        .unwrap();
    metadata
}

#[test]
fn protected_history_is_denied_before_cutoff_or_orphan_derivation() {
    for message_type in [
        "user",
        "assistant",
        "tool_call",
        "tool_result",
        "workflow_event",
    ] {
        let history = vec![
            history_row(
                1,
                "tool_call",
                json!({
                    "event": "tool_request",
                    "tool_call_id": "orphan",
                    "tool_name": "fs_read_text_file",
                    "args": {},
                }),
            ),
            // Empty/ignored content and invalid tool payloads must not bypass
            // dependency admission. This row would disappear at the cutoff.
            TranscriptHistoryRow {
                visibility: ConversationMessageVisibility::DiagnosticOnly,
                ..history_row(2, message_type, protected_metadata())
            },
            history_row(100, "user", json!({})),
        ];
        let mut derived = false;
        let result = transcript_grouping_rows_from_history(history).map(|mut rows| {
            derived = true;
            rows.retain(|row| row.sequence_no.unwrap_or(0) > 50);
            orphan_tool_call_ids_from_rows(&transcript_rows_from_grouping_rows(&rows))
        });
        assert!(matches!(result, Err(DenError::Authorization(_))));
        assert!(
            !derived,
            "protected raw history reached cutoff/orphan processing"
        );
    }
}

#[test]
fn malformed_dependency_metadata_fails_the_entire_raw_history() {
    let malformed = [
        Value::Null,
        json!({}),
        json!([]),
        json!({"version": 2, "personal_facts": []}),
        json!({"version": 1}),
        json!({"version": 1, "personal_facts": null}),
        json!({"version": 1, "personal_facts": [{}]}),
        json!({"version": 1, "personal_facts": [{
            "bear_id": "not-a-uuid", "fact_id": Uuid::new_v4(),
        }]}),
    ];
    for envelope in malformed {
        let result = transcript_grouping_rows_from_history(vec![
            history_row(1, "user", json!({})),
            TranscriptHistoryRow {
                visibility: ConversationMessageVisibility::DiagnosticOnly,
                ..history_row(
                    2,
                    "workflow_event",
                    json!({"den_context_dependencies": envelope}),
                )
            },
            history_row(3, "assistant", json!({})),
        ]);
        assert!(matches!(result, Err(DenError::Authorization(_))));
    }
}

#[test]
fn unannotated_history_and_explicit_empty_dependencies_remain_admissible() {
    let mut empty_metadata = json!({"unrelated": "preserved"});
    ContextDependencies::default()
        .attach_to_metadata(&mut empty_metadata)
        .unwrap();
    let history = vec![
        history_row(1, "user", Value::Null),
        history_row(2, "assistant", json!({})),
        history_row(3, "assistant", empty_metadata.clone()),
    ];
    let rows = transcript_grouping_rows_from_history(history).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].sequence_no, Some(1));
    assert_eq!(rows[2].content_json, empty_metadata);
}

#[test]
fn unannotated_tool_chain_reconstructs_and_repairs_without_changes() {
    let mut history = vec![
        history_row(1, "user", json!({})),
        history_row(2, "assistant", json!({})),
        history_row(
            3,
            "tool_call",
            json!({
                "event": "tool_request",
                "tool_call_id": "ordinary-call",
                "tool_name": "fs_read_text_file",
                "args": {"path": "/workspace/file"},
            }),
        ),
        history_row(
            4,
            "tool_result",
            json!({
                "event": "tool_result",
                "tool_call_id": "ordinary-call",
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "ordinary tool output",
            }),
        ),
        history_row(5, "assistant", json!({})),
    ];
    history[0].content_text = "Read the file".into();
    history[1].content_text = "Reading it".into();
    history[4].content_text = "Done".into();
    let grouping = transcript_grouping_rows_from_history(history).unwrap();
    let rows = transcript_rows_from_grouping_rows(&grouping);
    assert!(orphan_tool_call_ids_from_rows(&rows).is_empty());
    let messages = reconstruct_transcript_messages(rows);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].content.as_deref(), Some("Read the file"));
    let call = &messages[1].tool_calls.as_ref().unwrap()[0];
    assert_eq!(call.id, "ordinary-call");
    assert_eq!(call.function.name, "fs_read_text_file");
    assert_eq!(
        serde_json::from_str::<Value>(&call.function.arguments).unwrap(),
        json!({"path": "/workspace/file"})
    );
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("ordinary-call"));
    assert_eq!(messages[2].content.as_deref(), Some("ordinary tool output"));
    assert_eq!(messages[3].content.as_deref(), Some("Done"));
    let before = serde_json::to_value(&messages).unwrap();
    assert_eq!(
        serde_json::to_value(repair_tool_call_message_chain(messages)).unwrap(),
        before
    );
}

async fn append_history_row(
    pool: &PgPool,
    conversation_id: Uuid,
    message_type: ConversationMessageType,
    visibility: ConversationMessageVisibility,
    text: &str,
    metadata: Value,
) -> AppendedConversationMessage {
    let role = match message_type {
        ConversationMessageType::User => Some(ConversationMessageRole::User),
        ConversationMessageType::Assistant => Some(ConversationMessageRole::Assistant),
        _ => None,
    };
    persistence::append_message(
        pool,
        conversation_id,
        &ConversationMessageWrite::structured(message_type, role, visibility, text, metadata),
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn public_loaders_deny_protected_and_malformed_rows_before_visibility_cutoff_and_backfill(
    pool: PgPool,
) {
    use ConversationMessageType::{Assistant, ToolCall, ToolResult, WorkflowEvent};
    use ConversationMessageVisibility::{AdminOnly, Default, DiagnosticOnly, HiddenFromUser};

    let (session, _, _) = crate::agent_loop::source_admission::tests::fixture(&pool).await;
    let metadata_cases = [
        protected_metadata(),
        json!({"den_context_dependencies": null}),
        json!({"den_context_dependencies": {"version": 2, "personal_facts": []}}),
        json!({"den_context_dependencies": {"version": 1, "personal_facts": ["invalid"]}}),
    ];
    for (visibility, message_type) in [
        (DiagnosticOnly, Assistant),
        (DiagnosticOnly, WorkflowEvent),
        (DiagnosticOnly, ToolCall),
        (DiagnosticOnly, ToolResult),
        (HiddenFromUser, Assistant),
        (HiddenFromUser, ToolCall),
        (HiddenFromUser, ToolResult),
        (AdminOnly, Assistant),
        (Default, Assistant),
    ] {
        for metadata in &metadata_cases {
            let external = format!("den-conv-{}", Uuid::new_v4().simple());
            let conversation = persistence::ensure_conversation_for_external_id(
                &pool,
                session.bear_id,
                session.user_id,
                &external,
                None,
                None,
            )
            .await
            .unwrap();
            append_history_row(
                &pool,
                conversation.id,
                ConversationMessageType::User,
                Default,
                "ordinary input",
                json!({}),
            )
            .await;
            let blocked = append_history_row(
                &pool,
                conversation.id,
                message_type,
                visibility,
                "must not be reused",
                metadata.clone(),
            )
            .await;
            let orphan = append_history_row(
                &pool,
                conversation.id,
                ToolCall,
                DiagnosticOnly,
                "ordinary orphan request",
                json!({
                    "event": "tool_request",
                    "tool_call_id": "must-not-backfill",
                    "tool_name": "fs_read_text_file",
                    "args": {},
                }),
            )
            .await;
            assert!(orphan.sequence_no > blocked.sequence_no);

            // The blocked row is below the cutoff; the ordinary orphan is above
            // it. Neither visibility projection nor cutoff may hide the denial.
            assert!(matches!(
                load_transcript_messages_after_seq(
                    &pool,
                    session.bear_id,
                    &external,
                    Some(blocked.sequence_no),
                )
                .await,
                Err(DenError::Authorization(_))
            ));
            assert!(matches!(
                load_transcript_grouping_rows(&pool, session.bear_id, &external).await,
                Err(DenError::Authorization(_))
            ));
            assert!(matches!(
                load_transcript_messages(&pool, session.bear_id, &external).await,
                Err(DenError::Authorization(_))
            ));
            let persisted = persistence::list_messages_page(&pool, conversation.id, None, 100)
                .await
                .unwrap();
            assert_eq!(
                persisted.len(),
                3,
                "denial must not backfill an orphan result"
            );
            assert_eq!(persisted[0].sequence_no, orphan.sequence_no);
            assert_eq!(persisted[1].content_json, *metadata);
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn ordinary_public_loader_preserves_exact_prior_diagnostic_filter_and_tool_replay(
    pool: PgPool,
) {
    let (session, conversation_id, _) =
        crate::agent_loop::source_admission::tests::fixture(&pool).await;
    let mut expected_ids = Vec::new();
    for visibility in ConversationMessageVisibility::ALL {
        let call_id = format!("ordinary-{}", visibility.as_str());
        for message_type in ConversationMessageType::ALL {
            let text = format!("{visibility}/{message_type}");
            let metadata = match message_type {
                ConversationMessageType::ToolCall => json!({
                    "event": "tool_request",
                    "tool_call_id": call_id,
                    "tool_name": "fs_read_text_file",
                    "args": {},
                }),
                ConversationMessageType::ToolResult => json!({
                    "event": "tool_result",
                    "tool_call_id": call_id,
                    "tool_name": "fs_read_text_file",
                    "status": "ok",
                    "content": text,
                }),
                _ => json!({}),
            };
            let appended = append_history_row(
                &pool,
                conversation_id,
                message_type,
                visibility,
                &text,
                metadata,
            )
            .await;
            if visibility != ConversationMessageVisibility::DiagnosticOnly
                || matches!(
                    message_type,
                    ConversationMessageType::ToolCall | ConversationMessageType::ToolResult
                )
            {
                expected_ids.push(Some(appended.id.to_string()));
            }
        }
    }
    let grouping = load_transcript_grouping_rows(&pool, session.bear_id, &session.conversation_id)
        .await
        .unwrap();
    assert_eq!(
        grouping
            .iter()
            .map(|row| row.message_id.clone())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(grouping.len(), 26); // All 24 non-diagnostic rows plus two diagnostic tools.
    let messages = load_transcript_messages(&pool, session.bear_id, &session.conversation_id)
        .await
        .unwrap();
    for visibility in ConversationMessageVisibility::ALL {
        let call_id = format!("ordinary-{}", visibility.as_str());
        assert!(messages.iter().any(|message| message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| calls.iter().any(|call| call.id == call_id))));
        assert!(messages.iter().any(|message| {
            message.role == "tool"
                && message.tool_call_id.as_deref() == Some(call_id.as_str())
                && message.content.as_deref() == Some(format!("{visibility}/tool_result").as_str())
        }));
        for message_type in [
            ConversationMessageType::User,
            ConversationMessageType::Assistant,
        ] {
            let text = format!("{visibility}/{message_type}");
            assert_eq!(
                messages
                    .iter()
                    .any(|message| message.content.as_deref() == Some(text.as_str())),
                visibility != ConversationMessageVisibility::DiagnosticOnly
            );
        }
    }
    assert_eq!(
        serde_json::to_value(repair_tool_call_message_chain(messages.clone())).unwrap(),
        serde_json::to_value(messages).unwrap()
    );
}
