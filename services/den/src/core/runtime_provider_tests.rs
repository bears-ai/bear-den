#[cfg(test)]
mod tests {
    use crate::{
        config::Config,
        core::runtime_provider::{
            acp_requires_runtime, AcpTurnRunner, CancelTurnRequest, ContinueTurnRequest,
            ContinueTurnResult, InteractionRunStore, RetrievalService, RoleProfileRegistry,
            RoleRunner, RoleRuntimeBinding, RuntimeApprovalDecision, RuntimeContinuation,
            RuntimeConversationRef, RuntimeStartupCapabilities, RuntimeStreamContinuation,
            RuntimeToolResultStatus, ToolActuatorRegistry,
        },
        errors::CustomError,
    };

    #[test]
    fn acp_requires_runtime_when_gateway_enabled() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = true;
        assert!(acp_requires_runtime(&config));
    }

    #[test]
    fn acp_does_not_require_letta_runtime_when_gateway_disabled() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = false;
        assert!(!acp_requires_runtime(&config));
    }

    #[test]
    fn startup_capabilities_reflect_current_acp_to_letta_requirement() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = true;
        let caps = RuntimeStartupCapabilities::from_config(&config);
        assert!(caps.acp_gateway_enabled);
        assert!(caps.runtime_required_for_acp);

        config.acp_gateway_enabled = false;
        let caps = RuntimeStartupCapabilities::from_config(&config);
        assert!(!caps.acp_gateway_enabled);
        assert!(!caps.runtime_required_for_acp);
    }

    struct NoopRegistry;

    impl ToolActuatorRegistry for NoopRegistry {}

    impl RoleProfileRegistry for NoopRegistry {
        async fn resolve_compatibility_binding(
            &self,
            _bear_id: uuid::Uuid,
            _role: &str,
        ) -> Result<Option<RoleRuntimeBinding>, CustomError> {
            Ok(None)
        }
    }

    impl RoleRunner for NoopRegistry {
        async fn check_health(&self) -> Result<String, CustomError> {
            Ok("ok".to_string())
        }
    }

    impl InteractionRunStore for NoopRegistry {
        async fn check_health(&self) -> Result<String, CustomError> {
            Ok("ok".to_string())
        }
    }

    impl RetrievalService for NoopRegistry {
        async fn check_health(&self) -> Result<String, CustomError> {
            Ok("ok".to_string())
        }
    }

    impl AcpTurnRunner for NoopRegistry {
        async fn preflight_hygiene(
            &self,
            _binding: &RoleRuntimeBinding,
            _conversation: Option<&RuntimeConversationRef>,
            _reason: &str,
        ) -> Result<(), CustomError> {
            Ok(())
        }

        async fn start_turn(
            &self,
            _request: crate::core::runtime_provider::StartTurnRequest,
        ) -> Result<crate::core::runtime_provider::StartTurnResult, CustomError> {
            Ok(crate::core::runtime_provider::StartTurnResult {
                turn: None,
                stream: crate::core::runtime_provider::RuntimeStreamContinuation::Deferred,
            })
        }

        async fn continue_turn(
            &self,
            _request: ContinueTurnRequest,
        ) -> Result<ContinueTurnResult, CustomError> {
            Ok(ContinueTurnResult {
                turn: None,
                stream: RuntimeStreamContinuation::Deferred,
            })
        }

        async fn cancel_turn(
            &self,
            _request: CancelTurnRequest,
        ) -> Result<crate::core::runtime_provider::CancelTurnResult, CustomError> {
            Ok(crate::core::runtime_provider::CancelTurnResult {
                skipped: true,
                detail: "noop".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn phase_zero_runtime_contracts_are_implementable() {
        let noop = NoopRegistry;
        let bear_id = uuid::Uuid::nil();

        assert_eq!(
            noop.resolve_compatibility_binding(bear_id, "pair")
                .await
                .unwrap(),
            None
        );
        assert_eq!(RoleRunner::check_health(&noop).await.unwrap(), "ok");
        assert_eq!(
            InteractionRunStore::check_health(&noop).await.unwrap(),
            "ok"
        );
        assert_eq!(RetrievalService::check_health(&noop).await.unwrap(), "ok");
        let binding = RoleRuntimeBinding {
            binding_id: "binding".to_string(),
            compatibility_backend: Some("letta".to_string()),
        };
        let conversation = RuntimeConversationRef {
            id: "conv-test".to_string(),
        };
        assert!(
            AcpTurnRunner::preflight_hygiene(&noop, &binding, Some(&conversation), "test")
                .await
                .is_ok()
        );
        assert_eq!(
            AcpTurnRunner::continue_turn(
                &noop,
                ContinueTurnRequest {
                    conversation: conversation.clone(),
                    turn: None,
                    binding: binding.clone(),
                    continuation: RuntimeContinuation::ToolResult {
                        tool_call_id: "call-1".to_string(),
                        approval_request_id: Some("approval-1".to_string()),
                        status: RuntimeToolResultStatus::Ok,
                        content: "tool ok".to_string(),
                    },
                }
            )
            .await
            .unwrap(),
            ContinueTurnResult {
                turn: None,
                stream: RuntimeStreamContinuation::Deferred,
            }
        );
        assert_eq!(
            AcpTurnRunner::continue_turn(
                &noop,
                ContinueTurnRequest {
                    conversation,
                    turn: None,
                    binding,
                    continuation: RuntimeContinuation::ApprovalDecision {
                        approval_request_id: "approval-2".to_string(),
                        tool_call_id: Some("call-2".to_string()),
                        decision: RuntimeApprovalDecision::Deny,
                        reason: Some("user denied".to_string()),
                    },
                }
            )
            .await
            .unwrap(),
            ContinueTurnResult {
                turn: None,
                stream: RuntimeStreamContinuation::Deferred,
            }
        );
    }
}
