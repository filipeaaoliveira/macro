//! What the factory's use cases take and return.

#[cfg(test)]
mod test;

use bot_id::BotId;
use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::{MainHold, RequiredChecksState};
use factory_records::domain::models::{
    EntryId, InvalidName, Outcome, Repository, Role, Run, RunId, RunOutcome, SteerRefusal, Step,
    StepKey,
};
use factory_records::domain::review_rounds::ReviewRound;
use factory_records::domain::standing_acts::{StandingAct, StandingActRecord, StandingRoleName};
use factory_records::domain::standing_rules::{OrchestratorDefinition, ProjectRules, RulesSource};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::control_files::ControlFilesError;
use crate::domain::definition::DefinitionError;
use crate::domain::flow::{Entry, Flow, FlowError, StepName};
use crate::domain::gate_owner::OwnerNote;
use crate::domain::setup::RepositorySetup;
use crate::domain::token_budget::RunTokenBudget;

/// What a Run's workflow starts with. The Run's request stays in its
/// records, so a Run's history carries ids, not its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    /// The Run, already recorded.
    pub run_id: RunId,
    /// The Flow it walks.
    pub flow: Flow,
    /// Whether the Run keeps a token budget, which the workflow checks
    /// before each agent Step. Left out when it keeps none, so the start of
    /// a Run without one reads as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub token_budget: bool,
}

/// One Step for a worker to run. A field it does not know is refused, so a
/// worker never runs a Step it would misread: a worker older than a field
/// fails the activity, and a newer one picks it up. Fields are therefore
/// only ever added, with a default, never removed or renamed: Runs in
/// flight carry them in their histories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepRequest {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The entry it visits.
    pub entry: EntryId,
    /// The work it does.
    pub step: StepName,
    /// The Role an agent fills to do it, as its entry names.
    #[serde(default)]
    pub role: Option<Role>,
    /// Which visit of the entry it is, from 1.
    pub visit: u16,
    /// The review round the Step reviews in, by the place of the round's
    /// first Step; none for a Step of no review panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u16>,
    /// Whether its agent may ask the Run's owner a question, ending the
    /// Step `question`: its Flow lets agents ask.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub may_ask: bool,
    /// How many questions its work may still ask before the next one ends
    /// the Run for a person: the Flow's, less those the Steps it goes on
    /// from asked.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub questions_left: u8,
    /// The Step whose agent asked the question this Step goes on from, by
    /// its place: this Step works in that Step's session, with the owner's
    /// answer. None for a Step that starts its work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumes: Option<u16>,
}

impl StepRequest {
    /// The `visit` of `entry`, as the Step `key`: the work its entry names,
    /// and the Role it names for an agent to fill.
    pub fn of(key: StepKey, entry: &Entry, visit: u16) -> Self {
        Self {
            key,
            entry: entry.id.clone(),
            step: entry.step.clone(),
            role: entry.role.clone(),
            visit,
            round: None,
            may_ask: false,
            questions_left: 0,
            resumes: None,
        }
    }

    /// The Step `key` of the review round that starts at `round`, the
    /// `visit` of the review panel `entry`, in which a reviewer of `role`
    /// reviews.
    pub fn reviewer(key: StepKey, entry: &Entry, visit: u16, role: &Role, round: u16) -> Self {
        Self {
            key,
            entry: entry.id.clone(),
            step: entry.step.clone(),
            role: Some(role.clone()),
            visit,
            round: Some(round),
            may_ask: false,
            questions_left: 0,
            resumes: None,
        }
    }

    /// The Step `key` that goes on from this one's question, as the same
    /// visit of its entry, in its session.
    pub fn going_on(&self, key: StepKey) -> Self {
        Self {
            key,
            resumes: Some(self.key.seq),
            ..self.clone()
        }
    }
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// How a Step ended, for its workflow to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEnding {
    /// The Step.
    pub key: StepKey,
    /// How it ended.
    pub report: StepReport,
}

