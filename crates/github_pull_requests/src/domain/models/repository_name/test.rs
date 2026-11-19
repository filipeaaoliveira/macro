use super::{is_github_owner, is_github_repository_name};

/// Account and repository names GitHub gives are accepted.
#[test]
fn github_names_are_accepted() {
    assert!(is_github_owner("acme"));
    assert!(is_github_owner("Acme-Labs2"));
    assert!(is_github_repository_name("widgets"));
    assert!(is_github_repository_name("my_widgets.v2-beta"));
    assert!(is_github_repository_name(".github"));
}

/// Parts that would point a GitHub API path at another repository or pull
/// request are refused.
#[test]
fn crafted_parts_are_refused() {
    for owner in [
        "",
        "acme/widgets/pulls/7/merge#",
        "acme/widgets",
        "..",
        "ac%6De",
        "acme?x=",
        "acme.labs",
        "acme_labs",
        "a".repeat(40).as_str(),
    ] {
        assert!(!is_github_owner(owner), "{owner:?}");
    }
    for repo in [
        "",
        ".",
        "..",
        "widgets/pulls/7/merge?x=",
        "x/../widgets",
        "widget%73",
        "widgets#",
        "widgets?x=1",
        "widgets.git",
        "widgets.GIT",
        "wid gets",
        "r".repeat(101).as_str(),
    ] {
        assert!(!is_github_repository_name(repo), "{repo:?}");
    }
}
