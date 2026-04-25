//! Tool selection policy for modern BEARS agents: drop legacy block-mutation tools in favor of
//! memfs / current Letta memory tools.
//!
//! **Letta API (Create/PATCH agent):** `include_base_tools: false` avoids attaching core_memory-style
//! tools automatically; we still filter explicit `tool_ids` by name using `GET /v1/tools/` catalog.
//! **Git-backed memory:** `git_enabled: true` on create/patch matches Context Repository / memfs
//! server-side flags (see Letta API agent create docs).

use super::LettaToolOption;

/// Tool `name` values (as returned by Letta `GET /v1/tools/`) to never attach to bears.
pub const LEGACY_MEMORY_TOOL_NAMES: &[&str] = &[
    "memory_apply_patch",
    "core_memory_append",
    "core_memory_replace",
];

fn norm_tool_name(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// True if this tool name is a legacy memory mutation tool we hide from bears.
pub fn is_legacy_memory_tool_name(name: &str) -> bool {
    let n = norm_tool_name(name);
    LEGACY_MEMORY_TOOL_NAMES
        .iter()
        .any(|legacy| norm_tool_name(legacy) == n)
}

/// Build id → tool name map from the Letta catalog (latest id wins if duplicated).
fn id_to_name_map(catalog: &[LettaToolOption]) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    for t in catalog {
        m.insert(t.id.clone(), t.label.clone());
    }
    m
}

/// Drop selected tool ids whose catalog name matches a legacy memory tool. Ids not present in
/// `catalog` are kept (forward-compatible with new tools).
pub fn filter_legacy_memory_tool_ids(
    catalog: &[LettaToolOption],
    selected_ids: &[String],
) -> Vec<String> {
    let map = id_to_name_map(catalog);
    selected_ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|id| {
            match map.get(*id) {
                Some(name) => !is_legacy_memory_tool_name(name),
                None => true,
            }
        })
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_known_legacy_by_label() {
        let catalog = vec![
            LettaToolOption {
                id: "a".into(),
                label: "core_memory_append".into(),
            },
            LettaToolOption {
                id: "b".into(),
                label: "memory_insert".into(),
            },
        ];
        let out = filter_legacy_memory_tool_ids(&catalog, &["a".into(), "b".into()]);
        assert_eq!(out, vec!["b"]);
    }

    #[test]
    fn keeps_unknown_ids() {
        let catalog: Vec<LettaToolOption> = vec![];
        let out = filter_legacy_memory_tool_ids(&catalog, &["unknown-1".into()]);
        assert_eq!(out, vec!["unknown-1"]);
    }
}
