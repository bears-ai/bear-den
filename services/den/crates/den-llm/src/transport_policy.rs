//! Provider-neutral primary request transport policy for a verified Den model.

use crate::LlmApiStyle;

/// Unknown gateway method support is a caller-context decision, not a capability
/// inferred from a provider/model name. Pair and explicit reasoning prefer Responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryTransportPreference {
    ResponsesWhenUnknown,
    ChatCompletionsWhenUnknown,
}

/// Known gateway support wins. With unknown support, use the explicit policy;
/// this selects a transport to attempt, not an assertion of gateway capability.
pub fn primary_api_style_for_catalog_support(
    supports_responses_api: Option<bool>,
    preference: PrimaryTransportPreference,
) -> LlmApiStyle {
    match supports_responses_api {
        Some(true) => LlmApiStyle::ResponsesStream,
        Some(false) => LlmApiStyle::ChatCompletionsStream,
        None => match preference {
            PrimaryTransportPreference::ResponsesWhenUnknown => LlmApiStyle::ResponsesStream,
            PrimaryTransportPreference::ChatCompletionsWhenUnknown => {
                LlmApiStyle::ChatCompletionsStream
            }
        },
    }
}

#[cfg(test)]
#[path = "transport_policy_tests.rs"]
mod tests;
