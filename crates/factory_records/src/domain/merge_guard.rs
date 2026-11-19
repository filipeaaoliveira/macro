//! Who merges a Run's pull request: while the Run that opened it goes on,
//! only that Run does, through its merge Gate. Merges from elsewhere, such
//! as Macro's GitHub merge route, look the Run up here and refuse.

use std::future::Future;

use crate::domain::models::RunId;

/// The page of the GitHub pull request `number` of `owner/repo`, as a Run
/// records the pull request it opened.
pub fn pull_request_page(owner: &str, repo: &str, number: u64) -> String {
    format!("https://github.com/{owner}/{repo}/pull/{number}")
}

/// Finds the Run that holds a pull request's merge.
pub trait MergeGuardRepository: Send + Sync + 'static {
    /// The error type returned by reads.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The newest Run that opened the pull request at `page` and has not
    /// ended, whoever owns it. A pull request whose Runs all ended, or that
    /// no Run opened, has none. Pages are compared as
    /// [`pull_request_key`](crate::domain::board::pull_request_key) compares
    /// them.
    fn live_run_of_pull_request(
        &self,
        page: &str,
    ) -> impl Future<Output = Result<Option<RunId>, Self::Err>> + Send;
}