/// What a Step's work is given: the Run's request, and what the Run's Steps
/// before it produced.
#[derive(Debug, Clone, Copy)]
pub struct StepInput<'a> {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Role an agent fills to do the Step, as its entry names.
    pub role: Option<&'a Role>,
    /// The Run's owner, as whom an agent does the Step.
    pub owner: &'a MacroUserIdStr<'static>,
    /// What the Run was asked to do.
    pub request: &'a str,
    /// Whether the Run's owner created it in their own signed-in session:
    /// only then does its request ask for the Control Files it names.
    pub created_in_person: bool,
    /// The Control File patterns the Run keeps from its creation; none for
    /// a Run created before Runs kept them, which the default checks.
    pub control_file_patterns: Option<&'a [String]>,
    /// The repository of the Factory Definition the Run started under, by
    /// its URL; none when it started under none.
    pub definition_repo: Option<&'a str>,
    /// The Macro task the Run works on: its task document's id.
    pub task_id: Option<Uuid>,
    /// The repository the Run works on, if it works on one.
    pub repository: Option<&'a Repository>,
    /// The Run's Steps that ended before this one, in order.
    pub earlier: &'a [Step],
    /// For a reviewer's Step, the review round it reviews in; for any
    /// other, the Run's latest review round that ended before it.
    pub review: Option<&'a ReviewRound>,
    /// Whether it is a reviewer's Step of a review panel.
    pub reviewer: bool,
    /// Whether its agent may ask the Run's owner a question.
    pub may_ask: bool,
    /// How many questions its work may still ask before the next one ends
    /// the Run for a person.
    pub questions_left: u8,
    /// The Step whose question it goes on from, by its place: its agent
    /// goes on in that Step's session, with the owner's answer.
    pub resumes: Option<u16>,
    /// What the Run's owner commented on the Run's task since its latest
    /// Gate opened, oldest first: none unless the Run's owner section is
    /// on.
    pub owner_notes: &'a [OwnerNote],
    /// The commit the Run's branch starts from, when the Run is a Benchmark
    /// trial pinned to one; otherwise the branch starts from its base.
    pub base_commit: Option<&'a str>,
}

/// What an agent is asked to do for one Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTask {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, as whom the agent works.
    pub owner: MacroUserIdStr<'static>,
    /// The Step.
    pub step: StepName,
    /// The Role the agent fills.
    pub role: Role,
    /// The Step's instructions, the Run's request and its earlier Steps.
    pub prompt: String,
    /// The outcomes the agent may end the Step with.
    pub outcomes: Vec<Outcome>,
    /// The repository the agent works in, as its
    /// `https://github.com/owner/name` URL, if the Run works on one.
    pub repo_url: Option<String>,
    /// When to stop waiting for the agent's answer, at the latest, when the
    /// Step sets a time of its own; the attempt's own deadline holds too.
    pub answer_by: Option<DateTime<Utc>>,
    /// The Step whose question this one goes on from, by its place: the
    /// agent is prompted in that Step's session, which it goes on in, not
    /// a new one. None for a Step that starts its work.
    pub resumes: Option<u16>,
    /// For a Step that goes on from a question, its whole prompt, the
    /// owner's answers in it: what a new session is prompted with when the
    /// asking Step's cannot be resumed.
    pub afresh: Option<String>,
    /// The repository's setup, read from the Run's base, which each new
    /// session of the Step runs before its first prompt; none when the Run
    /// works on no repository or its base declares no setup.
    pub setup: Option<RepositorySetup>,
}

/// What the Run's owner's Step Terminal asks of the agent session a Step
/// works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSteer {
    /// Send the session `prompt` as the prompt `prompt_id`: the agent reads
    /// it once its turn ends, or at once when it works on none.
    Prompt {
        /// The prompt's id, a UUIDv7 the Step keeps.
        prompt_id: Uuid,
        /// What it says.
        prompt: String,
    },
    /// Stop the agent's turn.
    Interrupt,
}

/// A later prompt an agent is asked to work on, in the session it did its
/// Step's work in: a round of checks, or the fix of one that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurn {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, as whom the agent works.
    pub owner: MacroUserIdStr<'static>,
    /// The Role the agent fills, whose Agent's session the Step works in.
    pub role: Role,
    /// The prompt's id in the session, a UUIDv7 the Step recorded before
    /// asking, so a retried Step asks under the same one.
    pub action_id: Uuid,
    /// Its place among the Step's later prompts: a later prompt has a
    /// higher one, whatever its id, so a stale attempt never takes the Step
    /// back to an earlier prompt.
    pub place: u16,
    /// What the agent is asked.
    pub prompt: String,
    /// The outcomes the agent may end the prompt with.
    pub outcomes: Vec<Outcome>,
    /// When to stop waiting for the agent's answer, at the latest: the
    /// Step's own time for its rounds.
    pub answer_by: DateTime<Utc>,
}

/// A command run in the checkout of the session a Step's agent works in,
/// beside the agent: one check of a round the engine runs itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, whose session it is.
    pub owner: MacroUserIdStr<'static>,
    /// The Role the agent fills, whose Agent's session the Step works in.
    pub role: Role,
    /// What to run, from the checkout's root.
    pub command: agent_session::domain::session_command::SessionCommand,
}

/// A GitHub repository, named by its owner and name: the two segments of
/// the `https://github.com/owner/name` URL a coding session takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRepository {
    owner: String,
    name: String,
}

