use crate::tools::{
    aliases::canonical_builtin_den_tool,
    constants::*,
    descriptor::{
        builtin_den_tool_descriptor_for_provider_name, builtin_den_tool_descriptors,
        builtin_den_tool_descriptors_for_origin, builtin_den_tool_descriptors_for_profile,
    },
    dispatch::has_known_executor,
    identity::authorize_tool_for_origin,
};
use crate::{ArmatureAvailability, DenError, RuntimeContextLabel, TurnExecutionOrigin};

const RETIRED_NAMES: &[&str] = &[
    DEN_MEMORY_ORIENT_WORK_SURFACE,
    DEN_MEMORY_ORIENT_WORK_SURFACE_PROVIDER,
    "den_memory_orient_work_surface",
    DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD,
    DEN_MEMORY_CREATE_WORK_SURFACE_SCAFFOLD_PROVIDER,
    DEN_ENTITY_MERGE,
    DEN_ENTITY_MERGE_PROVIDER,
    DEN_ENTITY_SPLIT,
    DEN_ENTITY_SPLIT_PROVIDER,
    DEN_ENTITY_WRITE_ACCESS_RULE,
    DEN_ENTITY_WRITE_ACCESS_RULE_PROVIDER,
    DEN_ENTITY_WRITE_ANCHOR,
    DEN_ENTITY_WRITE_ANCHOR_PROVIDER,
    DEN_MEMORY_LIST_PROPOSALS,
    DEN_MEMORY_LIST_PROPOSALS_PROVIDER,
    DEN_MEMORY_READ_PROPOSAL,
    DEN_MEMORY_READ_PROPOSAL_PROVIDER,
    DEN_MEMORY_RESOLVE_PROPOSAL,
    DEN_MEMORY_RESOLVE_PROPOSAL_PROVIDER,
    DEN_MEMORY_MARK_LIFECYCLE,
    DEN_MEMORY_MARK_LIFECYCLE_PROVIDER,
    DEN_SKILL_APPROVE_PROPOSAL,
    DEN_SKILL_REJECT_PROPOSAL,
    DEN_TASK_APPROVE_INTENT,
    DEN_TASK_REJECT_INTENT,
    DEN_CORE_WRITE_RESULT_SUMMARY,
    DEN_OBSERVATION_WRITE,
];

#[test]
fn retired_memory_tools_have_no_descriptor_resolver_or_executor() {
    for name in RETIRED_NAMES {
        assert!(canonical_builtin_den_tool(name).is_none(), "{name}");
        assert!(
            builtin_den_tool_descriptor_for_provider_name(name).is_none(),
            "{name}"
        );
        assert!(!has_known_executor(name), "{name}");
    }
    for descriptor in builtin_den_tool_descriptors() {
        for name in RETIRED_NAMES {
            assert!(
                !descriptor.description.contains(name),
                "{}: {name}",
                descriptor.name
            );
        }
    }
    let lists =
        builtin_den_tool_descriptor_for_provider_name(DEN_TASK_LISTS_LIST_PROVIDER).unwrap();
    assert!(lists.input_schema["properties"]
        .get("include_artifacts")
        .is_none());
}

#[test]
fn retirement_is_independent_of_profile_and_execution_origin() {
    for profile in [
        RuntimeContextLabel::ChannelConversation,
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::Curation,
        RuntimeContextLabel::Observation,
    ] {
        assert!(builtin_den_tool_descriptors_for_profile(profile)
            .iter()
            .all(|descriptor| !RETIRED_NAMES.contains(&descriptor.name)));
    }
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(builtin_den_tool_descriptors_for_origin(origin)
            .iter()
            .all(|descriptor| !RETIRED_NAMES.contains(&descriptor.name)));
        for name in RETIRED_NAMES {
            assert!(
                matches!(
                    authorize_tool_for_origin(name, origin),
                    Err(DenError::NotFound(_))
                ),
                "{origin:?}: {name}"
            );
        }
    }
}

#[test]
fn canonical_resource_descriptors_remain_available() {
    for name in [
        DEN_WORK_CATALOG_PROVIDER,
        DEN_SITUATION_GET_PROVIDER,
        DEN_MEMORY_WRITE_ENTRY_PROVIDER,
    ] {
        assert!(
            builtin_den_tool_descriptor_for_provider_name(name).is_some(),
            "{name}"
        );
    }
}
