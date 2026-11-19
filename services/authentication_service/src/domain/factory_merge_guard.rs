//! Macro's GitHub merge route refuses to merge a pull request while the
//! factory Run that opened it goes on: that Run merges it, through its merge
//! Gate, and its Standing Rules and Main Holds assume no one merges under it.
//! The Run merges on GitHub itself, with its owner's token, so the refusal
//! never stands in its way. A pull request whose Runs all ended, or that no
//! Run opened, merges as before.

#[cfg(test)]
mod test;

use std::future::Future;

use factory_records::domain::merge_guard::{MergeGuardRepository, pull_request_page};
use factory_records::domain::models::RunId;
use github::domain::models::{GithubError, GithubPullRequestNumber, MergeGithubPullRequestRequest};
use macro_user_id::{lowercased::Lowercase, user_id::MacroUserId};

/// Whether a person can see a pull request on GitHub.
pub trait PullRequestSight: Send + Sync + 'static {
    /// Whether `user` sees `pull_request`, with their own GitHub grant.
    fn sees(
        &self,
        user: &MacroUserId<Lowercase<'static>>,
        pull_request: &GithubPullRequestNumber,
    ) -> impl Future<Output = Result<bool, GithubError>> + Send;
}

/// Why a merge was refused before it reached GitHub.
#[derive(Debug, thiserror::Error)]
pub enum FactoryMergeRefusal {
    /// A factory Run that goes on opened the pull request.
    #[error("factory Run {0} has not ended")]
    LiveRun(RunId),
    /// A factory Run that goes on opened the pull request, and the caller
    /// cannot see it on GitHub.
    #[error("the pull request is not visible to the caller")]
    NotVisible,
    /// The Runs could not be read.
    #[error("could not read the factory Runs of the pull request")]
    Lookup(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// GitHub could not say whether the caller sees the pull request.
    #[error(transparent)]
    Github(GithubError),
}

/// Refuses merges under a factory Run that goes on.
pub struct FactoryMergeGuard<R, S> {
    runs: R,
    sight: S,
}

impl<R: MergeGuardRepository, S: PullRequestSight> FactoryMergeGuard<R, S> {
    /// A guard that reads Runs from `runs` and asks `sight` what a caller
    /// sees on GitHub.
    pub fn new(runs: R, sight: S) -> Self {
        Self { runs, sight }
    }

    /// Whether `user` may merge the pull request `request` names, as far as
    /// the factory is concerned; GitHub still decides the rest.
    ///
    /// Runs that cannot be read refuse the merge: they live in the database
    /// this route already reads the caller's GitHub grant from, so a merge
    /// that goes on without them would rarely get further, and one that did
    /// could merge under a Run.
    ///
    /// The Run is named only to a caller who sees the pull request on
    /// GitHub, who already sees its id in the branch name `factory/<id>`.
    /// Anyone else is answered as GitHub answers a pull request it hides,
    /// so they learn nothing of the Run.
    #[tracing::instrument(skip(self), err)]
    pub async fn check(
        &self,
        user: &MacroUserId<Lowercase<'static>>,
        request: &MergeGithubPullRequestRequest,
    ) -> Result<(), FactoryMergeRefusal> {
        let page = pull_request_page(&request.owner, &request.repo, request.number);
        let run = self
            .runs
            .live_run_of_pull_request(&page)
            .await
            .map_err(|error| FactoryMergeRefusal::Lookup(Box::new(error)))?;
        let Some(run) = run else {
            return Ok(());
        };

        let pull_request = GithubPullRequestNumber {
            owner: request.owner.clone(),
            repo: request.repo.clone(),
            number: request.number,
        };
        let sees = self
            .sight
            .sees(user, &pull_request)
            .await
            .map_err(FactoryMergeRefusal::Github)?;
        if sees {
            Err(FactoryMergeRefusal::LiveRun(run))
        } else {
            Err(FactoryMergeRefusal::NotVisible)
        }
    }
}
