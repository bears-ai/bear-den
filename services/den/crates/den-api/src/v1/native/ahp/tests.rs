use super::{
    projection,
    wire::{self, Action, Channel, InputAction, Origin, Part},
};
use serde_json::{json, Value};
use uuid::Uuid;

#[test]
fn resource_uris_are_canonical_and_cannot_smuggle_paths_or_authority() {
    let id = Uuid::new_v4();
    for channel in [Channel::Root, Channel::Chat(id), Channel::Session(id)] {
        assert_eq!(Channel::parse(&channel.uri()).unwrap(), channel);
    }
    for invalid in [
        "file:///etc/passwd",
        "ahp-chat:/../etc/passwd",
        "ahp-chat://host/path",
        "ahp-chat:/00000000-0000-0000-0000-000000000000",
        "ahp-chat:/default",
        "ahp-chat:/new-123",
    ] {
        assert!(Channel::parse(invalid).is_err(), "{invalid}");
    }
    assert!(Channel::parse(&format!("ahp-chat:/{id}/extra")).is_err());
}

fn start(text: Value) -> Value {
    json!({"type":"chat/turnStarted","turnId":Uuid::new_v4(),"startedAt":"2026-10-28T00:00:00Z","message":{"text":text,"origin":{"kind":"user"}}})
}

#[test]
fn released_turn_action_and_limits_are_validated() {
    let action: InputAction = serde_json::from_value(start(json!("hello"))).unwrap();
    action.validate().unwrap();
    for text in [
        String::new(),
        "   ".to_string(),
        "x".repeat(wire::MAX_INPUT + 1),
        "hello\0world".into(),
    ] {
        assert!(serde_json::from_value::<InputAction>(start(json!(text)))
            .unwrap()
            .validate()
            .is_err());
    }
    let mut invalid = start(json!("hello"));
    invalid["startedAt"] = json!("not a timestamp");
    assert!(serde_json::from_value::<InputAction>(invalid)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn unsupported_actions_and_authority_bearing_fields_are_rejected() {
    for action in [
        "chat/toolCallConfirmed",
        "chat/toolCallComplete",
        "chat/inputCompleted",
        "chat/turnResume",
        "chat/workingDirectorySet",
        "session/defaultChatChanged",
        "unknown/action",
    ] {
        assert!(serde_json::from_value::<InputAction>(json!({"type":action})).is_err());
    }
    for field in ["attachments", "model", "agent", "_meta"] {
        let mut action = start(json!("hello"));
        action["message"][field] = json!([]);
        assert!(
            serde_json::from_value::<InputAction>(action).is_err(),
            "{field}"
        );
    }
    let mut forged = start(json!("hello"));
    forged["message"]["origin"]["kind"] = json!("agent");
    assert!(serde_json::from_value::<InputAction>(forged).is_err());
}

#[test]
fn projection_uses_ahp_actions_not_custom_sse_shapes() {
    let id = Uuid::new_v4();
    let action = Action::Delta {
        turn_id: id,
        part_id: "response".into(),
        content: "hello".into(),
    };
    let origin = Origin {
        client_id: "native".into(),
        client_seq: 2,
    };
    let envelope = wire::notification(Channel::Chat(id), json!(action), 7, Some(&origin), None);
    assert_eq!(
        envelope,
        json!({"jsonrpc":"2.0","method":"action","params":{"channel":format!("ahp-chat:/{id}"),"serverSeq":7,"origin":{"clientId":"native","clientSeq":2},"action":{"type":"chat/delta","turnId":id,"partId":"response","content":"hello"}}})
    );
    let unknown = json!({"type":"chat/toolCallConfirmed","toolCallId":"x"});
    let rejection = wire::notification(
        Channel::Chat(id),
        unknown.clone(),
        8,
        Some(&origin),
        Some("unsupported"),
    );
    assert_eq!(rejection["params"]["action"], unknown);
    assert_eq!(rejection["params"]["rejectionReason"], "unsupported");
    assert_eq!(
        json!(Part::Markdown {
            id: "p".into(),
            content: String::new()
        }),
        json!({"kind":"markdown","id":"p","content":""})
    );
}

#[test]
fn historical_turns_preserve_text_roles_and_explicit_outcomes() {
    let id = Uuid::new_v4();
    let turn = Uuid::new_v4();
    let mut user = super::super::tests::message(1, "user", "question");
    user.provider_message_id = Some(turn.to_string());
    let mut assistant = super::super::tests::message(2, "assistant", "partial answer");
    assistant.content_json = json!({"ahp_state":"cancelled"});
    let turns = projection::turns(id, &[user, assistant]);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["id"], turn.to_string());
    assert_eq!(turns[0]["message"]["origin"]["kind"], "user");
    assert_eq!(turns[0]["state"], "cancelled");
    assert_eq!(turns[0]["responseParts"][0]["content"], "partial answer");
}

#[test]
fn browser_and_armature_history_keeps_answers_with_request_correlation() {
    let conversation = Uuid::new_v4();
    let request = Uuid::new_v4();
    let mut user = super::super::tests::message(1, "user", "historical question");
    user.content_json = json!({"request_id":request});
    let mut assistant = super::super::tests::message(2, "assistant", "historical answer");
    assistant.content_json = json!({"request_id":request});
    let turns = projection::turns(conversation, &[user, assistant]);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["responseParts"][0]["content"], "historical answer");
    assert_eq!(turns[0]["state"], "complete");
}

#[test]
fn unsupported_features_are_explicit_and_do_not_create_armature_authority() {
    let metadata = wire::feature_metadata();
    let features = &metadata["ai.bears.native"];
    for feature in [
        "permissions",
        "subagents",
        "localTools",
        "sandbox",
        "attachments",
        "sendReplay",
        "liveMultiClient",
    ] {
        assert_eq!(features[feature], false);
    }
    assert_eq!(features["reconnect"], "snapshot");
}

#[test]
fn typed_terminal_records_restore_empty_cancelled_failed_and_complete_turns() {
    use den_runtime::channel_lifecycle::{ChannelTurnOutcome, ChannelTurnTerminal};
    for (outcome, expected) in [
        (ChannelTurnOutcome::Cancelled, "cancelled"),
        (ChannelTurnOutcome::Completed, "complete"),
        (ChannelTurnOutcome::Failed, "error"),
    ] {
        let source = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut user = super::super::tests::message(1, "user", "question");
        user.provider_message_id = Some(id.to_string());
        let terminal = ChannelTurnTerminal {
            turn_id: id,
            user_message_sequence: 1,
            assistant_message_id: None,
            outcome,
            duration_ms: 7,
            error_message: Some("Safe failure".into()),
        };
        let mut row = super::super::tests::message(2, "workflow_event", "");
        row.role = None;
        row.visibility = "diagnostic_only".into();
        row.content_json = json!({"event":"channel_turn_terminal","turn":terminal});
        assert!(row.to_user_history_record().is_none());
        assert!(row.to_model_transcript_record().is_none());
        let turns = projection::turns(source, &[user, row]);
        assert_eq!(turns[0]["state"], expected);
        assert_eq!(turns[0]["duration"], 7);
        assert_eq!(
            turns[0]["responseParts"].as_array().unwrap().len(),
            usize::from(outcome == ChannelTurnOutcome::Failed)
        );
    }
}
