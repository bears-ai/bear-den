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
