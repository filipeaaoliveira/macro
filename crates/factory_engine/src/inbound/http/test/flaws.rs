//! Marking a Run flawed: its owner, in their own signed-in session, marks a
//! Run that ended `success` with why; anyone else, or any other caller, is
//! refused, and so is a Run that did not succeed.

use axum::Router;
use axum::http::StatusCode;
use factory_records::domain::models::{RunId, RunOutcome};
use factory_records::domain::ports::RunRepository;
use tower::ServiceExt as _;

use super::{ALICE, Caller, alice_s_run, body, call, records_router};
use crate::domain::fakes::FakeRepo;

/// Mark `run` flawed for `reason`, as `caller`.
async fn flaw(
    router: &Router,
    run: RunId,
    caller: Caller<'_>,
    reason: &str,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(call(
            "POST",
            &format!("/runs/{run}/flaw"),
            caller,
            serde_json::json!({ "reason": reason }),
        ))
        .await
        .expect("response")
}

#[tokio::test]
async fn the_owner_marks_a_succeeded_run_flawed_with_why() {
    let repo = FakeRepo::default();
    let run = alice_s_run(&repo).await;
    repo.end_run(run, RunOutcome::Success).await.expect("end");
    let router = records_router(&repo);

    let marked = flaw(
        &router,
        run,
        Caller::User("alice"),
        "  The export drops dates  ",
    )
    .await;

    assert_eq!(marked.status(), StatusCode::OK);
    let end = &body(marked).await["run"]["end"];
    assert_eq!(end["outcome"], "flawed");
    assert_eq!(end["flaw"]["by"]["kind"], "person");
    assert_eq!(end["flaw"]["by"]["userId"], ALICE);
    assert_eq!(end["flaw"]["reason"], "The export drops dates");
}

#[tokio::test]
async fn only_the_owner_in_person_marks_a_run_flawed() {
    let repo = FakeRepo::default();
    let run = alice_s_run(&repo).await;
    repo.end_run(run, RunOutcome::Success).await.expect("end");
    let router = records_router(&repo);

    let eve = flaw(&router, run, Caller::User("eve"), "Wrong").await;
    let key = flaw(&router, run, Caller::ApiKey("alice-key"), "Wrong").await;
    let internal = flaw(&router, run, Caller::Internal(Some(ALICE)), "Wrong").await;

    assert_eq!(eve.status(), StatusCode::NOT_FOUND);
    assert_eq!(key.status(), StatusCode::FORBIDDEN);
    assert_eq!(internal.status(), StatusCode::FORBIDDEN);
    let kept = repo.get_run(run).await.expect("read").expect("kept");
    assert_eq!(kept.end.map(|end| end.outcome), Some(RunOutcome::Success));
}

#[tokio::test]
async fn a_run_that_did_not_succeed_or_a_blank_reason_is_refused() {
    let repo = FakeRepo::default();
    let going = alice_s_run(&repo).await;
    let broke = alice_s_run(&repo).await;
    repo.end_run(broke, RunOutcome::Infrastructure)
        .await
        .expect("end");
    let succeeded = alice_s_run(&repo).await;
    repo.end_run(succeeded, RunOutcome::Success)
        .await
        .expect("end");
    let router = records_router(&repo);

    let still_going = flaw(&router, going, Caller::User("alice"), "Wrong").await;
    let infrastructure = flaw(&router, broke, Caller::User("alice"), "Wrong").await;
    let blank = flaw(&router, succeeded, Caller::User("alice"), "   ").await;

    assert_eq!(still_going.status(), StatusCode::CONFLICT);
    assert_eq!(infrastructure.status(), StatusCode::CONFLICT);
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
}
