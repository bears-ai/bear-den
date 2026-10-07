use super::*;
use uuid::Uuid;

fn example() -> PortableHat {
    PortableHat {
        original_id: HatId::new(Uuid::new_v4()),
        model_configuration_id: None,
        name: "Home".into(),
        purpose: "Care for the house".into(),
        short_summary: Some("House care".into()),
        identity_prompt: "Maintain the home".into(),
        work_requested: true,
        automatic_sharing_requested: true,
        repository_names: vec!["house".into()],
        https_hosts: vec!["example.com".into()],
        web_fetch_requested: true,
        web_search_requested: false,
    }
}

#[test]
fn portable_hats_require_unique_identity_and_names_and_a_real_default() {
    let hat = example();
    assert!(validate(std::slice::from_ref(&hat), Some(hat.original_id)).is_ok());
    assert!(validate(&[hat.clone(), hat.clone()], None).is_err());
    assert!(validate(std::slice::from_ref(&hat), Some(HatId::new(Uuid::new_v4()))).is_err());
    let mut invalid = example();
    invalid.name = " ".into();
    assert!(validate(&[invalid], None).is_err());
}

#[test]
fn portable_hat_intent_is_not_a_secret_or_execution_receipt() {
    let encoded = serde_json::to_value(example()).unwrap();
    assert!(encoded.get("work_requested").is_some());
    for forbidden in [
        "secret",
        "credential",
        "token",
        "work_enabled",
        "review_receipt",
        "source_owner",
    ] {
        assert!(encoded.get(forbidden).is_none(), "unexpected {forbidden}");
    }
    let mut invalid = example();
    invalid.https_hosts = vec!["localhost".into()];
    assert!(validate(&[invalid], None).is_err());
}
