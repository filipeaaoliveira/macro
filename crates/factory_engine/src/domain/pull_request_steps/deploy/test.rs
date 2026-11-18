use std::sync::LazyLock;

use factory_records::domain::models::{Repository, RunId, StepKey};
use macro_user_id::user_id::MacroUserIdStr;

use super::super::{PullRequestStepError, PullRequestSteps};
use super::DEPLOY_WAIT;
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches, ci};
use crate::domain::models::{
    Deployment, DeploymentState, PullRequestState, StepInput, StepReport,
};
use crate::domain::verification::VERIFY_FILE;

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

fn input() -> StepInput<'static> {
    StepInput {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 9,
        },
        role: None,
        owner: &JULIA,
        request: "Collapse repeated dashes in slugs",
        created_in_person: true,
        control_file_patterns: None,
        task_id: None,
        repository: Some(&REPOSITORY),
        earlier: &[],
        review: None,
        reviewer: false,
        may_ask: false,
        questions_left: 0,
        resumes: None,
        owner_notes: &[],
    }
}

/// GitHub with the Run's pull request merged, its base declaring `verify`
/// as its verify file before the merge (none when absent).
fn merged(verify: Option<&str>) -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    {
        let mut state = github.state();
        let pull_request = state.pull_request.as_mut().expect("opened");
        pull_request.state = PullRequestState::Merged;
        pull_request.merge_commit = Some(FakeGithub::MERGED.to_string());
        if let Some(verify) = verify {
            state.files.insert(
                (FakeGithub::BASE.to_string(), VERIFY_FILE.to_string()),
                verify.to_string(),
            );
        }
    }
    github
}

/// A deployment of `commit` that went as `state`.
fn deployment_of(commit: &str, state: DeploymentState) -> Deployment {
    Deployment {
        commit: commit.to_string(),
        state,
        url: Some("https://app.example.com".to_string()),
    }
}

/// The merge commit's deployment, gone as `state`.
fn deployment(state: DeploymentState) -> Vec<Deployment> {
    vec![deployment_of(FakeGithub::MERGED, state)]
}

async fn deploy(github: &FakeGithub) -> Result<StepReport, PullRequestStepError> {
    PullRequestSteps::new(github.clone(), FakeRepo::default(), FakeWatches::default())
        .deploy(input())
        .await
}

const PRODUCTION: &str = r#"{"deploy": {"environment": "production"}}"#;

/// A repository that declares no deploy has nothing to wait for: the merged
/// commit is verified as it is.
#[tokio::test]
async fn no_deploy_declared_is_not_needed() {
    for verify in [None, Some("{}")] {
        let github = merged(verify);

        let report = deploy(&github).await.expect("deploy");

        assert_eq!(report.outcome.as_str(), "not_needed");
        assert!(report.output.contains(FakeGithub::MERGED), "{}", report.output);
        assert!(github.state().deployment_reads.is_empty());
    }
}

/// The Run waits while the merge commit's deployment to the declared
/// environment has not succeeded, and goes on once it has.
#[tokio::test(start_paused = true)]
async fn the_run_waits_for_the_merge_commit_s_deployment() {
    let github = merged(Some(PRODUCTION));
    // Each wait reads the commit's deployments, then the environment's.
    github.state().deployments = [
        Vec::new(),
        Vec::new(),
        deployment(DeploymentState::Waiting),
        deployment(DeploymentState::Waiting),
        deployment(DeploymentState::Succeeded),
    ]
    .into();

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "deployed");
    assert!(
        report.output.contains("deployed to production at https://app.example.com"),
        "{}",
        report.output
    );
    let reads = github.state().deployment_reads.clone();
    assert_eq!(reads.len(), 5);
    assert_eq!(
        reads[4],
        (FakeGithub::MERGED.to_string(), "production".to_string())
    );
    assert_eq!(reads[1], ("any".to_string(), "production".to_string()));
}

