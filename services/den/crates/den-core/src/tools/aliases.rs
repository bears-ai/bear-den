use crate::tools::descriptor::builtin_den_tool_descriptors;

use crate::tools::constants::{
    DEN_BEAR_ENVIRONMENT, DEN_CONVERSATION_SET_TITLE, DEN_MEMORY_APPLY_CORE_UPDATE,
    DEN_MEMORY_LIST_PROPOSALS, DEN_MEMORY_MARK_LIFECYCLE, DEN_MEMORY_ORIENT_WORK_SURFACE,
    DEN_MEMORY_READ, DEN_MEMORY_READ_PROPOSAL, DEN_MEMORY_REQUEST_REVIEW,
    DEN_MEMORY_RESOLVE_PROPOSAL, DEN_MEMORY_SEARCH, DEN_MEMORY_STATUS, DEN_MEMORY_TREE,
    DEN_MEMORY_TREE_LEGACY_PROVIDER, DEN_MEMORY_WRITE_ENTRY, DEN_PLAN_MODE_RECORD_APPROVAL,
    DEN_PROMPT_MEMORY_LIST, DEN_PROMPT_MEMORY_PATCH, DEN_PROMPT_MEMORY_UPSERT, DEN_SITUATION_GET,
    DEN_SITUATION_GET_LEGACY_PROVIDER, DEN_WEB_FETCH, DEN_WEB_FETCH_LEGACY_PROVIDER,
    DEN_WEB_SEARCH,
};

pub fn provider_aliases_for_tool(name: &str) -> &'static [&'static str] {
    match name {
        DEN_WEB_FETCH => &[DEN_WEB_FETCH_LEGACY_PROVIDER],
        DEN_WEB_SEARCH => &["den_web_search"],
        DEN_CONVERSATION_SET_TITLE => &[
            "set_thread_title",
            "rename_conversation",
            "rename_thread",
            "conversation_rename",
        ],
        DEN_PLAN_MODE_RECORD_APPROVAL => &["approve_plan", "approve_current_plan"],
        DEN_BEAR_ENVIRONMENT => &["den_bear_environment"],
        DEN_SITUATION_GET => &[DEN_SITUATION_GET_LEGACY_PROVIDER, "den_situation_get"],
        DEN_MEMORY_WRITE_ENTRY => &["den_memory_write_entry"],
        DEN_MEMORY_STATUS => &["den_memory_status"],
        DEN_MEMORY_TREE => &[DEN_MEMORY_TREE_LEGACY_PROVIDER, "den_memory_tree"],
        DEN_MEMORY_READ => &["den_memory_read"],
        DEN_MEMORY_SEARCH => &["den_memory_search"],
        DEN_MEMORY_ORIENT_WORK_SURFACE => &["den_memory_orient_work_surface"],
        DEN_MEMORY_REQUEST_REVIEW => &["den_memory_request_review"],
        DEN_PROMPT_MEMORY_UPSERT => &["den_prompt_memory_upsert"],
        DEN_PROMPT_MEMORY_LIST => &["den_prompt_memory_list"],
        DEN_PROMPT_MEMORY_PATCH => &["den_prompt_memory_patch"],
        DEN_MEMORY_LIST_PROPOSALS => &["den_memory_list_proposals"],
        DEN_MEMORY_READ_PROPOSAL => &["den_memory_read_proposal"],
        DEN_MEMORY_RESOLVE_PROPOSAL => &["den_memory_resolve_proposal"],
        DEN_MEMORY_APPLY_CORE_UPDATE => &["den_memory_apply_core_update"],
        DEN_MEMORY_MARK_LIFECYCLE => &["den_memory_mark_lifecycle"],

        _ => &[],
    }
}

pub fn canonical_builtin_den_tool(name: &str) -> Option<&'static str> {
    builtin_den_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == name || descriptor.provider_name == name)
        .map(|descriptor| descriptor.name)
}

/// True when `name` is the canonical or provider name of a builtin Den tool.
///
/// Derived from [`builtin_den_tool_descriptors`] rather than a hand-kept list;
/// canonical and provider aliases share one descriptor-owned resolver.
pub fn is_builtin_den_tool(name: &str) -> bool {
    canonical_builtin_den_tool(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::constants::{DEN_TASK_FOCUS, DEN_TASK_FOCUS_PROVIDER};

    #[test]
    fn advertised_provider_names_resolve_to_their_canonical_tool() {
        assert_eq!(
            canonical_builtin_den_tool(DEN_TASK_FOCUS_PROVIDER),
            Some(DEN_TASK_FOCUS)
        );
        assert!(is_builtin_den_tool(DEN_TASK_FOCUS_PROVIDER));
    }
}