impl CodeRepository {
    /// The repository `url` names. It must be exactly
    /// `https://github.com/owner/name`, each segment of `[A-Za-z0-9._-]` and
    /// neither `.` nor `..`, with no `.git` ending or trailing slash: the
    /// canonical form a coding session's repository takes, so a Run and its
    /// sessions agree on which repository they work on. The segments are put
    /// into GitHub's API paths.
    pub fn parse(url: &str) -> Option<Self> {
        let path = url.strip_prefix("https://github.com/")?;
        let (owner, name) = path.split_once('/')?;
        let segment = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        };
        // A `.git` ending names the same repository as one without; one
        // form keeps a Run's record and its session's equal.
        if !segment(owner) || !segment(name) || name.ends_with(".git") {
            return None;
        }
        Some(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// The account the repository lives under.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repository's name, without its owner.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its `https://github.com/owner/name` URL.
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

/// The branch a Run's work is pushed to and proposed from. A Run has one, so
/// every Step of it, and every retry of a Step, finds the same branch and the
/// same pull request.
pub fn run_branch(run_id: RunId) -> String {
    format!("{RUN_BRANCH_PREFIX}{run_id}")
}

/// What every Run's branch starts with, before its Run's id.
pub const RUN_BRANCH_PREFIX: &str = "factory/";

/// Whether `branch` is a Run's branch: the prefix, then a Run's id written
/// as [`run_branch`] writes it, hyphenated and in lower case. An id written
/// another way (upper case, no hyphens, braces, a `urn:uuid:` prefix) is
/// another branch.
pub fn is_run_branch(branch: &str) -> bool {
    branch
        .strip_prefix(RUN_BRANCH_PREFIX)
        .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id))
}

/// What a Run's owner may do with a repository on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryAccess {
    /// Its `owner/name` as GitHub names it now, when GitHub says: another
    /// than the one asked for when it followed a renamed or transferred
    /// repository.
    pub full_name: Option<String>,
    /// The branch it checks out by default.
    pub default_branch: String,
    /// Whether the owner may push to it.
    pub can_push: bool,
    /// Whether GitHub deletes a pull request's branch once it merges.
    pub deletes_merged_branches: bool,
}

/// A pull request as the Run's owner sees it on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number in its repository.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
    /// Whether it is open, closed, or merged.
    pub state: PullRequestState,
    /// The commit its head branch is at.
    pub head_sha: String,
}

/// Whether a pull request is open, closed, or merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestState {
    /// Open.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged.
    Merged,
}

/// What GitHub says of the checks on a pull request's head commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadChecks {
    /// Its check suites, one per app that checks it, such as a CI.
    pub suites: Vec<CheckSuite>,
    /// Its commit statuses, the latest of each context.
    pub statuses: Vec<CommitStatus>,
    /// Its check runs: every one when they were read to find required
    /// checks by name, else only those that failed, read to name them.
    pub runs: Vec<Check>,
    /// Whether a list was longer than was read, so these are not all its
    /// checks.
    pub truncated: bool,
    /// The apps, by slug, that hold CI on the head: a run not finished, or
    /// runs not all read, as a GitHub Actions run a concurrency group holds,
    /// whose suite may be empty or not made yet.
    pub held_apps: Vec<String>,
    /// Whether the owner's token may not read GitHub Actions runs, so a run
    /// that waits shows only through an empty Actions suite.
    pub actions_unread: bool,
}

/// A CI job that finished on a commit, as CI health records it: a check
/// run's latest, or a commit status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedJob {
    /// Its name: the check run's, or the commit status's context.
    pub name: String,
    /// How it ended: [`CheckState::Passed`] or [`CheckState::Failed`].
    pub state: CheckState,
    /// For a GitHub Actions job that failed, the first of its steps that
    /// failed, when its steps could be read.
    pub failed_step: Option<String>,
    /// What a check run that failed says of itself: its output's title and
    /// summary.
    pub detail: Option<String>,
    /// The failures a check run that failed annotated, such as the tests
    /// that failed, when they could be read.
    pub annotations: Vec<FailureAnnotation>,
}

/// A failure a check run annotated: a test that failed, or an error of the
/// CI itself, which GitHub Actions puts on `.github`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureAnnotation {
    /// The file it is on, from the repository's root.
    pub path: String,
    /// Its title, when it has one, such as the test's name.
    pub title: Option<String>,
    /// What it says.
    pub message: String,
}

/// The checks a pull request's base requires, as far as they could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredChecks {
    /// The checks the readable sources name.
    pub checks: Vec<RequiredCheck>,
    /// Whether every source was read in full; a partial set may leave
    /// required checks out.
    pub complete: bool,
}

/// A check a pull request's base requires before it merges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredCheck {
    /// The check run's name, or the commit status's context.
    pub name: String,
    /// The app whose check run it must be; none when any app's run, or a
    /// commit status, will do.
    pub app_id: Option<u64>,
}

