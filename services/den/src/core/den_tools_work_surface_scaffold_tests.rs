use super::den_tools::{
    normalize_work_surface_slug, work_surface_entry_body, work_surface_index_file_body,
    work_surface_scaffold_requests,
};
use crate::core::bears::BearAgentRole;

#[test]
fn normalize_work_surface_slug_collapses_non_alphanumeric_runs() {
    assert_eq!(
        normalize_work_surface_slug(" Core/API Surface ").unwrap(),
        "core-api-surface"
    );
}

#[test]
fn work_surface_registry_seed_is_stable() {
    assert_eq!(
        work_surface_index_file_body(),
        "# Work Surfaces\n\nThis index lists the Bear's registered Work Surfaces.\n"
    );
}

#[test]
fn work_surface_entry_body_links_to_scaffold_index() {
    assert_eq!(
        work_surface_entry_body("meta", "Meta"),
        "- [Meta](./meta/index.md)"
    );
}

#[test]
fn work_surface_scaffold_requests_cover_registry_and_anchor_files() {
    let requests = work_surface_scaffold_requests(
        BearAgentRole::Pair,
        "meta",
        "Meta",
        "Meta overview.",
        Some("Term: meaning."),
        Some("Current understanding."),
    );
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].target_path, "core/work_surfaces/index.md");
    assert_eq!(requests[0].mode, "create_file");
    assert_eq!(requests[1].target_path, "core/work_surfaces/index.md");
    assert_eq!(requests[1].mode, "append_section");
    assert_eq!(requests[2].target_path, "core/work_surfaces/meta/index.md");
    assert_eq!(
        requests[3].target_path,
        "core/work_surfaces/meta/overview.md"
    );
    assert_eq!(
        requests[4].target_path,
        "core/work_surfaces/meta/glossary.md"
    );
    assert_eq!(
        requests[5].target_path,
        "pair/work_surfaces/meta/current-understanding.md"
    );
}

#[test]
fn work_surface_scaffold_requests_use_work_role_local_path_when_role_is_work() {
    let requests = work_surface_scaffold_requests(
        BearAgentRole::Work,
        "meta",
        "Meta",
        "Meta overview.",
        Some("Term: meaning."),
        None,
    );
    assert_eq!(requests.len(), 6);
    assert_eq!(
        requests[5].target_path,
        "work/work_surfaces/meta/current-understanding.md"
    );
}

#[test]
fn work_surface_scaffold_requests_skip_role_local_file_for_talk() {
    let requests = work_surface_scaffold_requests(
        BearAgentRole::Talk,
        "meta",
        "Meta",
        "Meta overview.",
        Some("Term: meaning."),
        None,
    );
    assert_eq!(requests.len(), 5);
    assert!(requests
        .iter()
        .all(|request| !request.target_path.starts_with("talk/work_surfaces/")));
}
