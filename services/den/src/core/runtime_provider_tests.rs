#[cfg(test)]
mod tests {
    use crate::{
        config::Config,
        core::runtime_provider::{
            acp_requires_letta_runtime, InteractionRunStore, RetrievalService,
            RoleProfileRegistry, RoleRunner, RoleRuntimeBinding, RuntimeStartupCapabilities,
            ToolActuatorRegistry,
        },
        errors::CustomError,
    };

    #[test]
    fn acp_requires_letta_runtime_when_gateway_enabled() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = true;
        assert!(acp_requires_letta_runtime(&config));
    }

    #[test]
    fn acp_does_not_require_letta_runtime_when_gateway_disabled() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = false;
        assert!(!acp_requires_letta_runtime(&config));
    }

    #[test]
    fn startup_capabilities_reflect_current_acp_to_letta_requirement() {
        let mut config = Config::test_stub();
        config.acp_gateway_enabled = true;
        let caps = RuntimeStartupCapabilities::from_config(&config);
        assert!(caps.acp_gateway_enabled);
        assert!(caps.letta_required_for_acp);

        config.acp_gateway_enabled = false;
        let caps = RuntimeStartupCapabilities::from_config(&config);
        assert!(!caps.acp_gateway_enabled);
        assert!(!caps.letta_required_for_acp);
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
        assert_eq!(InteractionRunStore::check_health(&noop).await.unwrap(), "ok");
        assert_eq!(RetrievalService::check_health(&noop).await.unwrap(), "ok");
    }
}