/// Where one check stands. GitHub's states map onto it in one place
/// ([`CheckState::of_check`], [`CheckState::of_status`]), so every reader
/// agrees on what passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckState {
    /// Not finished, or not started.
    Waiting,
    /// Finished, and lets a pull request merge.
    Passed,
    /// Finished, and does not: GitHub's conclusion or state, for people.
    Failed(String),
}

impl CheckState {
    /// A check suite's or run's state from GitHub's `status` and
    /// `conclusion`: a completed one passed when it succeeded, was neutral,
    /// or was skipped.
    pub fn of_check(status: &str, conclusion: Option<&str>) -> Self {
        if status != "completed" {
            return Self::Waiting;
        }
        match conclusion {
            Some("success" | "neutral" | "skipped") => Self::Passed,
            Some(conclusion) => Self::Failed(conclusion.to_string()),
            None => Self::Failed("no conclusion".to_string()),
        }
    }

    /// A commit status's state from GitHub's: `success` passed, `pending`
    /// waits, and `failure` or `error` failed.
    pub fn of_status(state: &str) -> Self {
        match state {
            "success" => Self::Passed,
            "pending" => Self::Waiting,
            other => Self::Failed(other.to_string()),
        }
    }
}

/// One app's checks on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSuite {
    /// GitHub's id for it.
    pub id: u64,
    /// The app that runs it.
    pub app: String,
    /// Where it stands.
    pub state: CheckState,
    /// How many check runs it holds: an app that runs nothing on the commit
    /// leaves an empty suite queued for good, though a CI may add its runs
    /// a while after it made the suite.
    pub check_runs: u64,
    /// When GitHub created it.
    pub created_at: Option<DateTime<Utc>>,
}

/// The latest commit status of one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStatus {
    /// The context, such as a CI's name.
    pub context: String,
    /// Where it stands.
    pub state: CheckState,
    /// Its page, when it gives one.
    pub url: Option<String>,
}

/// One check run, such as a CI job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Its name, which a branch's required checks name it by.
    pub name: String,
    /// Where it stands.
    pub state: CheckState,
    /// Its page, when GitHub gives one.
    pub url: Option<String>,
    /// GitHub's id of the app that runs it.
    pub app_id: Option<u64>,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPullRequest {
    /// The branch whose commits it proposes.
    pub branch: String,
    /// The branch it proposes them to.
    pub base: String,
    /// Its title.
    pub title: String,
    /// Its description.
    pub body: String,
}

/// What asking GitHub to open a pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// GitHub opened it.
    Opened,
    /// GitHub declined, for this reason: one is already open for the
    /// branch, or the branch has nothing its base lacks.
    Refused(String),
}

/// What asking GitHub to delete a branch came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDeletion {
    /// The branch was deleted.
    Deleted,
    /// The repository has no such branch, as when it was deleted already.
    Missing,
}

/// What asking GitHub to merge a pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merging {
    /// GitHub merged it.
    Merged,
    /// Its head is no longer the commit the merge named.
    HeadChanged,
    /// GitHub declined, for this reason.
    Refused(String),
}

/// What an agent handed back for a Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAnswer {
    /// One of the task's outcomes.
    pub outcome: Outcome,
    /// What it did, for a person and the Run's later Steps to read.
    pub text: String,
}

/// Whether a Step started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStart {
    /// The Step is recorded, and its work may begin.
    Started,
    /// The Step's Run ended before it started, as one cancelled, or ended
    /// for good by the sweep, before its workflow did. The workflow ends as
    /// the Run did.
    RunEnded(RunOutcome),
}

/// How a Step ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReport {
    /// Its outcome, which the Flow routes.
    pub outcome: Outcome,
    /// What it produced, for a person to read.
    pub output: String,
}

/// A Run's end, for its workflow to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFinish {
    /// The Run.
    pub run_id: RunId,
    /// How it ended.
    pub outcome: RunOutcome,
}

/// The factory Step an agent session works in, as its owner reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionStepRef {
    /// The Run the Step belongs to.
    pub run_id: RunId,
    /// The Step's place among the Run's Steps.
    pub seq: u16,
}

/// A Run and the Steps it took.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    /// The Run.
    pub run: Run,
    /// Its Steps, in the order they started.
    pub steps: Vec<Step>,
    /// The Main Hold on the base branch the Run merges to, while the Run
    /// is at its merge and the base's required checks fail: the Run does
    /// not merge until it clears, even once its merge Gate is approved.
    pub main_hold: Option<MainHold>,
    /// Why the checks the base branch requires decide nothing, while the
    /// Run is at its merge: they could not be read, or the base requires
    /// none. Nothing holds the merge on them.
    pub base_required_checks: Option<RequiredChecksState>,
    /// Its review rounds, in order, each with what its reviewers found.
    pub review_rounds: Vec<ReviewRound>,
    /// What agents did to it under its Factory Definition's Standing
    /// Roles, in order: who started, cancelled or linked it, and answered
    /// its Gates.
    pub standing_acts: Vec<StandingActRecord>,
    /// Its token budget and what its agent Steps used of it; none for a
    /// Run that keeps no budget.
    pub token_budget: Option<RunTokenBudget>,
}

