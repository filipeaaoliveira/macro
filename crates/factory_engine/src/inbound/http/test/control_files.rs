//! The Control File settings routes, and answering a Tampered Run's Gate.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::models::Tampering;
use factory_records::domain::ports::RunRepository;
use macro_authorization::CODE_EXECUTION_HEADER;
use tower::ServiceExt as _;

use super::{ALICE, Caller, alice_s_run, body, call, records_router};
use crate::domain::control_files::ControlFiles;
use crate::domain::fakes::FakeRepo;

const PATH: &str = "/settings/control-files";

/// Only the person in their own signed-in session sets their patterns: a
/// shorter list lets an agent change more files unseen. An internal caller
/// acting for them, their API key or token, and an agent working as them
/// are refused, and nothing is kept.
#[tokio::test]
async fn only_the_person_s_own_session_sets_their_control_files() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let patterns = serde_json::json!({ "patterns": ["ci/**"] });
    let send = |request: Request<Body>| {
        let router = router.clone();
        async move { router.oneshot(request).await.expect("response").status() }
    };

    let internal = send(call(
        "PUT",
        PATH,
        Caller::Internal(Some(ALICE)),
        patterns.clone(),
    ))
    .await;
    let api_key = send(call(
        "PUT",
        PATH,
        Caller::ApiKey("alice-key"),
        patterns.clone(),
    ))
    .await;
    let sdk = send(call("PUT", PATH, Caller::User("sdk"), patterns.clone())).await;
    let agent = send(call("PUT", PATH, Caller::User("agent"), patterns.clone())).await;
    let mut in_code_mode = call("PUT", PATH, Caller::User("alice"), patterns.clone());
    in_code_mode
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    let in_code_mode = send(in_code_mode).await;

    for refused in [internal, api_key, sdk, agent, in_code_mode] {
        assert_eq!(refused, StatusCode::FORBIDDEN);
    }
    assert!(repo.control_files.lock().expect("patterns").is_empty());

    let person = router
        .clone()
        .oneshot(call("PUT", PATH, Caller::User("alice"), patterns))
        .await
        .expect("response");
    assert_eq!(person.status(), StatusCode::OK);
    assert_eq!(
        body(person).await,
        serde_json::json!({ "patterns": ["ci/**"], "isDefault": false })
    );
}

/// The patterns read as the caller's own, or the default, by any caller
/// acting for them.
#[tokio::test]
async fn the_control_files_read_as_the_caller_s_own_or_the_default() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);

    let default = router
        .clone()
        .oneshot(call(
            "GET",
            PATH,
            Caller::Internal(Some(ALICE)),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(default.status(), StatusCode::OK);
    assert_eq!(
        body(default).await,
        serde_json::json!({
            "patterns": ControlFiles::default_patterns(),
            "isDefault": true,
        })
    );

    let invalid = router
        .oneshot(call(
            "PUT",
            PATH,
            Caller::User("alice"),
            serde_json::json!({ "patterns": ["a**b"] }),
        ))
        .await
        .expect("response");
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

/// A Tampered Run shows its files; approving its Gate without approving
/// them is refused with a code a client acts on, and with them is taken.
#[tokio::test]
async fn approving_a_tampered_run_needs_approve_control_files() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let run_id = alice_s_run(&repo).await;
    repo.set_tampering(
        run_id,
        Some(&Tampering {
            files: vec!["package.json".to_string()],
            unlisted: false,
        }),
    )
    .await
    .expect("keep");
    let answer = format!("/runs/{run_id}/steps/4/answer");

    let read = router
        .clone()
        .oneshot(call(
            "GET",
            &format!("/runs/{run_id}"),
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(
        body(read).await["run"]["tampering"],
        serde_json::json!({ "files": ["package.json"], "unlisted": false })
    );

    let refused = router
        .clone()
        .oneshot(call(
            "POST",
            &answer,
            Caller::User("alice"),
            serde_json::json!({ "answer": "approve" }),
        ))
        .await
        .expect("response");
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(refused).await["code"], "controlFilesNotApproved");

    let approved = router
        .oneshot(call(
            "POST",
            &answer,
            Caller::User("alice"),
            serde_json::json!({ "answer": "approve", "approveControlFiles": true }),
        ))
        .await
        .expect("response");
    assert_eq!(approved.status(), StatusCode::ACCEPTED);
}
