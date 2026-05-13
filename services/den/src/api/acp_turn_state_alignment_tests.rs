use serde_json::json;

use crate::{
    core::acp_sessions::AcpSessionRow,
    api::acp::acp_session_row_to_http_with_modes,
};

#[test]
fn acp_session_http_surfaces_turn_state_as_primary_and_legacy_states_as_compatibility_only() {
    let row = AcpSessionRow {
        id: uuid::Uuid::nil(),
        user_id: 1,
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        acp_session_id: "acp-test".to_string(),
        runtime_session_id: "runtime-test".to_string(),
        conversation_id: "conv-test".to_string(),
        resolved_conversation_id: None,
        client: "zed".to_string(),
        cwd: Some("/workspace".to_string()),
        current_mode: "write".to_string(),
        conversation_title: None,
        conversation_title_updated_at: None,
        conversation_title_synced_at: None,
        closed_at: None,
        archived_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let plan_mode = Some(json!({
        "id": uuid::Uuid::nil(),
        "state": "approved",
        "plan_artifact_path": "pair/plans/example.md",
        "reason": "test",
        "requested_by": "pair"
    }));

    let payload = serde_json::to_value(acp_session_row_to_http_with_modes(row, plan_mode)).unwrap();
    assert_eq!(payload["workflow_state"]["schema"], "bears.turn_state/v1");
    assert_eq!(payload["workflow_state"]["workplan"]["state"], "approved");
    let legacy_states = payload["legacy_states"].as_array().expect("legacy_states array");
    assert!(!legacy_states.is_empty());
    assert!(legacy_states.iter().all(|entry| entry["compatibility_only"] == true));
}