/// The Control File patterns a person's Runs are checked against: their
/// team's, or the factory's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControlFilesSettings {
    /// The glob patterns, matched against each changed path from the
    /// repository's root, ignoring case.
    pub patterns: Vec<String>,
    /// Whether they are the factory's default: the team set none, or the
    /// person is on no team.
    pub is_default: bool,
    /// The team whose patterns they are; none when the person is on no
    /// team.
    pub team_id: Option<Uuid>,
    /// Whether the person may change them: they administer the team.
    pub can_edit: bool,
    /// Whether the team names a Factory Definition, whose `rules.json`
    /// sets its Runs' Control Files in place of these.
    pub definition_named: bool,
}

/// The Factory Definition a person's team names: the repository whose
/// files its Runs take their Flows, Agents and rules from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionSettings {
    /// The repository's `https://github.com/owner/name` URL; none when the
    /// team names none, so its Runs take the team's settings and the
    /// factory's built-in Flows and Agents.
    pub repo_url: Option<String>,
    /// The commit of its default branch a Run started now would read,
    /// when the Definition reads there as the person reaches it.
    pub commit: Option<String>,
    /// Why it does not read now, when it does not: the person cannot reach
    /// it, or a file of it is wrong.
    pub problem: Option<String>,
    /// Whether a ruleset or the branch protection of its default branch
    /// asks for a pull request; none when that cannot be told, as when the
    /// person does not administer it. Without one, a push changes the
    /// factory's rules unreviewed. With one, whoever may bypass the rules
    /// (a ruleset's bypass list, admins when protection does not include
    /// them) still can: GitHub does not tell the factory who.
    pub requires_pull_request: Option<bool>,
    /// The team; none when the person is on no team.
    pub team_id: Option<Uuid>,
    /// Whether the person may change it: they administer the team.
    pub can_edit: bool,
}

/// The Standing Rules a person's Runs are created with: their team's
/// Orchestrator Definition, or the factory's default, and the team's
/// Projects' own, which replace it for a Run whose task is in one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StandingRulesSettings {
    /// Which kinds of Gate the factory answers on its own.
    pub definition: OrchestratorDefinition,
    /// Whether it is the factory's default: the team set none, or the
    /// person is on no team.
    pub is_default: bool,
    /// The team's Projects that have their own, by the initiative's id.
    pub projects: Vec<ProjectRules>,
    /// The team whose rules they are; none when the person is on no team.
    pub team_id: Option<Uuid>,
    /// Whether the person may change them: they administer the team.
    pub can_edit: bool,
    /// Whether the team names a Factory Definition, whose `rules.json`
    /// sets its Runs' Standing Rules in place of these and the Projects'.
    pub definition_named: bool,
}

/// A Flow a person can start a Run of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowSummary {
    /// Its name, which a Run of it is started with.
    pub name: String,
    /// What it does, for a person.
    pub description: String,
    /// Whether a Run of it works on a GitHub repository, which starting it
    /// must name.
    pub repository: bool,
}

/// Who answers a Gate as its Run's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answerer {
    /// The owner, in their own signed-in session.
    InPerson,
    /// An agent session the owner works with, such as the Orchestrator in
    /// Macro chat, answering from its code with the owner's own access. It
    /// answers only what a Standing Rule could: never what the factory
    /// keeps for the person ([`PersonOnly`]).
    Agent {
        /// The agent session the call came from.
        session_id: Uuid,
    },
}

/// The agent a program acts as when it starts, cancels or links one of
/// its owner's Runs: what Standing Roles check. None acts for a person in
/// their own signed-in session, an internal caller, or a program signed
/// only for the person (their own API key or token, a routine running as
/// them with no agent): that is the person acting, which Standing Roles do
/// not govern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActingAgent {
    /// Code in an agent session, as the correlation layer validated it
    /// against the call's credential: its bot acts, when the owner owns
    /// the session.
    Session(Uuid),
    /// Code signed for a bot that runs in no agent session, such as a
    /// self-hosted agent's code-mode call or a routine's run for an agent:
    /// the bot its credential is signed for acts.
    Bot(BotId),
}

