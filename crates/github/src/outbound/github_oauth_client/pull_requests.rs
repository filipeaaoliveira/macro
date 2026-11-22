//! Calls that take a branch to a merged pull request as the user: what they
//! may do with a repository, the pull request from a branch, opening one,
//! the checks and statuses on its head, and deleting the branch once
//! merged. Each read is one request for only what it needs, as they are
//! read again and again while CI runs.
//!
//! A refusal GitHub answers is told apart from an outage: 401 is a token
//! that no longer works ([`GithubError::ReauthenticationRequired`]), 403
//! that is not throttling is [`GithubError::Forbidden`], and 404 is a
//! repository or commit the user cannot reach
//! ([`GithubError::RepositoryUnavailable`]). Throttling, 5xx and transport
//! failures are [`GithubError::Upstream`].

use reqwest::StatusCode;
use serde::de::DeserializeOwned;

use super::{
    GithubErrorBody, GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling,
    transport_error, unexpected,
};
use crate::domain::models::{
    GithubBranchDeletion, GithubCheckAnnotation, GithubCheckRun, GithubCheckRunResult,
    GithubCheckSuite, GithubCommitStatus, GithubError, GithubJobStep, GithubListing,
    GithubOpenPullRequest, GithubOpenPullRequestItem, GithubOpenPullRequestOutcome,
    GithubPullRequestHead, GithubRepositoryAccess, GithubRequiredCheck, GithubRequiredChecks,
    GithubWorkflowRun, GithubWorkflowRunStatus,
};

#[cfg(test)]
mod test;

/// The most items one page of a list holds.
const PAGE_SIZE: usize = 100;

/// The most pages of one list read: past any real commit's checks, so a
/// list GitHub keeps paging is not read for ever.
const MAX_PAGES: usize = 10;

/// GitHub's message for a branch it does not have, on a ref's deletion.
const MISSING_REFERENCE: &str = "Reference does not exist";

/// What GitHub's message starts with when a pull request's branch is not
/// updated because its head is no longer the commit the caller named.
const EXPECTED_HEAD_MOVED: &str = "expected head sha didn";

/// What GitHub's message holds, in any case, when a pull request's branch
/// is not updated because its base and it conflict ("merge conflict between
/// base and head").
const MERGE_CONFLICT: &str = "merge conflict";

/// What GitHub did when asked to merge a pull request's base into its
/// branch ([`GithubOauthImpl::update_pull_request_branch`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubBranchUpdate {
    /// GitHub took the request: it merges the base into the branch, as a
    /// commit of its own on top of the head the caller named, shortly.
    Started,
    /// The branch's head is no longer the commit the caller named: someone
    /// pushed to it, and nothing was merged.
    HeadMoved,
    /// The base and the branch conflict, with GitHub's message: nothing
    /// was merged, and someone resolves it by hand.
    Conflict(String),
    /// GitHub declined for another reason, with its message, as for a
    /// token that lacks a scope or a branch it may not push to: no
    /// conflict, and asking again changes nothing.
    Declined(String),
}

/// Whether git takes `branch` as a branch's name, by the rules of
/// `git check-ref-format --branch`: no empty part and none that starts with
/// `.` or ends with `.lock`, no `..` or `@{`, no control character, space,
/// `~`, `^`, `:`, `?`, `*`, `[` or `\`, and not `@`, nor starting with `-`
/// or ending with `.`.
fn is_branch_name(branch: &str) -> bool {
    const REFUSED: &[char] = &[' ', '~', '^', ':', '?', '*', '[', '\\'];

    let refused_char = branch
        .chars()
        .any(|c| c.is_ascii_control() || REFUSED.contains(&c));
    let refused_part = branch
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"));
    !branch.is_empty()
        && branch != "@"
        && !branch.starts_with('-')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !refused_char
        && !refused_part
}

/// Whether `part` can be a GitHub owner's or repository's name: letters,
/// digits, `-`, `_` and `.`, and neither `.` nor `..`, which URL parsing
/// would fold.
pub(super) fn is_repository_part(part: &str) -> bool {
    let allowed = part
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    allowed && !part.is_empty() && part != "." && part != ".."
}

