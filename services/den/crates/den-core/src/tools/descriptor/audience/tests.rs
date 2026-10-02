use super::*;
use crate::{
    tools::descriptor::{
        builtin_den_tool_descriptor_for_provider_name, builtin_den_tool_descriptors_for_origin,
        builtin_den_tool_descriptors_for_profile, DenToolDescriptor,
    },
    ArmatureAvailability,
};
use std::collections::BTreeSet;

#[test]
fn typed_origin_matrix_preserves_profile_compatibility_without_using_it_as_authority() {
    for (profile, origin) in [
        (BearProfile::Chat, TurnExecutionOrigin::ChannelConversation),
        (
            BearProfile::Pair,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        ),
        (BearProfile::Pair, TurnExecutionOrigin::BrowserTaskSession),
        (
            BearProfile::Work,
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        ),
        (BearProfile::Curate, TurnExecutionOrigin::InternalCuration),
        (BearProfile::Watch, TurnExecutionOrigin::InboundObservation),
    ] {
        let legacy = builtin_den_tool_descriptors_for_profile(profile)
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect::<BTreeSet<_>>();
        let current = builtin_den_tool_descriptors_for_origin(origin)
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(current, legacy, "{origin:?}");
        assert_eq!(
            ToolAudience::from_origin(origin).compatibility_profile(),
            profile
        );
    }
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(builtin_den_tool_descriptors_for_origin(origin).is_empty());
        for descriptor in crate::tools::descriptor::builtin_den_tool_descriptors() {
            assert!(!descriptor.allows_origin(origin));
            assert!(!descriptor.allowed_roles.contains(&"curate"));
            assert!(!descriptor.allowed_roles.contains(&"watch"));
        }
    }
    let fetch = builtin_den_tool_descriptor_for_provider_name("web_fetch").unwrap();
    assert_eq!(fetch.allowed_roles, vec!["pair"]);
    assert!(
        fetch.allows_origin(TurnExecutionOrigin::ArmatureConversation(
            ArmatureAvailability::Connected
        ))
    );
    assert!(!fetch.allows_origin(TurnExecutionOrigin::AuthorizedWorkRun(
        ArmatureAvailability::Connected
    )));
}

#[test]
fn deserialized_descriptor_role_and_origin_claims_never_grant_execution() {
    let builtin = builtin_den_tool_descriptor_for_provider_name("web_fetch").unwrap();
    let mut untrusted = serde_json::to_value(builtin).unwrap();
    untrusted["allowed_roles"] = serde_json::json!(["pair", "curate", "work"]);
    untrusted["allowed_origins"] = serde_json::json!([
        "armature_conversation",
        "internal_curation",
        "authorized_work_run"
    ]);
    let decoded: DenToolDescriptor = serde_json::from_value(untrusted).unwrap();
    for origin in [
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
    ] {
        assert!(!decoded.allows_origin(origin), "{origin:?}");
    }
    assert!(!decoded.allows_profile(BearProfile::Curate));
}
