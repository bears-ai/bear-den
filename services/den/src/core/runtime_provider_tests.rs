#[cfg(test)]
mod tests {
    use crate::{
        config::Config,
        core::runtime_provider::{acp_requires_letta_runtime, RuntimeStartupCapabilities},
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
}
