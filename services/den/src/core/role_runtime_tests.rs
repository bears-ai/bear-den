#[cfg(test)]
mod tests {
    use crate::core::{
        acp_tool_turns::AcpToolTurnCoordinator,
        acp_turn_controller::AcpActiveTurnCancelRegistry,
        role_runtime::{AcpTurnLifecycleContext, AcpTurnLifecycleRuntime, RoleRuntimeRole},
    };
    use uuid::Uuid;

    #[test]
    fn acp_turn_lifecycle_runtime_builds_pair_scope() {
        let tool_turns = AcpToolTurnCoordinator::new();
        let cancellations = AcpActiveTurnCancelRegistry::new();
        let runtime = AcpTurnLifecycleRuntime::new(tool_turns, cancellations);
        let request_id = Uuid::new_v4();
        let bear_id = Uuid::new_v4();

        let lease = runtime
            .acquire_pair_turn(
                AcpTurnLifecycleContext {
                    bear_id,
                    acp_session_id: "session-1".to_string(),
                    resolved_conversation_id: Some("conv-1".to_string()),
                },
                request_id,
            )
            .expect("pair turn lease should be acquired");

        assert_eq!(lease.turn_scope.bear_id, bear_id);
        assert_eq!(lease.turn_scope.role, RoleRuntimeRole::Pair);
        assert_eq!(lease.turn_scope.channel_id, "session-1");
        assert_eq!(lease.turn_scope.conversation_id.as_deref(), Some("conv-1"));

        lease.active_turn_guard.release();
    }
}
