use super::*;

#[test]
fn only_canonical_github_https_repositories_and_configured_branches_are_accepted() {
    let repository = GithubRepository::parse(
        "https://github.com/acme/widget.git",
        "refs/heads/feature/review",
    )
    .unwrap();
    assert_eq!(
        repository.api_url().as_str(),
        "https://api.github.com/repos/acme/widget/git/ref/heads/feature/review"
    );
    assert_eq!(repository.expected_ref(), "refs/heads/feature/review");
    for upstream in [
        "http://github.com/acme/widget",
        "https://evil.test/acme/widget",
        "https://github.com:444/acme/widget",
        "https://token@github.com/acme/widget",
        "https://github.com/acme/widget?token=x",
        "https://github.com/acme/widget#x",
        "https://github.com/acme/../widget",
        "https://github.com/acme/%77idget",
        "https://github.com/acme/widget/",
        "https://github.com/acme/widget/more",
    ] {
        assert!(
            GithubRepository::parse(upstream, "main").is_err(),
            "{upstream}"
        );
    }
    for branch in [
        "HEAD",
        "refs/tags/latest",
        "refs/heads/",
        "../main",
        "a//b",
        "a.lock",
        "-main",
        "main~1",
        "main@{1}",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        assert!(
            GithubRepository::parse("https://github.com/acme/widget", branch).is_err(),
            "{branch}"
        );
    }
}