/// What the factory keeps for the Run's owner in person, which an agent
/// answering for them may not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PersonOnly {
    /// A session that is not a chat with the person: a routine's, or a
    /// background run's.
    #[error("only an agent the run's owner chats with answers a gate for them")]
    NotAChat,
    /// The session doing a Step of a Run: it would answer its own work.
    #[error("a factory step's own agent session never answers a gate")]
    StepSession,
    /// An agent's question, which only the owner answers.
    #[error("only the run's owner, signed in to Macro, answers an agent's question")]
    Question,
    /// A Gate its Flow keeps for a person, or one no Standing Rule could
    /// answer.
    #[error("only the run's owner, signed in to Macro, answers this gate")]
    PersonsGate,
    /// Approving a Tampered Run, or sending Control Files with an answer:
    /// only the owner approves them.
    #[error(
        "the run's branch changes control files: only the run's owner, signed in to Macro, approves them"
    )]
    ControlFiles,
    /// Approving a merge.
    #[error("only the run's owner, signed in to Macro, approves a merge")]
    Merge,
    /// The approving answer to a kind of Gate the Run's Orchestrator
    /// Definition does not let the Orchestrator approve.
    #[error(
        "the run's standing rules do not let the orchestrator approve this kind of gate: its owner answers it"
    )]
    NotAllowed,
    /// Sending Blockers to a fix the `fix_blockers` rule would leave to the
    /// owner: past its fix rounds in a row, or Blockers found again
    /// unchanged after a fix.
    #[error("these blockers are the run's owner's to send to another fix")]
    OwnersBlockers,
}

/// Why an agent may not do `act`, naming the Standing Roles that allow it.
fn standing_refusal(act: StandingAct, allowing: &[StandingRoleName]) -> String {
    if allowing.is_empty() {
        return format!(
            "no standing role of the run's factory definition allows {act}: the run's owner does it"
        );
    }
    let roles: Vec<&str> = allowing.iter().map(StandingRoleName::as_str).collect();
    format!(
        "this agent fills no standing role that allows {act}; the standing roles that allow it: {}",
        roles.join(", ")
    )
}

/// Who set a Queue limit, in words.
fn queue_limit_source(source: RulesSource) -> &'static str {
    match source {
        RulesSource::Project => "the project's standing rules",
        RulesSource::Definition => "your team's factory definition",
        RulesSource::Team | RulesSource::Default => "your team's standing rules",
    }
}

