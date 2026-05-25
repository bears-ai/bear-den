use crate::{config::Config, errors::CustomError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProviderKind {
    Letta,
}

impl RuntimeProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Letta => "letta",
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait RuntimeProviderHealthCheck {
    fn kind(&self) -> RuntimeProviderKind;
    fn enabled(&self) -> bool;
    async fn check_health(&self) -> Result<String, CustomError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStartupCapabilities {
    pub acp_gateway_enabled: bool,
    pub letta_required_for_acp: bool,
}

impl RuntimeStartupCapabilities {
    pub fn from_config(config: &Config) -> Self {
        Self {
            acp_gateway_enabled: config.acp_gateway_enabled,
            letta_required_for_acp: config.acp_gateway_enabled,
        }
    }
}

pub fn acp_requires_letta_runtime(config: &Config) -> bool {
    RuntimeStartupCapabilities::from_config(config).letta_required_for_acp
}

#[allow(async_fn_in_trait)]
pub trait RoleProfileRegistry {
    async fn resolve_provider_binding(
        &self,
        bear_id: uuid::Uuid,
        role: &str,
    ) -> Result<Option<String>, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait RoleRunner {
    async fn check_health(&self) -> Result<String, CustomError>;
}

#[allow(async_fn_in_trait)]
pub trait InteractionRunStore {
    async fn check_health(&self) -> Result<String, CustomError>;
}

pub trait ToolActuatorRegistry {}

#[allow(async_fn_in_trait)]
pub trait RetrievalService {
    async fn check_health(&self) -> Result<String, CustomError>;
}
