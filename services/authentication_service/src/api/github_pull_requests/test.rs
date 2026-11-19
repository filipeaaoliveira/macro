use axum::{http::StatusCode, response::IntoResponse};
use factory_records::domain::models::RunId;
use github::domain::models::GithubError;

use super::MergeGithubPullRequestError;
use crate::domain::factory_merge_guard::FactoryMergeRefusal;

async fn answer(error: MergeGithubPullRequestError) -> (StatusCode, serde_json::Value) {
    let response = error.into_response();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).expect("json"))
}

/// A merge under a Run that has not ended is a conflict that names the Run
/// and where to merge it.
#[tokio::test]
async fn a_live_run_s_refusal_names_the_run_and_its_merge_gate() {
    let run = RunId::generate();

    let (status, body) = answer(FactoryMergeRefusal::LiveRun(run).into()).await;

    assert_eq!(status, StatusCode::CONFLICT);
    let message = body["message"].as_str().expect("message");
    assert!(message.contains(&run.to_string()), "{message}");
    assert!(message.contains("merge Gate"), "{message}");
}

/// A caller GitHub hides the pull request from is answered as GitHub
/// answers them, without the Run.
#[tokio::test]
async fn a_hidden_pull_request_s_refusal_reads_as_github_s() {
    let (status, body) = answer(FactoryMergeRefusal::NotVisible.into()).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "Not Found");
}

/// GitHub failing to say what the caller sees is answered as the merge's
/// own GitHub failures are.
#[tokio::test]
async fn a_failed_sight_reads_as_the_merge_s_github_failure() {
    let (status, body) = answer(FactoryMergeRefusal::Github(GithubError::NoLinkFound).into()).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "no github link found");
}
