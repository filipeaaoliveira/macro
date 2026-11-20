use std::sync::Mutex;

use factory_records::domain::merge_guard::MergeGuardRepository;
use factory_records::domain::models::RunId;
use github::domain::models::{GithubError, GithubPullRequestNumber, MergeGithubPullRequestRequest};
use macro_user_id::cowlike::CowLike;
use macro_user_id::{lowercased::Lowercase, user_id::MacroUserId};

use super::{FactoryMergeGuard, FactoryMergeRefusal, PullRequestSight};

#[derive(Debug, thiserror::Error)]
#[error("database unavailable")]
struct Unavailable;

/// The live Run of each page asked, or a failed read; keeps the pages asked.
struct Runs {
    live: Result<Option<RunId>, ()>,
    asked: Mutex<Vec<String>>,
}

impl Runs {
    fn answering(live: Result<Option<RunId>, ()>) -> Self {
        Self {
            live,
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl MergeGuardRepository for Runs {
    type Err = Unavailable;

    async fn live_run_of_pull_request(&self, page: &str) -> Result<Option<RunId>, Self::Err> {
        self.asked.lock().expect("lock").push(page.to_string());
        self.live.map_err(|()| Unavailable)
    }
}

/// Whether the caller sees each pull request; keeps the pull requests asked.
struct Sight {
    sees: bool,
    asked: Mutex<Vec<GithubPullRequestNumber>>,
}

impl Sight {
    fn seeing(sees: bool) -> Self {
        Self {
            sees,
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl PullRequestSight for Sight {
    async fn sees(
        &self,
        _user: &MacroUserId<Lowercase<'static>>,
        pull_request: &GithubPullRequestNumber,
    ) -> Result<bool, GithubError> {
        self.asked.lock().expect("lock").push(pull_request.clone());
        Ok(self.sees)
    }
}

fn caller() -> MacroUserId<Lowercase<'static>> {
    MacroUserId::parse_from_str("macro|teo@example.com")
        .expect("valid")
        .into_owned()
        .lowercase()
}

fn request() -> MergeGithubPullRequestRequest {
    MergeGithubPullRequestRequest {
        owner: "Acme".to_string(),
        repo: "shop".to_string(),
        number: 7,
        merge_method: None,
    }
}

/// A pull request no Run that goes on opened merges as before, and GitHub
/// is not asked anything more.
#[tokio::test]
async fn a_pull_request_with_no_live_run_merges() {
    let guard = FactoryMergeGuard::new(Runs::answering(Ok(None)), Sight::seeing(true));

    let checked = guard.check(&caller(), &request()).await;

    assert!(checked.is_ok(), "{checked:?}");
    assert_eq!(
        *guard.runs.asked.lock().expect("lock"),
        vec!["https://github.com/Acme/shop/pull/7".to_string()]
    );
    assert!(guard.sight.asked.lock().expect("lock").is_empty());
}

/// A pull request a Run that goes on opened is refused, naming the Run, to
/// a caller who sees it on GitHub.
#[tokio::test]
async fn a_live_run_s_pull_request_is_refused_naming_the_run() {
    let run = RunId::generate();
    let guard = FactoryMergeGuard::new(Runs::answering(Ok(Some(run))), Sight::seeing(true));

    let checked = guard.check(&caller(), &request()).await;

    let Err(FactoryMergeRefusal::LiveRun(named)) = checked else {
        panic!("the merge went on: {checked:?}");
    };
    assert_eq!(named, run);
    assert_eq!(
        *guard.sight.asked.lock().expect("lock"),
        vec![GithubPullRequestNumber {
            owner: "Acme".to_string(),
            repo: "shop".to_string(),
            number: 7,
        }]
    );
}

/// A caller GitHub hides the pull request from is refused without the Run.
#[tokio::test]
async fn a_hidden_pull_request_is_refused_without_its_run() {
    let guard = FactoryMergeGuard::new(
        Runs::answering(Ok(Some(RunId::generate()))),
        Sight::seeing(false),
    );

    let checked = guard.check(&caller(), &request()).await;

    assert!(
        matches!(checked, Err(FactoryMergeRefusal::NotVisible)),
        "{checked:?}"
    );
}

/// Runs that cannot be read refuse the merge.
#[tokio::test]
async fn unread_runs_refuse_the_merge() {
    let guard = FactoryMergeGuard::new(Runs::answering(Err(())), Sight::seeing(true));

    let checked = guard.check(&caller(), &request()).await;

    assert!(
        matches!(checked, Err(FactoryMergeRefusal::Lookup(_))),
        "{checked:?}"
    );
}
