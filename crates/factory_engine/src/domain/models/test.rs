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

/// A Step request with a field the worker does not know is refused rather
/// than run without it; one with the fields Runs have always sent reads.
#[test]
fn a_step_request_refuses_a_field_it_does_not_know() {
    let sent = serde_json::json!({
        "key": { "run_id": "0195f7a0-0000-7000-8000-000000000001", "seq": 3 },
        "entry": "review",
        "step": "review",
        "role": "reviewer",
        "visit": 1
    });
    let mut newer = sent.clone();
    newer["panel"] = serde_json::json!(true);

    assert!(serde_json::from_value::<super::StepRequest>(sent).is_ok());
    assert!(serde_json::from_value::<super::StepRequest>(newer).is_err());
}

/// Only a commit GitHub signed as itself is GitHub's: its account named as
/// committer without a verified signature, as a commit made by hand with
/// GitHub's email, is not, nor one someone else signed.
#[test]
fn only_a_commit_github_signed_as_itself_is_github_s() {
    let commit = |login: Option<&str>, verified| super::CommitMade {
        parents: vec![],
        committer_login: login.map(str::to_string),
        verified,
    };
    assert!(commit(Some("web-flow"), true).made_by_github());
    assert!(!commit(Some("web-flow"), false).made_by_github());
    assert!(!commit(Some("julia"), true).made_by_github());
    assert!(!commit(None, true).made_by_github());
}
