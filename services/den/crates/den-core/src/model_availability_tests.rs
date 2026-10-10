use super::*;

#[test]
fn safe_label_preserves_the_selected_model_but_never_urls_credentials_or_markup() {
    let failure = ModelAvailabilityFailure::new(
        ModelAvailabilityFailureKind::ModelMissing,
        Some("openai/gpt-6-sol"),
    );
    assert_eq!(failure.model.as_ref().unwrap().as_str(), "openai/gpt-6-sol");
    assert!(failure.public_message().contains("openai/gpt-6-sol"));
    for unsafe_model in [
        "sk-bf-secret",
        "openai/sk-bf-secret",
        "openai/gpt?key=PRIVATE",
        "https://gateway/models?key=PRIVATE",
        "https://PRIVATE@gateway/models",
        "openai/<script>",
        "openai/gpt\nAuthorization: PRIVATE",
        "openai/token-PRIVATE",
        "user:PRIVATE/model",
    ] {
        let failure = ModelAvailabilityFailure::new(
            ModelAvailabilityFailureKind::ModelMissing,
            Some(unsafe_model),
        );
        assert!(failure.model.is_none());
        assert!(!format!("{failure:?} {failure}").contains("PRIVATE"));
        assert!(!failure.to_string().contains(unsafe_model));
    }
}

#[test]
fn every_reason_has_a_distinct_public_code_and_actionable_recovery() {
    let mut codes = std::collections::HashSet::new();
    for kind in [
        ModelAvailabilityFailureKind::ModelMissing,
        ModelAvailabilityFailureKind::ModelUnavailable,
        ModelAvailabilityFailureKind::VirtualKeyMissing,
        ModelAvailabilityFailureKind::VirtualKeyRejected,
        ModelAvailabilityFailureKind::CatalogUnavailable,
    ] {
        let descriptor = kind.descriptor();
        assert!(codes.insert(descriptor.code));
        assert!(!descriptor.recovery.is_empty());
    }
}
