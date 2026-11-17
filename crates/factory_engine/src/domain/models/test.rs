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

/// A Step's request carries the Role its entry names, so its agent fills it.
#[test]
fn a_step_request_carries_the_role_its_entry_names() {
    let flow = crate::domain::flow::Flow::parse(
        serde_json::json!({
            "entries": [{ "id": "write", "step": "write", "role": "writer" }]
        }),
        |_| Some(crate::domain::flow::StepUse::Agent),
    )
    .expect("a flow");
    let key = factory_records::domain::models::StepKey {
        run_id: factory_records::domain::models::RunId::generate(),
        seq: 3,
    };

    let request = super::StepRequest::of(key, &flow.entries[0], 2);

    assert_eq!(
        request.role.as_ref().map(|role| role.as_str()),
        Some("writer")
    );
    assert_eq!(request.step.as_str(), "write");
    assert_eq!((request.key, request.visit), (key, 2));
}
