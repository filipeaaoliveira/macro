use super::CodeRepository;

/// Only the canonical `https://github.com/owner/name` names a repository:
/// its two segments go into GitHub's API paths and a session's repository.
#[test]
fn a_repository_is_named_only_by_its_canonical_url() {
    let named = CodeRepository::parse("https://github.com/macro-inc/app.web_2").expect("valid");
    assert_eq!(named.owner(), "macro-inc");
    assert_eq!(named.name(), "app.web_2");
    assert_eq!(named.url(), "https://github.com/macro-inc/app.web_2");

    for refused in [
        "http://github.com/macro/app",
        "https://github.com.evil.example/macro/app",
        "https://github.com/macro/..",
        "https://github.com/../app",
        "https://github.com/macro/app?tab=readme",
        "https://github.com/macro/app/",
        "https://@github.com/macro/app",
        "https://github.com/macro@evil/app",
        "https://github.com/macro/app.git",
        "https://github.com/macro/app/tree/main",
        "https://github.com/macro",
        "https://github.com/macro/a%2Fb",
    ] {
        assert_eq!(CodeRepository::parse(refused), None, "{refused}");
    }
}
