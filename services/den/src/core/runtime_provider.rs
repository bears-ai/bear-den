use crate::config::Config;

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

pub fn acp_requires_letta_runtime(config: &Config) -> bool {
    config.acp_gateway_enabled
}
