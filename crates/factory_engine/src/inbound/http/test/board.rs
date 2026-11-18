//! The board: `GET /runs`.

use axum::http::StatusCode;
use factory_records::domain::models::RunOutcome;
use factory_records::domain::ports::RunRepository;
use tower::ServiceExt as _;

use super::{Caller, alice_s_run, body, call, records_router};
use crate::domain::fakes::FakeRepo;

/// The board lists the caller's own Runs, newest first, each with where it
/// is, narrowed by its state, a page at a time; another person's board
/// lists none of them, and a caller signed in as no one is refused.
#[tokio::test]
async fn the_board_lists_the_caller_s_runs_a_page_at_a_time() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let oldest = alice_s_run(&repo).await;
    let ended = alice_s_run(&repo).await;
    repo.end_run(ended, RunOutcome::Failed).await.expect("end");
    let newest = alice_s_run(&repo).await;
    let list = |path: &'static str, caller| {
        let router = router.clone();
        async move {
            router
                .oneshot(call("GET", path, caller, serde_json::Value::Null))
                .await
                .expect("response")
        }
    };
    let id = |run| serde_json::to_value(run).expect("id");

    let all = list("/runs", Caller::User("alice")).await;
    assert_eq!(all.status(), StatusCode::OK);
    let page = body(all).await;
    let listed: Vec<(serde_json::Value, serde_json::Value)> = page["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .map(|listed| {
            (
                listed["detail"]["run"]["id"].clone(),
                listed["state"].clone(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (id(newest), serde_json::json!("running")),
            (id(ended), serde_json::json!("stopped")),
            (id(oldest), serde_json::json!("running")),
        ]
    );
    assert_eq!(page["next"], serde_json::Value::Null);

    let stopped = body(list("/runs?state=stopped", Caller::User("alice")).await).await;
    assert_eq!(stopped["runs"][0]["detail"]["run"]["id"], id(ended));
    assert_eq!(stopped["runs"].as_array().expect("runs").len(), 1);

    let first = body(list("/runs?limit=1", Caller::User("alice")).await).await;
    assert_eq!(first["runs"][0]["detail"]["run"]["id"], id(newest));
    assert_eq!(first["next"]["id"], id(newest));
    let created_at = first["next"]["createdAt"].as_str().expect("time");
    // Its time is in UTC, `Z`, so it reads in a query as it is.
    let after = format!("/runs?limit=1&afterCreatedAt={created_at}&afterId={newest}");
    let second = router
        .clone()
        .oneshot(call(
            "GET",
            &after,
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        body(second).await["runs"][0]["detail"]["run"]["id"],
        id(ended)
    );

    let eve_s = body(list("/runs?scope=team", Caller::User("eve")).await).await;
    assert_eq!(eve_s["runs"], serde_json::json!([]));

    let unknown_state = list("/runs?state=lost", Caller::User("alice")).await;
    assert_eq!(unknown_state.status(), StatusCode::BAD_REQUEST);

    let no_one = list("/runs", Caller::Nobody).await;
    assert_eq!(no_one.status(), StatusCode::UNAUTHORIZED);
}
