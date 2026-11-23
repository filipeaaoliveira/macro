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

/// What a worker from before retries read of a Run's start: its fields
/// then, with `flow` required.
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct StartBeforeRetries {
    run_id: factory_records::domain::models::RunId,
    flow: crate::domain::flow::Flow,
    #[serde(default)]
    token_budget: bool,
}

/// A retry's start writes its Flow as `retry_flow`, so a worker from before
/// retries cannot read it and leaves it to one that can, rather than walk
/// the retry from its first entry; a new worker reads it whole. Any other
/// start writes `flow`, as before, and both workers read it.
#[test]
fn a_worker_from_before_retries_cannot_read_a_retry_s_start() {
    let flow = crate::domain::flow::Flow::parse(
        serde_json::json!({ "entries": [{ "id": "reply", "step": "reply" }] }),
        |_| Some(crate::domain::flow::StepUse::Work),
    )
    .expect("a flow");
    let plain = super::RunStart {
        run_id: factory_records::domain::models::RunId::generate(),
        flow,
        token_budget: true,
        start_at: None,
    };
    let retry = super::RunStart {
        start_at: Some(super::StartAt {
            entry: factory_records::domain::models::EntryId::try_from("reply".to_string())
                .expect("valid"),
            first_seq: 3,
        }),
        ..plain.clone()
    };

    let plain_json = serde_json::to_value(&plain).expect("serializes");
    let retry_json = serde_json::to_value(&retry).expect("serializes");

    assert!(plain_json.get("flow").is_some() && plain_json.get("start_at").is_none());
    assert!(serde_json::from_value::<StartBeforeRetries>(plain_json.clone()).is_ok());
    assert!(serde_json::from_value::<StartBeforeRetries>(retry_json.clone()).is_err());
    assert_eq!(
        serde_json::from_value::<super::RunStart>(plain_json).expect("reads"),
        plain
    );
    assert_eq!(
        serde_json::from_value::<super::RunStart>(retry_json).expect("reads"),
        retry
    );
}