/// A deployment replaced by a later one did deploy the change.
#[tokio::test]
async fn a_replaced_deployment_was_deployed() {
    let github = merged(Some(PRODUCTION));
    github.state().deployments = [deployment(DeploymentState::Replaced)].into();

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "deployed");
}

/// A later commit's deployment that succeeded deployed the merge with it
/// when that commit contains the merge, so a cancelled or failed
/// deployment of the merge itself is no failure then; one of a commit
/// beside it deployed nothing of it.
#[tokio::test(start_paused = true)]
async fn a_later_commit_s_deployment_deploys_the_merge() {
    const LATER: &str = "4444444444444444444444444444444444444444";
    const BESIDE: &str = "5555555555555555555555555555555555555555";
    let github = merged(Some(PRODUCTION));
    github.state().deployments = [vec![
        deployment_of(BESIDE, DeploymentState::Succeeded),
        deployment_of(LATER, DeploymentState::Succeeded),
        deployment_of(FakeGithub::MERGED, DeploymentState::Failed),
    ]]
    .into();
    github
        .state()
        .descendants
        .insert((FakeGithub::MERGED.to_string(), LATER.to_string()));

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "deployed", "{}", report.output);
    assert!(report.output.contains(LATER), "{}", report.output);

    // Without it, the failed deployment of the merge is a failure.
    github.state().descendants.clear();
    let report = deploy(&github).await.expect("deploy");
    assert_eq!(report.outcome.as_str(), "deploy_failed");
}

#[tokio::test]
async fn a_failed_deployment_ends_the_wait() {
    let github = merged(Some(PRODUCTION));
    github.state().deployments = [deployment(DeploymentState::Failed)].into();

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "deploy_failed");
}

/// A deployment that never succeeds ends the wait with `timeout`, within a
/// Step attempt.
#[tokio::test(start_paused = true)]
async fn a_deployment_that_never_comes_times_out() {
    let github = merged(Some(PRODUCTION));
    let started = tokio::time::Instant::now();

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "timeout");
    assert!(started.elapsed() <= DEPLOY_WAIT);
}

/// A repository that turns the verify phase off ends the Run once merged,
/// and one whose file does not read says why.
#[tokio::test]
async fn the_phase_turned_off_or_unreadable() {
    let off = merged(Some(
        r#"{"verify": false, "deploy": {"environment": "production"}}"#,
    ));
    let report = deploy(&off).await.expect("deploy");
    assert_eq!(report.outcome.as_str(), "off");
    assert!(off.state().deployment_reads.is_empty());

    let unreadable = merged(Some(r#"{"verify": "yes"}"#));
    let report = deploy(&unreadable).await.expect("deploy");
    assert_eq!(report.outcome.as_str(), "unreadable");
    assert!(report.output.contains(VERIFY_FILE), "{}", report.output);
}

/// The verify file is read as the base had it before the merge: a Run whose
/// own change turns the phase off, now on the base branch, is still
/// verified.
#[tokio::test]
async fn a_run_cannot_turn_its_own_verification_off() {
    let github = merged(None);
    github.state().files.insert(
        ("main".to_string(), VERIFY_FILE.to_string()),
        r#"{"verify": false}"#.to_string(),
    );

    let report = deploy(&github).await.expect("deploy");

    assert_eq!(report.outcome.as_str(), "not_needed", "{}", report.output);
}

/// A pull request GitHub lists merged without its merge commit yet is
/// tried again; one not merged is not.
#[tokio::test]
async fn the_merge_commit_must_be_known() {
    let github = merged(Some(PRODUCTION));
    github
        .state()
        .pull_request
        .as_mut()
        .expect("opened")
        .merge_commit = None;
    let lagging = deploy(&github).await;
    assert!(matches!(lagging, Err(PullRequestStepError::NoMergeCommit)));

    github.state().pull_request.as_mut().expect("opened").state = PullRequestState::Open;
    let open = deploy(&github).await;
    assert!(matches!(open, Err(PullRequestStepError::NotMerged)));
}