/// What a refused read is, once throttling is ruled out.
fn refusal(status: StatusCode) -> Option<GithubError> {
    match status {
        StatusCode::UNAUTHORIZED => Some(GithubError::ReauthenticationRequired),
        StatusCode::FORBIDDEN => Some(GithubError::Forbidden),
        StatusCode::NOT_FOUND => Some(GithubError::RepositoryUnavailable),
        _ => None,
    }
}

#[derive(serde::Deserialize)]
struct RepositoryResponse {
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    permissions: Option<RepositoryPermissions>,
    #[serde(default)]
    delete_branch_on_merge: bool,
}

#[derive(serde::Deserialize)]
struct RepositoryPermissions {
    #[serde(default)]
    push: bool,
}

#[derive(serde::Deserialize)]
struct PullRequestListItem {
    number: u64,
    html_url: String,
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    head: PullRequestHeadRef,
}

#[derive(serde::Deserialize)]
struct OpenPullRequestListItem {
    number: u64,
    title: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
}

#[derive(serde::Deserialize)]
struct PullRequestHeadRef {
    sha: String,
}

#[derive(serde::Deserialize)]
struct CheckSuitesResponse {
    check_suites: Vec<CheckSuiteResponse>,
}

#[derive(serde::Deserialize)]
struct CheckSuiteResponse {
    id: u64,
    status: Option<String>,
    conclusion: Option<String>,
    #[serde(default)]
    latest_check_runs_count: u64,
    app: Option<AppResponse>,
    #[serde(default)]
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(serde::Deserialize)]
struct AppResponse {
    slug: Option<String>,
}

#[derive(serde::Deserialize)]
struct CombinedStatusResponse {
    statuses: Vec<StatusResponse>,
}

#[derive(serde::Deserialize)]
struct StatusResponse {
    context: String,
    state: String,
    target_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct BranchResponse {
    #[serde(default)]
    protected: bool,
    #[serde(default)]
    protection: Option<BranchProtection>,
}

#[derive(serde::Deserialize)]
struct BranchProtection {
    #[serde(default)]
    required_status_checks: Option<RequiredStatusChecks>,
}

#[derive(serde::Deserialize)]
struct RequiredStatusChecks {
    #[serde(default)]
    contexts: Vec<String>,
    #[serde(default)]
    checks: Vec<RequiredCheck>,
}

/// A required check: branch protection names its app `app_id`, a
/// ruleset `integration_id`.
#[derive(serde::Deserialize)]
struct RequiredCheck {
    context: String,
    #[serde(default)]
    app_id: Option<i64>,
    #[serde(default)]
    integration_id: Option<i64>,
}

/// An app id a required check names: GitHub writes -1 for "any source".
fn named_app(app_id: Option<i64>) -> Option<u64> {
    app_id.and_then(|app_id| u64::try_from(app_id).ok())
}

#[derive(serde::Deserialize)]
struct WorkflowRunsResponse {
    workflow_runs: Vec<WorkflowRunResponse>,
}

#[derive(serde::Deserialize)]
struct WorkflowRunResponse {
    status: Option<String>,
}

#[derive(serde::Deserialize)]
struct BranchRule {
    #[serde(rename = "type")]
    kind: BranchRuleKind,
    #[serde(default)]
    parameters: Option<BranchRuleParameters>,
}

/// The kinds of branch rule this reads; any other is left alone.
#[derive(serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BranchRuleKind {
    RequiredStatusChecks,
    PullRequest,
    #[serde(other)]
    Other,
}

#[derive(serde::Deserialize)]
struct BranchRuleParameters {
    #[serde(default)]
    required_status_checks: Vec<RequiredCheck>,
}

#[derive(serde::Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRunResponse>,
}

#[derive(serde::Deserialize)]
struct CheckRunResponse {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
    app: Option<CheckRunApp>,
}

#[derive(serde::Deserialize)]
struct CheckRunApp {
    id: u64,
}

#[derive(serde::Deserialize)]
struct CheckRunResultsResponse {
    check_runs: Vec<CheckRunResultResponse>,
}

#[derive(serde::Deserialize)]
struct CheckRunResultResponse {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    app: Option<CheckRunResultApp>,
    #[serde(default)]
    output: Option<CheckRunOutput>,
}

