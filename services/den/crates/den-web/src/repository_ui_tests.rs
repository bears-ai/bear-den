use super::render;
use minijinja::{context, Value};

#[test]
fn external_connections_are_inactive_and_never_echo_backend_references() {
    let html = render(
        "connections.html",
        context! {
            connection_catalog => vec![context! {id=>"account",name=>"Reference",provider=>"github_external",revision=>1,revoked=>false,repository_count=>0,repositories=>Vec::<Value>::new(),other_repository_count=>0}],
            repositories=>Vec::<Value>::new(),bears=>Vec::<Value>::new(),has_linkable_repositories=>false,
        },
    );
    assert!(html.contains("reference saved, not operational"));
    assert!(html.contains("no external credential backend is integrated"));
    assert!(html.contains("Previously distributed legacy credential copies are not erased"));
    assert!(html.contains("Save inactive external reference"));
    assert!(html.contains("name=\"confirm_external_boundary\""));
    assert!(!html.contains("name=\"external_secret_id\" required autocomplete=\"off\" value="));
}