/// Errors of the factory's Run use cases.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The Flow definition does not hold together.
    #[error(transparent)]
    InvalidFlow(#[from] FlowError),
    /// The request is empty.
    #[error("the request must not be empty")]
    EmptyRequest,
    /// The request is larger than a Run keeps.
    #[error("the request must be at most {max} bytes")]
    RequestTooLarge {
        /// The most bytes a request may hold.
        max: usize,
    },
    /// No Run has this id.
    #[error("run not found")]
    NotFound,
    /// A page of the board names half of where it starts.
    #[error("afterCreatedAt and afterId go together")]
    HalfCursor,
    /// A lookup by pull request names more of them than it reads.
    #[error("at most {max} pull requests are looked up at once")]
    TooManyPullRequests {
        /// The most pull requests one lookup names.
        max: usize,
    },
    /// No task the Run's owner can see has this id.
    #[error("no task the run's owner can see has this id")]
    UnknownTask,
    /// No task the caller can see has the id the path names.
    #[error("task not found")]
    TaskNotFound,
    /// No built-in Flow has this name.
    #[error("no flow has this name")]
    UnknownFlow,
    /// The Flow works on a repository, and the Run names none.
    #[error("this flow works on a repository: give its repoUrl")]
    RepositoryRequired,
    /// The Flow works on no repository, and the Run names one.
    #[error("this flow works on no repository")]
    RepositoryNotUsed,
    /// The repository is not named by a `https://github.com/owner/name` URL.
    #[error("the repository must be named by a https://github.com/owner/name URL")]
    InvalidRepository,
    /// The Run's owner cannot reach the repository with their GitHub
    /// account, has not connected one, or the repository has no branch yet.
    #[error(
        "the run's owner must connect GitHub to Macro and be able to reach the repository, which needs a branch"
    )]
    UnreachableRepository,
    /// The Run's owner may read the repository, but not push to it.
    #[error("the run's owner must be able to push to the repository")]
    NoPushAccess,
    /// The team names a Factory Definition whose repository the caller
    /// cannot read with their GitHub account, has not connected one for, or
    /// that has no branch yet.
    #[error(
        "the team's factory definition must be in a repository the caller reaches with the GitHub account they connected to Macro"
    )]
    UnreachableDefinition,
    /// The team's Factory Definition does not read at its default branch's
    /// head.
    #[error("the team's factory definition does not read: {0}")]
    InvalidDefinition(DefinitionError),
    /// The team names a Factory Definition, which sets what was to be
    /// changed: it changes by pull request to its repository.
    #[error(
        "the team's factory definition sets this: change it by a pull request to the definition's repository"
    )]
    DefinitionNamed,
    /// The Gate does not offer the answer.
    #[error("the gate offers {offered:?}")]
    AnswerNotOffered {
        /// The answers it offers.
        offered: Vec<String>,
    },
    /// The Run is Tampered, and the answer approving its Gate does not
    /// approve exactly the Control Files its branch changed unasked.
    #[error(
        "the run's branch changes control files its request does not ask for: approve them by listing them in approvedControlFiles"
    )]
    ControlFilesNotApproved,
    /// Someone who does not administer a team set its Control Files or its
    /// Standing Rules, or is on no team.
    #[error("only an admin of a team can change its factory settings")]
    NotATeamAdmin,
    /// Standing Rules were set for a Project no initiative has, or one the
    /// caller may not edit: the same answer, so a hidden one is not
    /// revealed.
    #[error("no project the caller can edit has this id")]
    UnknownProject,
    /// A list of Control File patterns does not hold together.
    #[error(transparent)]
    InvalidControlFiles(#[from] ControlFilesError),
    /// Temporal could not be reached or did not answer.
    #[error(transparent)]
    Upstream(#[from] upstream_error::UpstreamError),
    /// The document is not a task.
    #[error("the document is not a task")]
    NotATask,
    /// The user id names no account.
    #[error("no user has this id")]
    UnknownUser,
    /// A hand back came from outside a Step's agent session.
    #[error("only a factory step's agent session can hand back, from its code")]
    NotAStepSession,
    /// Someone other than the person in their own signed-in session
    /// answered a Gate or set their Control File patterns: an internal
    /// caller, an API key or token, or an agent's program acting for them.
    #[error(
        "only the person, signed in to Macro, can answer a run's gate or change the factory's settings"
    )]
    NotAPerson,
    /// A hand back came from someone other than the Run's owner.
    #[error("only the run's owner can hand back its step")]
    NotTheOwner,
    /// A hand back named an outcome its Step does not offer.
    #[error("the step ends with one of {offered:?}")]
    OutcomeNotOffered {
        /// The outcomes the Step offers.
        offered: Vec<String>,
    },
    /// A question is empty, or larger than a Gate's question keeps.
    #[error("a question must not be empty, and must be at most {max} bytes")]
    QuestionSize {
        /// The most bytes a question may hold.
        max: usize,
    },
    /// An answer to an agent's question carries no words in its note.
    #[error("an agent's question is answered in the note: it must not be empty")]
    AnswerMissing,
    /// A hand back is larger than a Step's output keeps.
    #[error("what is handed back must be at most {max} bytes")]
    HandBackTooLarge {
        /// The most bytes it may hold.
        max: usize,
    },
    /// The Step already ended.
    #[error("the step has ended")]
    StepEnded,
    /// The Run has ended, so it takes no answer and cannot be cancelled
    /// again.
    #[error("the run has ended")]
    RunEnded,
    /// A note to a Step's agent carries no words.
    #[error("a note must not be empty")]
    EmptyNote,
    /// Why a Run is flawed is empty, or larger than a Run keeps.
    #[error("why the run is flawed must not be empty, and must be at most {max} bytes")]
    FlawReasonSize {
        /// The most bytes a reason may hold.
        max: usize,
    },
    /// The Scorer marking a Run flawed is not named as a Flow names things.
    #[error(transparent)]
    InvalidScorer(InvalidName),
    /// The Run did not end `success`, so it cannot be flawed: it still
    /// runs, or ended another way.
    #[error("only a run that ended in success can be marked flawed")]
    NotSucceeded,
    /// The Step takes no steering now, and says why.
    #[error("the step takes no steering now: {}", steer_refusal_message(*.0))]
    NotSteerable(SteerRefusal),
    /// The Step's agent session did not take the owner's note or resume:
    /// the agent never read it.
    #[error("the step's agent session did not take it; try again")]
    SessionUnreachable,
    /// The note on an answer is larger than a Gate keeps.
    #[error("the note must be at most {max} bytes")]
    NoteTooLarge {
        /// The most bytes a note may hold.
        max: usize,
    },
    /// An agent answering for the Run's owner tried what the factory keeps
    /// for the owner in person.
    #[error(transparent)]
    ReservedForAPerson(#[from] PersonOnly),
    /// An agent acting for the Run's owner tried an act no Standing Role
    /// its bot fills allows, under a Factory Definition with Standing
    /// Roles.
    #[error("{}", standing_refusal(*act, allowing))]
    NotInStandingRole {
        /// The act it tried.
        act: StandingAct,
        /// The Standing Roles that allow the act.
        allowing: Vec<StandingRoleName>,
    },
    /// The person holds as many places in their Queue as the Queue limit of
    /// the rules a new Run of theirs would start under: Runs waiting on
    /// them at a Gate or a question, and Runs of a Flow with a Gate that
    /// have not reached one yet.
    #[error(
        "your queue is full: {waiting} of your runs wait on you at a gate or question and {starting} more have yet to ask you anything, which fills the queue limit of {limit} {} sets; answer some, or cancel a run you no longer need, before a new run starts; runs under way go on",
        queue_limit_source(*set_by)
    )]
    QueueFull {
        /// The Queue limit.
        limit: u16,
        /// Whose rules set it: the team's, or the Project's.
        set_by: RulesSource,
        /// The person's Runs waiting on them at a Gate or a question,
        /// counted up to the limit with `starting`.
        waiting: u16,
        /// The person's Runs that hold a place before their first Gate.
        starting: u16,
    },
    /// A Queue limit below the lowest or above the highest one a definition
    /// may set.
    #[error("the queue limit must be from {min} to {max}")]
    InvalidQueueLimit {
        /// The lowest Queue limit.
        min: u16,
        /// The highest Queue limit.
        max: u16,
    },
    /// A token budget below the smallest or above the largest one a
    /// definition may set.
    #[error("the token budget must be from {min} to {max}")]
    InvalidTokenBudget {
        /// The smallest token budget.
        min: u64,
        /// The largest token budget.
        max: u64,
    },
    /// A Run already runs on the task, so an Automation starts none.
    #[error("a run is already running on this task")]
    TaskBusy,
    /// The caller is on no team, which keeps Automations.
    #[error("only a member of a team can have automations")]
    NoTeam,
    /// The caller may not change the Automation: it is not theirs, and
    /// they do not administer its team.
    #[error("only an automation's creator or an admin of its team can change it")]
    NotTheAutomationOwner,
    /// The Automation asked for does not read.
    #[error(transparent)]
    InvalidAutomation(#[from] crate::domain::automations::AutomationRequestError),
    /// The firing does not read.
    #[error(transparent)]
    InvalidFiring(#[from] crate::domain::automations::FiringError),
    /// The scheduler refused the routine that would fire the Automation,
    /// and said why.
    #[error("the automation's trigger was refused: {0}")]
    TriggerRefused(String),
    /// The scheduler, which fires the Automation, could not be reached.
    #[error("the automation's trigger could not be set up; try again")]
    TriggerUnavailable,
    /// Only an Automation's routine fires it, from its code step, with the
    /// credential the scheduler signed for the routine.
    #[error("only an automation's routine fires it")]
    NotARoutine,
    /// A webhook call that is not signed with its Automation's secret, or
    /// whose timestamp is too far from now.
    #[error(
        "the call is not signed with the automation's secret, or its timestamp is not within five minutes of now"
    )]
    BadSignature,
    /// A firing with the same key was received before: a retry, or a
    /// replayed webhook call.
    #[error("this firing was received before")]
    FiringRepeated,
    /// The Run's workflow did not take the answer, and said why: it waits
    /// on no Gate of that Step, or the Gate was answered.
    #[error("the answer was refused: {0}")]
    AnswerRefused(String),
    /// Any other internal error.
    #[error("internal factory error: {0:?}")]
    Internal(rootcause::Report),
}