#[derive(serde::Deserialize)]
struct CheckRunResultApp {
    slug: Option<String>,
}

#[derive(serde::Deserialize)]
struct CheckRunOutput {
    title: Option<String>,
    summary: Option<String>,
}

#[derive(serde::Deserialize)]
struct CheckAnnotationResponse {
    path: String,
    annotation_level: Option<String>,
    title: Option<String>,
    message: Option<String>,
}

#[derive(serde::Deserialize)]
struct ActionsJobResponse {
    #[serde(default)]
    steps: Vec<ActionsJobStepResponse>,
}

#[derive(serde::Deserialize)]
struct ActionsJobStepResponse {
    name: String,
    conclusion: Option<String>,
}

impl GithubOauthImpl {
    /// GET `path` of the REST API as the user, read as `T`.
    pub(super) async fn read<T: DeserializeOwned>(
        &self,
        access_token: &str,
        path: &str,
        query: &[(&str, &str)],
        context: &str,
    ) -> Result<T, GithubError> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_base_url()))
            .query(query)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            return response.json().await.map_err(transport_error);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(status, &headers, body, context));
        }
        Err(refusal(status).unwrap_or_else(|| unexpected(status, &headers, body, context)))
    }

    /// Every page of the list at `path`, each read as `T` and its items
    /// taken out by `items`, up to [`MAX_PAGES`]: a page shorter than a full
    /// one is the last. A list still full at its last page read is not
    /// complete.
    async fn read_pages<T: DeserializeOwned, I>(
        &self,
        access_token: &str,
        path: &str,
        context: &str,
        items: impl Fn(T) -> Vec<I>,
    ) -> Result<GithubListing<I>, GithubError> {
        let page_size = PAGE_SIZE.to_string();
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let page = page.to_string();
            let read: T = self
                .read(
                    access_token,
                    path,
                    &[("per_page", page_size.as_str()), ("page", page.as_str())],
                    context,
                )
                .await?;
            let read = items(read);
            let last = read.len() < PAGE_SIZE;
            all.extend(read);
            if last {
                return Ok(GithubListing {
                    items: all,
                    complete: true,
                });
            }
        }
        // A full last page: there may be more than was read.
        Ok(GithubListing {
            items: all,
            complete: false,
        })
    }

    /// Whether a change reaches `branch` only through a pull request: a
    /// ruleset that applies to it requires one, or its branch protection
    /// requires pull request reviews. `None` when that cannot be told: the
    /// rulesets cannot be read, or the protection is refused to the caller,
    /// as it is to one who does not administer the repository.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_requires_pull_request(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<bool>, GithubError> {
        let branch = urlencoding::encode(branch);
        let rules = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/rules/branches/{branch}"),
                "failed to read branch rules",
                |page: Vec<BranchRule>| page,
            )
            .await;
        match rules {
            Ok(rules)
                if rules
                    .items
                    .iter()
                    .any(|rule| rule.kind == BranchRuleKind::PullRequest) =>
            {
                return Ok(Some(true));
            }
            Ok(_) => {}
            Err(GithubError::Forbidden | GithubError::RepositoryUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        }

        // No ruleset asks for one: the branch protection may, which only
        // the repository's admins may read. GitHub answers 404 when it
        // asks for none, or the branch has no protection.
        let reviews: Result<serde_json::Value, GithubError> = self
            .read(
                access_token,
                &format!(
                    "/repos/{owner}/{repo}/branches/{branch}/protection/required_pull_request_reviews"
                ),
                &[],
                "failed to read branch protection",
            )
            .await;
        match reviews {
            Ok(_) => Ok(Some(true)),
            Err(GithubError::RepositoryUnavailable) => Ok(Some(false)),
            Err(GithubError::Forbidden) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The checks a pull request to `branch` must pass before it merges:
    /// the branch protection's and the rulesets' required status checks,
    /// each with the app it must come from when the rule names one. What can
    /// be read is joined, and marked incomplete unless both sources were read
    /// in full; `None` when neither the branch nor its rules can be read, or
    /// the branch is protected but its protection is not shown, so a caller
    /// cannot tell which checks are required.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_required_checks(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<GithubRequiredChecks>, GithubError> {
        let branch = urlencoding::encode(branch);
        let branch_read: Result<BranchResponse, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/branches/{branch}"),
                &[],
                "failed to read branch",
            )
            .await;
        let rules_read = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/rules/branches/{branch}"),
                "failed to read branch rules",
                |page: Vec<BranchRule>| page,
            )
            .await;

        // A refusal leaves that source unread; any other failure is the
        // caller's to retry.
        let unread = |error: &GithubError| {
            matches!(
                error,
                GithubError::Forbidden | GithubError::RepositoryUnavailable
            )
        };
        let branch_read = match branch_read {
            Ok(read) => Some(read),
            Err(error) if unread(&error) => None,
            Err(error) => return Err(error),
        };
        // Rules read in part still name required checks; the set is then
        // incomplete.
        let (rules, rules_complete) = match rules_read {
            Ok(rules) => (Some(rules.items), rules.complete),
            Err(error) if unread(&error) => (None, false),
            Err(error) => return Err(error),
        };

        let mut required = Vec::new();
        match &branch_read {
            // A protected branch whose protection is not shown requires
            // checks no one here can name.
            Some(read) if read.protected && read.protection.is_none() => return Ok(None),
            Some(read) => {
                let checks = read
                    .protection
                    .as_ref()
                    .and_then(|protection| protection.required_status_checks.as_ref());
                if let Some(checks) = checks {
                    // `checks` names each context with its app; `contexts`
                    // is the older form, any app's.
                    required.extend(checks.checks.iter().map(|check| GithubRequiredCheck {
                        context: check.context.clone(),
                        app_id: named_app(check.app_id),
                    }));
                    required.extend(
                        checks
                            .contexts
                            .iter()
                            .filter(|context| {
                                !checks.checks.iter().any(|check| &check.context == *context)
                            })
                            .map(|context| GithubRequiredCheck {
                                context: context.clone(),
                                app_id: None,
                            }),
                    );
                }
            }
            None => {}
        }
        for rule in rules.iter().flatten() {
            if rule.kind == BranchRuleKind::RequiredStatusChecks
                && let Some(parameters) = &rule.parameters
            {
                required.extend(parameters.required_status_checks.iter().map(|check| {
                    GithubRequiredCheck {
                        context: check.context.clone(),
                        app_id: named_app(check.integration_id),
                    }
                }));
            }
        }
        if branch_read.is_none() && rules.is_none() {
            return Ok(None);
        }
        required.sort();
        required.dedup();
        Ok(Some(GithubRequiredChecks {
            checks: required,
            complete: branch_read.is_some() && rules_complete,
        }))
    }

    /// The repository's default branch, and whether the user may push to
    /// it; `None` when they cannot reach it.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_repository_access(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Option<GithubRepositoryAccess>, GithubError> {
        let read: Result<RepositoryResponse, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}"),
                &[],
                "failed to read repository",
            )
            .await;
        match read {
            Ok(repository) => Ok(Some(GithubRepositoryAccess {
                full_name: repository.full_name,
                default_branch: repository.default_branch,
                push: repository
                    .permissions
                    .is_some_and(|permissions| permissions.push),
                delete_branch_on_merge: repository.delete_branch_on_merge,
            })),
            // GitHub answers a private repository the user cannot see as
            // missing.
            Err(GithubError::RepositoryUnavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Whether an open pull request of the repository is based on `base`,
    /// so merging it would merge into that branch. An owner or repository
    /// name GitHub would not take is refused before any request. The
    /// caller logs a
    /// failure; only the shared helpers (`unexpected`, `transport_error`)
    /// log an outage's own line.
    #[tracing::instrument(skip(self, access_token))]
    pub async fn has_open_pull_request_onto(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        base: &str,
    ) -> Result<bool, GithubError> {
        if !is_repository_part(owner) || !is_repository_part(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }

        let open: Vec<serde::de::IgnoredAny> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/pulls"),
                &[("base", base), ("state", "open"), ("per_page", "1")],
                "failed to list pull requests onto a branch",
            )
            .await?;
        Ok(!open.is_empty())
    }

    /// The repository's open pull requests, most recently updated first,
    /// at most `limit` (one page, up to [`PAGE_SIZE`]). An owner or
    /// repository name GitHub would not take is refused before any request.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_open_pull_requests(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        limit: usize,
    ) -> Result<Vec<GithubOpenPullRequestItem>, GithubError> {
        if !is_repository_part(owner) || !is_repository_part(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }
        let per_page = limit.clamp(1, PAGE_SIZE).to_string();
        let open: Vec<OpenPullRequestListItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/pulls"),
                &[
                    ("state", "open"),
                    ("sort", "updated"),
                    ("direction", "desc"),
                    ("per_page", per_page.as_str()),
                ],
                "failed to list open pull requests",
            )
            .await?;
        Ok(open
            .into_iter()
            .take(limit)
            .map(|pull_request| GithubOpenPullRequestItem {
                number: pull_request.number,
                title: pull_request.title,
                html_url: pull_request.html_url,
                draft: pull_request.draft,
            })
            .collect())
    }

    /// The newest pull request, open or closed, from the repository's own
    /// branch `head`, with its head commit.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn find_pull_request_by_head(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        head: &str,
    ) -> Result<Option<GithubPullRequestHead>, GithubError> {
        // GitHub names a head as `owner:branch`; newest first, so a branch
        // whose pull request was closed and opened again reads the open one.
        let pull_requests: Vec<PullRequestListItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/pulls"),
                &[
                    ("head", format!("{owner}:{head}").as_str()),
                    ("state", "all"),
                    ("sort", "created"),
                    ("direction", "desc"),
                    ("per_page", "1"),
                ],
                "failed to find pull request",
            )
            .await?;
        Ok(pull_requests
            .into_iter()
            .next()
            .map(|pull_request| GithubPullRequestHead {
                number: pull_request.number,
                html_url: pull_request.html_url,
                state: pull_request.state,
                merged: pull_request.merged_at.is_some(),
                head_sha: pull_request.head.sha,
                // GitHub names a test merge commit while a pull request is
                // open: only a merged one's is kept.
                merge_commit_sha: pull_request.merged_at.and(pull_request.merge_commit_sha),
            }))
    }

    /// The check suites on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_check_suites(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<GithubListing<GithubCheckSuite>, GithubError> {
        let suites = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/check-suites"),
                "failed to list check suites",
                |page: CheckSuitesResponse| page.check_suites,
            )
            .await?;
        Ok(GithubListing {
            complete: suites.complete,
            items: suites
                .items
                .into_iter()
                .map(|suite| GithubCheckSuite {
                    id: suite.id,
                    app: suite
                        .app
                        .and_then(|app| app.slug)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status: suite.status.unwrap_or_else(|| "queued".to_string()),
                    conclusion: suite.conclusion,
                    check_runs: suite.latest_check_runs_count,
                    created_at: suite.created_at,
                })
                .collect(),
        })
    }

    /// The GitHub Actions workflow runs on the commit `sha`: a run that
    /// waits, such as one a concurrency group holds, has no check run yet.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_workflow_runs(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<GithubListing<GithubWorkflowRun>, GithubError> {
        let runs = self
            .read_pages(
                access_token,
                &format!(
                    "/repos/{owner}/{repo}/actions/runs?head_sha={}",
                    urlencoding::encode(sha)
                ),
                "failed to list workflow runs",
                |page: WorkflowRunsResponse| page.workflow_runs,
            )
            .await?;
        Ok(GithubListing {
            complete: runs.complete,
            items: runs
                .items
                .into_iter()
                .map(|run| GithubWorkflowRun {
                    status: GithubWorkflowRunStatus::from_github(
                        run.status.as_deref().unwrap_or("queued"),
                    ),
                })
                .collect(),
        })
    }

    /// The latest commit status of each context on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_commit_statuses(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<GithubListing<GithubCommitStatus>, GithubError> {
        let statuses = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/status"),
                "failed to read commit status",
                |page: CombinedStatusResponse| page.statuses,
            )
            .await?;
        Ok(GithubListing {
            complete: statuses.complete,
            items: statuses
                .items
                .into_iter()
                .map(|status| GithubCommitStatus {
                    context: status.context,
                    state: status.state,
                    target_url: status.target_url,
                })
                .collect(),
        })
    }

    /// The check runs on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_check_runs(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<GithubListing<GithubCheckRun>, GithubError> {
        let runs = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"),
                "failed to list check runs",
                |page: CheckRunsResponse| page.check_runs,
            )
            .await?;
        Ok(GithubListing {
            complete: runs.complete,
            items: runs
                .items
                .into_iter()
                .map(|run| GithubCheckRun {
                    name: run.name,
                    status: run.status,
                    conclusion: run.conclusion,
                    html_url: run.html_url,
                    app_id: run.app.map(|app| app.id),
                })
                .collect(),
        })
    }

    /// The latest check run of each name on the commit `sha`, with its id,
    /// its app and what its output says, for recording how CI went.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_check_run_results(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<GithubListing<GithubCheckRunResult>, GithubError> {
        let runs = self
            .read_pages(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"),
                "failed to list check run results",
                |page: CheckRunResultsResponse| page.check_runs,
            )
            .await?;
        Ok(GithubListing {
            complete: runs.complete,
            items: runs
                .items
                .into_iter()
                .map(|run| {
                    let (output_title, output_summary) = run
                        .output
                        .map(|output| (output.title, output.summary))
                        .unwrap_or_default();
                    GithubCheckRunResult {
                        id: run.id,
                        name: run.name,
                        status: run.status,
                        conclusion: run.conclusion,
                        app_slug: run.app.and_then(|app| app.slug),
                        output_title,
                        output_summary,
                    }
                })
                .collect(),
        })
    }

    /// The steps of the GitHub Actions job `job_id`, in order: a check run
    /// GitHub Actions runs has its job's id.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn actions_job_steps(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        job_id: u64,
    ) -> Result<Vec<GithubJobStep>, GithubError> {
        let job: ActionsJobResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/actions/jobs/{job_id}"),
                &[],
                "failed to read actions job",
            )
            .await?;
        Ok(job
            .steps
            .into_iter()
            .map(|step| GithubJobStep {
                name: step.name,
                conclusion: step.conclusion,
            })
            .collect())
    }

    /// The first `limit` annotations, at most 100, of the check run
    /// `check_run_id`: one page, in GitHub's order.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn check_run_annotations(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        check_run_id: u64,
        limit: usize,
    ) -> Result<Vec<GithubCheckAnnotation>, GithubError> {
        let per_page = limit.clamp(1, PAGE_SIZE).to_string();
        let annotations: Vec<CheckAnnotationResponse> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/check-runs/{check_run_id}/annotations"),
                &[("per_page", per_page.as_str())],
                "failed to list check run annotations",
            )
            .await?;
        Ok(annotations
            .into_iter()
            .map(|annotation| GithubCheckAnnotation {
                path: annotation.path,
                level: annotation.annotation_level.unwrap_or_default(),
                title: annotation.title.filter(|title| !title.trim().is_empty()),
                message: annotation.message.unwrap_or_default(),
            })
            .collect())
    }

    /// Opens a pull request as the user. GitHub declining it is a
    /// [`GithubOpenPullRequestOutcome::Rejected`] value, not an error.
    #[tracing::instrument(skip(self, access_token, pull_request), fields(head = %pull_request.head, base = %pull_request.base), err)]
    pub async fn open_pull_request(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        pull_request: &GithubOpenPullRequest,
    ) -> Result<GithubOpenPullRequestOutcome, GithubError> {
        #[derive(serde::Serialize)]
        struct OpenRequest<'a> {
            title: &'a str,
            body: &'a str,
            head: &'a str,
            base: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct Opened {
            number: u64,
        }

        let response = self
            .client
            .post(format!(
                "{}/repos/{owner}/{repo}/pulls",
                self.api_base_url()
            ))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&OpenRequest {
                title: &pull_request.title,
                body: &pull_request.body,
                head: &pull_request.head,
                base: &pull_request.base,
            })
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            let opened: Opened = response.json().await.map_err(transport_error)?;
            return Ok(GithubOpenPullRequestOutcome::Opened(opened.number));
        }

        let headers = response.headers().clone();
        let body = response_text(response).await;
        // A throttled request is a 403 too; it must not read as a refusal.
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(
                status,
                &headers,
                body,
                "failed to open pull request",
            ));
        }
        match status {
            // A token that no longer works, or a user who may not reach the
            // repository, is the same refusal on every try.
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
                Err(refusal(status).unwrap_or(GithubError::Forbidden))
            }
            // GitHub's answer about the pull request itself: one already
            // open, or nothing to propose.
            StatusCode::UNPROCESSABLE_ENTITY => {
                let message = serde_json::from_str::<GithubErrorBody>(&body)
                    .map(GithubErrorBody::reason)
                    .unwrap_or_else(|_| "GitHub declined to open the pull request.".to_string());
                Ok(GithubOpenPullRequestOutcome::Rejected(message))
            }
            _ => Err(unexpected(
                status,
                &headers,
                body,
                "failed to open pull request",
            )),
        }
    }

    /// Asks GitHub to merge the base of pull request `number` into its
    /// branch as the user, only while the branch's head is
    /// `expected_head`: GitHub's "Update branch". It adds a merge commit on
    /// top of that head, so it never rewrites the branch; a head that moved
    /// since is left alone. The merge happens after GitHub answers: read the
    /// branch's head to see it.
    #[tracing::instrument(skip(self, access_token))]
    pub async fn update_pull_request_branch(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        expected_head: &str,
    ) -> Result<GithubBranchUpdate, GithubError> {
        #[derive(serde::Serialize)]
        struct UpdateRequest<'a> {
            expected_head_sha: &'a str,
        }

        if !is_repository_part(owner) || !is_repository_part(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }
        let response = self
            .client
            .put(format!(
                "{}/repos/{owner}/{repo}/pulls/{number}/update-branch",
                self.api_base_url()
            ))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&UpdateRequest {
                expected_head_sha: expected_head,
            })
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            return Ok(GithubBranchUpdate::Started);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(
                status,
                &headers,
                body,
                "failed to update a pull request's branch",
            ));
        }
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            let message = serde_json::from_str::<GithubErrorBody>(&body)
                .map(|error| error.message)
                .unwrap_or_else(|_| "GitHub declined to update the branch.".to_string());
            if message.starts_with(EXPECTED_HEAD_MOVED) {
                return Ok(GithubBranchUpdate::HeadMoved);
            }
            if message.to_ascii_lowercase().contains(MERGE_CONFLICT) {
                return Ok(GithubBranchUpdate::Conflict(message));
            }
            return Ok(GithubBranchUpdate::Declined(message));
        }
        Err(refusal(status).unwrap_or_else(|| {
            unexpected(
                status,
                &headers,
                body,
                "failed to update a pull request's branch",
            )
        }))
    }

    /// Deletes `branch` as the user, or finds it missing, as when it was
    /// deleted already. GitHub refusing it, as for a protected branch, is
    /// an error, and so is a name git would not take for a branch, which is
    /// never sent, as is an owner or repository name GitHub would not take.
    /// The caller logs a failure; only the shared helpers (`unexpected`,
    /// `transport_error`) log an outage's own line.
    #[tracing::instrument(skip(self, access_token))]
    pub async fn delete_branch(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<GithubBranchDeletion, GithubError> {
        // A part such as `..` would survive the encoding below and move the
        // request off the branch's ref, onto the repository itself.
        if !is_branch_name(branch) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a branch name: {branch:?}"
            )));
        }
        if !is_repository_part(owner) || !is_repository_part(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }

        // GitHub names a branch's ref by its path: each part is encoded, its
        // slashes are not.
        let path = branch
            .split('/')
            .map(|part| urlencoding::encode(part).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let response = self
            .client
            .delete(format!(
                "{}/repos/{owner}/{repo}/git/refs/heads/{path}",
                self.api_base_url()
            ))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            return Ok(GithubBranchDeletion::Deleted);
        }

        let headers = response.headers().clone();
        let body = response_text(response).await;
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(
                status,
                &headers,
                body,
                "failed to delete a branch",
            ));
        }
        // GitHub answers a branch that is not there with a 422 of its own.
        let missing = status == StatusCode::UNPROCESSABLE_ENTITY
            && serde_json::from_str::<GithubErrorBody>(&body)
                .is_ok_and(|error| error.message == MISSING_REFERENCE);
        if missing {
            return Ok(GithubBranchDeletion::Missing);
        }
        Err(refusal(status)
            .unwrap_or_else(|| unexpected(status, &headers, body, "failed to delete a branch")))
    }
}
