use super::*;
use den_service::bifrost::BifrostCatalogEntry;

#[test]
fn catalog_metadata_preserves_explicit_reasoning_support_and_leaves_unknown_unknown() {
    for support in [Some(true), Some(false), None] {
        let entry = BifrostCatalogEntry {
            available: true,
            provider: "openai".into(),
            provider_model_id: "gpt-5".into(),
            gateway_handle: "openai/gpt-5".into(),
            display_name: Some("Catalog model".into()),
            context_window: 128_000,
            max_output_tokens: Some(16_000),
            supports_tools: Some(true),
            supports_responses_api: Some(true),
            supports_vision: Some(true),
            supports_reasoning_effort: support,
        };
        let metadata = catalog_metadata_json(&entry);
        assert_eq!(metadata["supports_reasoning_effort"], json!(support));
        assert_eq!(metadata["context_window"], 128_000);
        assert_eq!(metadata["supports_tools"], true);
    }
}
