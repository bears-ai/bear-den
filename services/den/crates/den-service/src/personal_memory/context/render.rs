use den_core::DenError;
use den_memory::personal_candidates::PreferredName;
use serde_json::json;

use crate::bears::prompt_fragments::{render_turn_fragment, repository_prompt_fragment_registry};

use super::types::RenderedPersonalContext;

pub(super) fn render_name(name: &PreferredName) -> Result<RenderedPersonalContext, DenError> {
    let fragments = repository_prompt_fragment_registry()?;
    let fragment = fragments.require("runtime_personal_context")?;
    // Quote at this private boundary; interpolation never treats the value as template source.
    let model_facing_name = serde_json::to_string(name.as_str())
        .map_err(|_| DenError::Parsing("Personal-context data could not be encoded.".into()))?;
    let text = render_turn_fragment(fragment, &json!({ "modelFacingName": model_facing_name }))?;
    Ok(RenderedPersonalContext::new(text))
}
