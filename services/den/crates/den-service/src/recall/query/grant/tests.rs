use den_core::ids::HatId;
use den_memory::MemorySource;

use super::*;

#[test]
fn vector_filter_never_admits_other_sessions_hats_or_legacy_profiles() {
    let bear = Uuid::new_v4();
    let source = MemorySource::Conversation(Uuid::new_v4());
    let hat = HatId::new(Uuid::new_v4());
    let filter = grant_scope_filter(
        bear,
        "bears-embed-v1",
        MemoryReadGrant::new(source, Some(hat)),
    );
    let must = filter["must"].as_array().expect("mandatory conditions");
    assert_eq!(must[0]["match"]["value"], bear.to_string());
    assert_eq!(must[2]["match"]["value"], "bears-embed-v1");
    let should = must[3]["should"].as_array().expect("scope alternatives");
    assert_eq!(should.len(), 2);
    assert_eq!(should[0]["match"]["value"], "shared");
    assert_eq!(should[1]["must"][0]["match"]["value"], "hat");
    assert_eq!(should[1]["must"][1]["match"]["value"], hat.to_string());
    assert!(!filter.to_string().contains("source_local"));
    assert!(!filter.to_string().contains("scope_profile"));
}

#[test]
fn no_hat_grant_does_not_add_a_hat_alternative() {
    let run = MemorySource::WorkRun(Uuid::new_v4());
    let filter = grant_scope_filter(
        Uuid::new_v4(),
        "bears-embed-v1",
        MemoryReadGrant::new(run, None),
    );
    let should = filter["must"][3]["should"].as_array().unwrap();
    assert_eq!(should.len(), 1);
    assert_eq!(should[0]["match"]["value"], "shared");
    assert!(!filter.to_string().contains("scope_hat_id"));
    assert!(!filter.to_string().contains("source_local"));
}
