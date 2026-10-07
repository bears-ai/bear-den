use anyhow::{anyhow, Context, Result};
use bearwire_protocol::session::ExpectedWorkSource;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PromptSource {
    #[default]
    Conversation,
    Work(ExpectedWorkSource),
}

impl PromptSource {
    pub(super) fn add_to_params(self, params: &mut Value) -> Result<()> {
        if let Self::Work(source) = self {
            params["expected_work_source"] = serde_json::to_value(source)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum StartupSafety {
    Ordinary,
    ExactWorkSource,
}

#[derive(Default, Deserialize)]
struct ServerCapabilities {
    #[serde(default)]
    expected_work_source: bool,
}

#[derive(Deserialize)]
struct InitializeCapabilities {
    capabilities: Option<ServerCapabilities>,
}

pub(super) fn require_expected_work_support(initialize: Value) -> Result<()> {
    const UPGRADE_ERROR: &str = "Den does not advertise expected_work_source support; upgrade Den before starting headless Work (older servers may silently ignore exact Work source fields)";
    let initialize: InitializeCapabilities =
        serde_json::from_value(initialize).context(UPGRADE_ERROR)?;
    if !initialize
        .capabilities
        .unwrap_or_default()
        .expected_work_source
    {
        return Err(anyhow!(UPGRADE_ERROR));
    }
    Ok(())
}
