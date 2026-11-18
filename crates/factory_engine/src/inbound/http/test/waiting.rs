//! The Runs waiting on the caller: `GET /runs/waiting`.

use axum::http::StatusCode;
use factory_records::domain::models::{EntryId, Outcome, RunOutcome, StepKey};
use factory_records::domain::ports::RunRepository;
use tower::ServiceExt as _;

use super::{Caller, alice_s_run, body, call, records_router};
use crate::domain::fakes::FakeRepo;

/// A Run waits on Alice while its Gate is open: her Runs at an open Gate
/// are listed, newest first, and a Run that ended or whose Gate was
/// answered is not; the path is not read as a Run's id.
#[tokio::test]
async fn the_caller_s_runs_waiting_at_a_gate_are_listed() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let answers = [Outcome::try_from("approve".to_string()).expect("valid")];
    let plan = EntryId::try_from("plan".to_string()).expect("entry");

    let mut runs = Vec::new();
    for _ in 0..4 {
        let run_id = alice_s_run(&repo).await;
        let key = StepKey { run_id, seq: 1 };
        repo.start_step(key, &plan, 1).await.expect("start");
        repo.open_gate(key, "Approve the plan?", &answers)
            .await
            .expect("open");
        runs.push(run_id);
    }
    let alice = repo
        .get_run(runs[2])
        .await
        .expect("get")
        .expect("kept")
        .user_id;
    repo.answer_gate(
        StepKey {
            run_id: runs[2],
            seq: 1,
        },
        &answers[0],
        None,
        &alice,
        None,
    )
    .await
    .expect("answer");
    repo.end_run(runs[3], RunOutcome::Cancelled)
        .await
        .expect("cancel");
    let list = |caller| {
        let router = router.clone();
        async move {
            router
                .oneshot(call(
                    "GET",
                    "/runs/waiting",
                    caller,
                    serde_json::Value::Null,
                ))
                .await
                .expect("response")
        }
    };

    let alice_s = list(Caller::User("alice")).await;
    assert_eq!(alice_s.status(), StatusCode::OK);
    let listed = body(alice_s).await["runs"].clone();
    let ids: Vec<serde_json::Value> = listed
        .as_array()
        .expect("runs")
        .iter()
        .map(|detail| detail["run"]["id"].clone())
        .collect();
    let expected: Vec<serde_json::Value> = [runs[1], runs[0]]
        .iter()
        .map(|id| serde_json::to_value(id).expect("id"))
        .collect();
    assert_eq!(ids, expected);
    assert_eq!(
        listed[0]["steps"][0]["gate"]["question"],
        "Approve the plan?"
    );

    let eve_s = list(Caller::User("eve")).await;
    assert_eq!(eve_s.status(), StatusCode::OK);
    assert_eq!(body(eve_s).await["runs"], serde_json::json!([]));

    let nobody = list(Caller::Nobody).await;
    assert_eq!(nobody.status(), StatusCode::UNAUTHORIZED);
}
