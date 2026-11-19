use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{http::StatusCode, response::IntoResponse};
use factory_records::domain::merge_guard::MergeGuardRepository;
use factory_records::domain::models::RunId;
use github::domain::models::{
    GithubError, GithubPullRequestNumber, MergeGithubPullRequestRequest,
    MergeGithubPullRequestResponse,
};
use macro_user_id::cowlike::CowLike;
use macro_user_id::{lowercased::Lowercase, user_id::MacroUserId};

use super::{MergeGithubPullRequestError, merge_unless_held};
use crate::domain::factory_merge_guard::{
    FactoryMergeGuard, FactoryMergeRefusal, PullRequestSight,
};

/// The Run that has not ended on every pull request, if any.
struct Runs {
    live: Option<RunId>,
}

impl MergeGuardRepository for Runs {
    type Err = std::io::Error;

    async fn live_run_of_pull_request(&self, _page: &str) -> Result<Option<RunId>, Self::Err> {
        Ok(self.live)
    }
}

/// A caller who sees every pull request on GitHub.
struct SeesAll;

impl PullRequestSight for SeesAll {
    async fn sees(
        &self,
        _user: &MacroUserId<Lowercase<'static>>,
        _pull_request: &GithubPullRequestNumber,
    ) -> Result<bool, GithubError> {
        Ok(true)
    }
}

fn guard(live: Option<RunId>) -> FactoryMergeGuard<Runs, SeesAll> {
    FactoryMergeGuard::new(Runs { live }, SeesAll)
}

fn caller() -> MacroUserId<Lowercase<'static>> {
    MacroUserId::parse_from_str("macro|teo@example.com")
        .expect("valid")
        .into_owned()
        .lowercase()
}

fn request(owner: &str, repo: &str) -> MergeGithubPullRequestRequest {
    MergeGithubPullRequestRequest {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number: 7,
        merge_method: None,
    }
}

/// Merges as GitHub would, counting the merges asked for.
async fn merged(
    merges: &AtomicUsize,
    _request: MergeGithubPullRequestRequest,
) -> Result<MergeGithubPullRequestResponse, GithubError> {
    merges.fetch_add(1, Ordering::SeqCst);
    Ok(MergeGithubPullRequestResponse {
        sha: "6dcb09b5".to_string(),
        message: "Pull Request successfully merged".to_string(),
        pull_request: None,
    })
}

/// The route merges a pull request no Run that goes on opened.
#[tokio::test]
async fn the_route_merges_a_pull_request_with_no_live_run() {
    let merges = AtomicUsize::new(0);

    let merging = merge_unless_held(
        &guard(None),
        &caller(),
        request("acme", "shop"),
        |request| merged(&merges, request),
    )
    .await;

    assert!(merging.is_ok(), "{merging:?}");
    assert_eq!(merges.load(Ordering::SeqCst), 1);
}

/// The route refuses a live Run's pull request before it asks GitHub to
/// merge.
#[tokio::test]
async fn the_route_refuses_a_live_run_s_pull_request_before_merging() {
    let run = RunId::generate();
    let merges = AtomicUsize::new(0);

    let merging = merge_unless_held(
        &guard(Some(run)),
        &caller(),
        request("acme", "shop"),
        |request| merged(&merges, request),
    )
    .await;

    assert!(
        matches!(
            merging,
            Err(MergeGithubPullRequestError::Factory(FactoryMergeRefusal::LiveRun(named))) if named == run
        ),
        "{merging:?}"
    );
    assert_eq!(merges.load(Ordering::SeqCst), 0);
}

/// An owner or repository GitHub would read as another path is refused
/// before GitHub is asked: the guard could not find the Run of the pull
/// request GitHub would merge.
#[tokio::test]
async fn the_route_refuses_a_crafted_repository_before_anything() {
    for (owner, repo) in [
        ("acme", "widgets/pulls/7/merge?x="),
        ("acme", "x/../widgets"),
        ("acme/widgets/pulls/7/merge#", "shop"),
        ("acme", "widget%73"),
        ("acme", "widgets.git"),
    ] {
        let merges = AtomicUsize::new(0);

        let merging = merge_unless_held(&guard(None), &caller(), request(owner, repo), |request| {
            merged(&merges, request)
        })
        .await;
        let (status, _) = answer(merging.expect_err("refused")).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{owner}/{repo}");
        assert_eq!(merges.load(Ordering::SeqCst), 0);
    }
}

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
    assert!(message.contains("cancel the Run"), "{message}");
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