/// Why a Step takes no steering, for a person.
fn steer_refusal_message(refusal: SteerRefusal) -> &'static str {
    match refusal {
        SteerRefusal::Ended => "it has ended",
        SteerRefusal::NoSession => "no agent works on it in a session",
        SteerRefusal::NotWorking => "its agent works on no turn now",
        SteerRefusal::Paused => "it is paused; resume it first",
        SteerRefusal::NotPaused => "it is not paused",
        SteerRefusal::Stopping => "its owner asked it to stop",
    }
}

impl From<rootcause::Report> for FactoryError {
    fn from(report: rootcause::Report) -> Self {
        FactoryError::Internal(report)
    }
}

/// Errors of running a Step or recording a Run's end.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// The Step's Run is not recorded.
    #[error("the step's run does not exist")]
    NoSuchRun,
    /// The Step's Run ended: before the Step started, as a Run cancelled
    /// before its workflow did, or while it worked.
    #[error("the step's run has ended")]
    RunEnded,
    /// The Step produced more output than a Step keeps.
    #[error("the step's output must be at most {max} bytes")]
    OutputTooLarge {
        /// The most bytes an output may hold.
        max: usize,
    },
    /// The Step's work failed in a way trying again would repeat.
    #[error("the step's work failed for good: {0:?}")]
    Final(rootcause::Report),
    /// The Step's agent was still at work on its turn when the wait for it
    /// ended: a retry waits on the same turn. Past the Step's time it is the
    /// work's failure, not the factory's.
    #[error("the step's agent was still at work when the wait ended: {0:?}")]
    Overdue(rootcause::Report),
    /// The quota of the Agents that fill the Step's Role ran out: trying
    /// again would repeat the refusal, and the work never started.
    #[error("the step's agents are out of quota: {0:?}")]
    OutOfQuota(rootcause::Report),
    /// Any other internal error.
    #[error("internal step error: {0:?}")]
    Internal(rootcause::Report),
}

impl From<rootcause::Report> for StepError {
    fn from(report: rootcause::Report) -> Self {
        StepError::Internal(report)
    }
}
