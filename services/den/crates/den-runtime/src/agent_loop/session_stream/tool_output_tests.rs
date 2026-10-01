use super::*;

#[test]
fn compacted_tool_output_read_requires_its_verified_descriptor_audience() {
    for origin in [
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::BrowserTaskSession,
    ] {
        assert!(
            require_tool_output_read_origin(origin).is_ok(),
            "{origin:?}"
        );
    }
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(
            matches!(
                require_tool_output_read_origin(origin),
                Err(DenError::Authorization(_))
            ),
            "{origin:?}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn compacted_tool_output_read_rechecks_actor_before_artifact_lookup(
    pool: sqlx::PgPool,
) -> Result<(), DenError> {
    let result = tool_output_read_result(
        &pool,
        Uuid::new_v4(),
        Some(1),
        "invented-conversation",
        "invented-session",
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        serde_json::json!({ "artifact_ref": "tool-output://invented" }),
    )
    .await;
    assert!(matches!(result, Err(DenError::Authorization(_))));
    Ok(())
}
