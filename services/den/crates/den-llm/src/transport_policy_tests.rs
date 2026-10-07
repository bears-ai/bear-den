use super::*;

#[test]
fn pair_and_explicit_reasoning_prefer_responses_for_unknown_methods() {
    assert_eq!(
        primary_api_style_for_catalog_support(
            None,
            PrimaryTransportPreference::ResponsesWhenUnknown
        ),
        LlmApiStyle::ResponsesStream
    );
    assert_eq!(
        primary_api_style_for_catalog_support(
            None,
            PrimaryTransportPreference::ChatCompletionsWhenUnknown
        ),
        LlmApiStyle::ChatCompletionsStream
    );
}

#[test]
fn known_catalog_support_wins_over_both_unknown_preferences() {
    for preference in [
        PrimaryTransportPreference::ResponsesWhenUnknown,
        PrimaryTransportPreference::ChatCompletionsWhenUnknown,
    ] {
        assert_eq!(
            primary_api_style_for_catalog_support(Some(true), preference),
            LlmApiStyle::ResponsesStream
        );
        assert_eq!(
            primary_api_style_for_catalog_support(Some(false), preference),
            LlmApiStyle::ChatCompletionsStream
        );
    }
}
