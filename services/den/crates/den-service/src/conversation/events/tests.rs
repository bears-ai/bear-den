use super::*;

#[test]
fn genuine_model_tool_records_are_replayable_without_promoting_diagnostics_or_human_history() {
    let source = ConversationEventProvenance::client_session("model-tool-test");
    let request = || {
        CanonicalToolRequestRecord::new(
            "repository_head",
            "call",
            "request",
            None,
            serde_json::json!({"work_surface_id":uuid::Uuid::nil()}),
            false,
            None,
            "native_runtime",
        )
    };
    let diagnostic = CanonicalConversationRecord::tool_request(request(), &source).to_write(None);
    let model = CanonicalConversationRecord::model_tool_request(request(), &source).to_write(None);
    assert_eq!(
        diagnostic.visibility,
        ConversationMessageVisibility::DiagnosticOnly
    );
    assert_eq!(
        model.visibility,
        ConversationMessageVisibility::HiddenFromUser
    );
    assert!(model.visibility.is_model_transcript_visible());
    assert!(!model.visibility.is_user_history_visible());
    let result = CanonicalConversationRecord::model_tool_result(
        CanonicalToolResultRecord::new(
            Some("repository_head".into()),
            "call",
            None,
            den_core::tools::result_compaction::ToolResultStatus::Ok,
            Some("safe result".into()),
            serde_json::Value::Null,
            serde_json::json!({}),
            Some("request".into()),
        ),
        &source,
    )
    .to_write(None);
    assert_eq!(
        result.visibility,
        ConversationMessageVisibility::HiddenFromUser
    );
}

#[test]
fn canonical_persistence_enabled_for_den_conv_ids() {
    assert!(canonical_persistence_enabled_for_conversation("default"));
    assert!(canonical_persistence_enabled_for_conversation(
        "conv-abc123"
    ));
    assert!(canonical_persistence_enabled_for_conversation(
        "den-conv-abc123"
    ));
    assert!(!canonical_persistence_enabled_for_conversation(
        "provider-only-id"
    ));
}
