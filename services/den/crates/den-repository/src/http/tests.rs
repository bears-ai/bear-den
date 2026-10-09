use super::*;

#[test]
fn private_mixed_and_wrong_port_dns_are_rejected() {
    assert!(validate_addresses(&["1.1.1.1:443".parse().unwrap()]).is_ok());
    for addresses in [
        vec![],
        vec!["127.0.0.1:443".parse().unwrap()],
        vec![
            "1.1.1.1:443".parse().unwrap(),
            "10.0.0.1:443".parse().unwrap(),
        ],
        vec!["1.1.1.1:80".parse().unwrap()],
        vec!["[::1]:443".parse().unwrap()],
    ] {
        assert_eq!(
            validate_addresses(&addresses),
            Err(RepositoryError::DestinationDenied)
        );
    }
}

#[test]
fn provider_echoes_and_substitutions_never_become_business_output() {
    let repository = GithubRepository::parse("https://github.com/acme/widget", "main").unwrap();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let safe = serde_json::json!({"ref":"refs/heads/main", "object":{"type":"commit", "sha":sha}, "url":"ghp_canary", "message":"Bearer ghp_canary"});
    assert_eq!(
        decode(
            &serde_json::to_vec(&safe).unwrap(),
            &repository,
            "ghp_canary"
        )
        .unwrap()
        .as_str(),
        sha
    );
    for payload in [
        serde_json::json!({"ref":"refs/heads/other", "object":{"type":"commit", "sha":sha}}),
        serde_json::json!({"ref":"refs/heads/main", "object":{"type":"tag", "sha":sha}}),
        serde_json::json!({"ref":"refs/heads/main", "object":{"type":"commit", "sha":"ghp_canary"}}),
    ] {
        assert_eq!(
            decode(
                &serde_json::to_vec(&payload).unwrap(),
                &repository,
                "ghp_canary"
            ),
            Err(RepositoryError::InvalidProviderResponse)
        );
    }
    assert_eq!(
        decode(&serde_json::to_vec(&safe).unwrap(), &repository, sha),
        Err(RepositoryError::InvalidProviderResponse)
    );
    assert_eq!(
        decode(
            &serde_json::to_vec(&safe).unwrap(),
            &repository,
            &format!("ghp_{}", sha.to_ascii_uppercase())
        ),
        Err(RepositoryError::InvalidProviderResponse)
    );
    assert_eq!(
        decode(b"Bearer ghp_canary", &repository, "ghp_canary"),
        Err(RepositoryError::InvalidProviderResponse)
    );
}
