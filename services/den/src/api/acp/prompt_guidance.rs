pub(super) fn maybe_workspace_tool_guidance(tool_names: &[&str]) -> Vec<String> {
    let mut guidance = Vec::new();
    if tool_names.contains(&"fs_list_directory") {
        guidance.push(
            "Use `fs_list_directory` with {{\"path\":\"/absolute/dir\",\"limit\":200}} to discover files.".to_string(),
        );
    }
    if tool_names.contains(&"fs_search_files") {
        guidance.push(
            "Use `fs_search_files` with {{\"path\":\"/absolute/path\",\"query\":\"text\",\"limit\":50,\"extensions\":[\"rs\"],\"pattern\":\"src/*\"}} to search.".to_string(),
        );
    }
    if tool_names.contains(&"fs_read_text_file") {
        guidance.push(
            "Use `fs_read_text_file` with {{\"path\":\"/absolute/file\",\"line\":1,\"limit\":400}} to read. Do not guess file contents.".to_string(),
        );
    }
    if tool_names.contains(&"fs_edit_file") {
        guidance.push(
            "Use `fs_edit_file` with {{\"path\":\"/absolute/file\",\"old_text\":\"exact\",\"new_text\":\"replacement\"}} to modify existing text files. It edits by replacing one exact `old_text` span with `new_text`, so read the file first and choose a unique span. Calling `fs_edit_file` is how you request local approval for an edit; do not ask for approval in chat when this tool is available.".to_string(),
        );
        guidance.push(
            "ACP edit workflow: discover/read the target, call `fs_edit_file` to request approval and perform the edit, wait for its result, verify the change with `fs_read_text_file`, then provide a concise final answer naming the changed file and what changed. Never claim you are blocked by approval if `fs_edit_file` is callable; invoke it instead.".to_string(),
        );
    } else {
        guidance.push(
            "No ACP edit tool is callable in this turn. Do not claim to request edit approval or ask for approval in chat; explain that editing is unavailable if asked to modify files.".to_string(),
        );
    }
    if tool_names.contains(&"fs_create_text_file") {
        guidance.push(
            "Use `fs_create_text_file` with {{\"path\":\"/absolute/new-file.txt\",\"content\":\"text\"}} to create new UTF-8 text files. It will not overwrite existing files; use `create_parent_dirs:true` only when parent directories should be created.".to_string(),
        );
    }
    if tool_names.contains(&"fs_delete_path") {
        guidance.push(
            "Use `fs_delete_path` with {{\"path\":\"/absolute/path\",\"expected_kind\":\"file\"}} to delete files or empty directories. For non-empty directories, `recursive:true` is required. Deleting workspace roots and sensitive paths is denied.".to_string(),
        );
    }
    guidance
}

pub(super) fn server_memory_tool_guidance() -> Vec<String> {
    vec![
        "Use server tools for non-local capabilities: `session_info` for trusted information about the authenticated human, current bear, role, session, memory scopes, and policy; `memory_write_entry` only for durable pair-local notes, logs, decisions, reflections, scratch, and summaries attributed to the authenticated human; `memory_status`, `memory_browse`, `memory_read`, and `memory_search` to inspect Bear memory; `memory_request_review` to ask Reflection/curate to review role-local memory without writing shared memory directly; `update_plan` to create and maintain the visible ACP task plan for the current mini-project with at most one `in_progress` item; `get_plan_status` and `list_plans` to recover visible plan state; `request_work_handoff` when channel work should become a durable reviewed task intent; `web_fetch` for bounded HTTP(S) page fetching; and `web_search` only when a Den search provider is configured. Do not switch ACP session modes yourself: Plan/Write/Ask mode is controlled by the user or ACP client UI. When planning would help, use `update_plan` and concise prose while remaining in the current mode; do not use memory entry tools for active plans, task lists, observations, run results, Cabinet writes, or direct core updates.".to_string(),
        "Memory is Bear-scoped across Workplaces and may contain multiple work surfaces. A Workplace is the role-scoped memory surface; for pair, that is the `pair` workplace. For questions about the current project, repo, service, architecture, terminology, or prior local decisions, first identify the relevant current work surface from trusted session hints, workspace roots, repo clues, or explicit user references rather than treating all Bear memory as one flat pool.".to_string(),
        "Prefer work-surface-first retrieval for local-understanding questions: current conversation and trusted session info -> current Workplace and current work-surface hints -> current work-surface canonical anchors -> current work-surface role-local working memory -> Bear-global shared anchors -> broader Bear memory search -> local workspace artifacts -> general world knowledge.".to_string(),
        "Use `memory_browse`, `memory_read`, and `memory_search` not only to recall prior notes, but to learn the current work surface within the current Workplace. If canonical work-surface anchors exist, prefer them over broad memory search for questions like 'what do you know about this?' or 'how does this work here?'.".to_string(),
        "Use `session_info.work_surface` as the trusted Den briefing for current Workplace/work-surface hints when available. Treat its reference candidates as guidance to resolve the active work surface, then confirm against canonical anchors and explicit user intent.".to_string(),
    ]
}

pub(super) fn tool_loop_rule_guidance() -> String {
    "Tool-loop rule: after any ACP tool result, continue from the returned content until the user's original request is complete. Do not stop merely because a tool succeeded. Do not ask the user whether to continue when the next step is implied by the original request. Stop only for required local approval, missing information, unrecoverable errors, or when you have verified and summarized completion. Never write textual tool-call syntax such as `to=functions...` or `functions.fs_edit_file`; if a tool is not callable, explain the limitation in normal prose.".to_string()
}
