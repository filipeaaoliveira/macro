//! What GitHub says of the checks on a commit, and the checks a branch
//! requires.

#[cfg(test)]
mod test;

/// A check suite on a commit: one per app that checks it, such as a CI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCheckSuite {
    /// GitHub's id for it.
    pub id: u64,
    /// The app that runs it, by its slug.
    pub app: String,
    /// GitHub's status: `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How it concluded, once completed.
    pub conclusion: Option<String>,
    /// How many check runs it holds; an app that runs nothing on the commit
    /// leaves an empty suite queued.
    pub check_runs: u64,
    /// When GitHub created it, as an app's CI may add its runs a while
    /// after.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One page or more of a list GitHub pages, and whether it was read to
/// its end: a list longer than a reader reads is not all there is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubListing<T> {
    /// What was read.
    pub items: Vec<T>,
    /// Whether that is the whole list.
    pub complete: bool,
}

/// A check run on a commit, with the app that runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCheckRun {
    /// Its name.
    pub name: String,
    /// GitHub's status: `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How it concluded, once completed.
    pub conclusion: Option<String>,
    /// Its page.
    pub html_url: Option<String>,
    /// GitHub's id of the app that runs it.
    pub app_id: Option<u64>,
}

/// A check a branch requires to pass before a pull request to it merges:
/// by name, and from one app when the rule names one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GithubRequiredCheck {
    /// The check run's name, or the commit status's context.
    pub context: String,
    /// The app whose check run it must be; none when any app's run, or a
    /// commit status, will do.
    pub app_id: Option<u64>,
}

/// The checks a branch requires, as far as they could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRequiredChecks {
    /// The checks the readable sources name.
    pub checks: Vec<GithubRequiredCheck>,
    /// Whether every source was read in full: the branch's protection and
    /// all of its rules. A partial set names checks that are required, but
    /// may leave others out.
    pub complete: bool,
}

/// A GitHub Actions workflow run on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubWorkflowRun {
    /// Where it stands.
    pub status: GithubWorkflowRunStatus,
}

/// Where a GitHub Actions workflow run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubWorkflowRunStatus {
    /// Finished.
    Completed,
    /// Not finished: queued, pending, waiting, requested, in progress, or a
    /// status GitHub added that this does not know, which is not taken as
    /// finished.
    Waiting,
}

impl GithubWorkflowRunStatus {
    /// The status GitHub names `status`.
    pub fn from_github(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            _ => Self::Waiting,
        }
    }

    /// Whether the run has not finished, as one a concurrency group holds.
    pub fn is_waiting(self) -> bool {
        self == Self::Waiting
    }
}

/// The latest commit status of one context on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCommitStatus {
    /// The context it reports, such as a CI's name.
    pub context: String,
    /// `success`, `pending`, `failure` or `error`.
    pub state: String,
    /// Its page, when it gives one.
    pub target_url: Option<String>,
}

/// A check run on a commit as CI health reads it: how it ended, and what it
/// says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCheckRunResult {
    /// GitHub's id for it; for a GitHub Actions run, its job's id.
    pub id: u64,
    /// Its name.
    pub name: String,
    /// GitHub's status: `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How it concluded, once completed.
    pub conclusion: Option<String>,
    /// The slug of the app that runs it.
    pub app_slug: Option<String>,
    /// The title of its output, such as the error a CI reports.
    pub output_title: Option<String>,
    /// The summary of its output.
    pub output_summary: Option<String>,
}

/// One step of a GitHub Actions job, in the job's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubJobStep {
    /// Its name, as the workflow names it or GitHub names it for an action.
    pub name: String,
    /// How it concluded, once completed.
    pub conclusion: Option<String>,
}

/// An annotation a check run left on the code, such as a test that failed:
/// GitHub Actions adds one for each error a step reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCheckAnnotation {
    /// The file it is on, from the repository's root; GitHub Actions puts
    /// its own errors on `.github`.
    pub path: String,
    /// `notice`, `warning` or `failure`.
    pub level: String,
    /// Its title, when it has one.
    pub title: Option<String>,
    /// What it says.
    pub message: String,
}
