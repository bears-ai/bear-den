use super::PendingConversationId;

#[test]
fn provisional_client_targets_are_parsed_once_not_granted_authority() {
    let pending = "new-acp-zed-provisional";
    assert_eq!(
        PendingConversationId::parse(pending).unwrap().as_str(),
        pending
    );
    for invalid in [
        "",
        "new-",
        "den-conv-history",
        "new- provisional",
        "new-provisional\n",
    ] {
        assert!(
            PendingConversationId::parse(invalid).is_none(),
            "{invalid:?}"
        );
    }
}
