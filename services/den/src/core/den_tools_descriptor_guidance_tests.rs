use crate::core::den_tools::{
    builtin_den_tool_descriptors, DEN_MEMORY_WRITE_ENTRY_PROVIDER, DEN_SITUATION_GET_PROVIDER,
    DEN_WORK_PLAN_UPDATE_PROVIDER,
};

#[test]
fn session_info_descriptor_keeps_explicit_orientation_language() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_SITUATION_GET_PROVIDER)
        .expect("session_info descriptor");

    assert!(
        descriptor
            .description
            .contains("Trusted Den orientation tool"),
        "unexpected description: {}",
        descriptor.description
    );
    assert!(
        descriptor.description.contains("Read-only"),
        "unexpected description: {}",
        descriptor.description
    );
}

#[test]
fn memory_write_descriptor_includes_shared_guidance() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_MEMORY_WRITE_ENTRY_PROVIDER)
        .expect("memory_write_entry descriptor");

    assert!(descriptor.description.contains("Scope:"));
    assert!(descriptor.description.contains("Side effect:"));
    assert!(descriptor.description.contains("session_info"));
    assert!(descriptor
        .description
        .contains("writes role-local semantic memory"));
}

#[test]
fn work_plan_update_descriptor_includes_active_work_state_guidance() {
    let descriptor = builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.provider_name == DEN_WORK_PLAN_UPDATE_PROVIDER)
        .expect("update_plan descriptor");

    assert!(descriptor.description.contains("Scope:"));
    assert!(descriptor
        .description
        .contains("Side effect: updates active work state"));
    assert!(descriptor.description.contains("session_info"));
}
