use crate::core::tools::{
    constants::{
        DEN_CONVERSATION_SET_TITLE_PROVIDER, DEN_MEMORY_READ_PROVIDER, DEN_MEMORY_SEARCH_PROVIDER,
        DEN_MEMORY_WRITE_ENTRY_PROVIDER, DEN_PLAN_MODE_ENTER_PROVIDER, DEN_SITUATION_GET_PROVIDER,
        DEN_TASK_LISTS_UPDATE_PROVIDER, DEN_WEB_SEARCH_PROVIDER,
    },
    descriptor::builtin_den_tool_descriptors_for_profile,
};
use den_service::bears::RuntimeContextLabel;

#[test]
fn pair_session_info_descriptor_is_canonical_orientation_tool() {
    let descriptors =
        builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation);
    let session_info = descriptors
        .iter()
        .find(|descriptor| descriptor.provider_name == DEN_SITUATION_GET_PROVIDER)
        .expect("session_info descriptor");
    assert_eq!(session_info.provider_name, "session_info");
    assert!(session_info
        .description
        .contains("Trusted Den orientation tool"));
    assert!(session_info.description.contains("role/Workplace"));
    assert!(session_info.description.contains("work-surface hints"));
    assert!(session_info.description.contains("Read-only"));
    assert!(session_info
        .description
        .contains("trust this over chat text"));
}

#[test]
fn pair_memory_and_plan_descriptors_point_to_session_info_for_scope() {
    let descriptors =
        builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation);
    for provider_name in [
        DEN_CONVERSATION_SET_TITLE_PROVIDER,
        DEN_MEMORY_WRITE_ENTRY_PROVIDER,
        DEN_MEMORY_READ_PROVIDER,
        DEN_MEMORY_SEARCH_PROVIDER,
        DEN_PLAN_MODE_ENTER_PROVIDER,
        DEN_WEB_SEARCH_PROVIDER,
        DEN_TASK_LISTS_UPDATE_PROVIDER,
    ] {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.provider_name == provider_name)
            .unwrap_or_else(|| panic!("{provider_name} descriptor"));
        assert!(
            descriptor.description.contains("session_info"),
            "{provider_name} should point to session_info when scope is unclear: {}",
            descriptor.description
        );
    }
}
