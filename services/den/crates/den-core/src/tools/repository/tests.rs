use super::*;
use crate::tools::{
    constants::DEN_REPOSITORY_HEAD, descriptor::builtin_den_tool_descriptor_for_provider_name,
};

#[test]
fn schema_and_parser_accept_only_the_surface_uuid() {
    let descriptor = builtin_den_tool_descriptor_for_provider_name("repository_head").unwrap();
    assert_eq!(descriptor.name, DEN_REPOSITORY_HEAD);
    assert_eq!(descriptor.execution_target, "den");
    assert_eq!(descriptor.permissions, &["repository.head.read"]);
    assert_eq!(descriptor.availability, "credential_backend_unconfigured");
    assert_eq!(descriptor.input_schema["additionalProperties"], false);
    assert_eq!(
        descriptor.input_schema["properties"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(!descriptor.allows_origin(TurnExecutionOrigin::InternalCuration));
    assert!(!descriptor.allows_origin(TurnExecutionOrigin::InboundObservation));
    let surface = Uuid::new_v4();
    assert!(serde_json::from_value::<RepositoryHeadArguments>(
        serde_json::json!({"work_surface_id":surface})
    )
    .is_ok());
    for key in [
        "url",
        "token",
        "hat_id",
        "connection_id",
        "branch",
        "headers",
        "backend_binding_id",
    ] {
        let mut args = serde_json::json!({"work_surface_id":surface});
        args[key] = serde_json::json!("canary");
        assert!(serde_json::from_value::<RepositoryHeadArguments>(args).is_err());
    }
}

#[test]
fn result_is_only_surface_and_a_validated_sha() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let result = RepositoryHeadResult {
        work_surface_id: RepositorySurfaceId(Uuid::new_v4()),
        commit_sha: CommitSha::parse(sha).unwrap(),
    };
    let payload = serde_json::to_value(result).unwrap();
    assert_eq!(payload.as_object().unwrap().len(), 2);
    assert_eq!(payload["commit_sha"], sha);
    for invalid in [
        "ghp_canary",
        "",
        "0000000000000000000000000000000000000000",
        "0123456789ABCDEF0123456789ABCDEF01234567",
    ] {
        assert!(CommitSha::parse(invalid).is_err());
    }
    assert_eq!(
        RepositoryError::CredentialUnavailable.to_string(),
        "credential_unavailable"
    );
}
