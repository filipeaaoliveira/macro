//! The Steps the factory knows: `reply` acknowledges the Run's request;
//! `plan`, `write`, `review`, `fix` and `ci_fix` hand it to an agent; and
//! `publish`, `ci`, `ready_to_merge` and `merge` take the Run's branch to a
//! merged pull request on GitHub ([`PullRequestSteps`]); `update` brings a
//! branch behind its base up to date (`update`).
//! On a repository, `write` and `fix` then have the agent run the checks
//! the repository declares, in rounds (`checks_round`). `accept`, before
//! the merge, has a verifier check the change does what the Run's request
//! asks (`accept`). Before a person reads a plan, `plan_review` checks it
//! and has a plan reviewer read it, and `split` files the child tasks of a
//! plan too big for one Run (`plan_review`). A Run ends at its merge: `deploy` and `verify`, which
//! once followed it, run only for the Runs whose Flow still names them, and
//! end at once ([`past_merge_outcome`]). No new Flow may name them.

mod accept;
mod checks_round;
mod plan_review;
mod review_panel;
mod update;

pub(crate) use review_panel::review_outcomes;
#[cfg(test)]
mod test;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::{
    CheckRoundOutcome, GateKind, Outcome, Repository, Role, Step, StepKey, StepSession,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{ReviewRound, ReviewRoundRepository};
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::acceptance::plan_output;
use crate::domain::agents::{PLAN_REVIEWER, REVIEWER_ROLES, VERIFIER};
use crate::domain::checks::fence_for;
use crate::domain::flow::{StepName, StepUse};
use crate::domain::gate_owner::owner_notes_brief;
use crate::domain::models::{AgentTask, SessionSteer, StepInput, StepReport};
use crate::domain::no_result::retry_prompt;
use crate::domain::ports::{
    AgentFailure, ChildTasks, MainWatches, NoChildTasks, PullRequests, Retryable, StepAgent,
    StepExecutor,
};
use crate::domain::pull_request_steps::{PullRequestStepError, PullRequestSteps};
use crate::domain::questions::{
    SESSION_LOST_NOTE, answers_brief, ask_instructions, going_on_prompt,
};
use crate::domain::review_rounds::{blockers_brief, escape, fenced_review};
use crate::domain::setup::{RepositorySetup, SETUP_FILE};
use crate::domain::verification::{VerifyFile, VerifyFileError};

/// What a `deploy` or `verify` Step reports: a Run ends at its merge, and
/// one whose Flow still names them goes no further.
const ENDED_AT_MERGE: &str = "The change is merged, and the run ends at its merge: \
     the factory no longer waits for a deploy or checks the merged change.";

/// How an old `verify` Step ends a round of acceptance checks an earlier
/// attempt of it left open.
const ROUND_ENDED_AT_MERGE: &str =
    "The run ended at its merge: these checks of the merged change no longer run.";

/// The outcome a `deploy` or `verify` Step, of a Run whose Flow still goes
/// on past its merge, ends with at once: the one every such Flow routes to
/// the Run's `success`. `deploy`'s `off` ends the Run there; a Run already
/// in `verify` ends with its `pass`, as the first of those Flows has no
/// `off` from `verify`. None for any other Step.
pub(crate) fn past_merge_outcome(step: &str) -> Option<&'static str> {
    match step {
        "deploy" => Some("off"),
        "verify" => Some("pass"),
        _ => None,
    }
}

/// What the agent is told for a `plan` Step.
const PLAN_INSTRUCTIONS: &str = include_str!("built_in_steps/plan.md");

/// What the agent is told for a `plan` Step on a repository, after
/// [`PLAN_INSTRUCTIONS`]: to end the plan with its acceptance checks.
const PLAN_ACCEPTANCE_INSTRUCTIONS: &str = include_str!("built_in_steps/plan_acceptance.md");

/// What the agent is told for a `plan` Step on a repository, before
/// [`PLAN_ACCEPTANCE_INSTRUCTIONS`]: to end the plan with its size block.
const PLAN_SIZE_INSTRUCTIONS: &str = include_str!("built_in_steps/plan_size.md");

/// What the agent is told for a `write` Step.
const WRITE_INSTRUCTIONS: &str = include_str!("built_in_steps/write.md");

/// What the agent is told for a `review` Step of a Run started before its
/// Flow had a review panel.
const REVIEW_INSTRUCTIONS: &str = include_str!("built_in_steps/review.md");

/// What a reviewer of a review panel is told.
const REVIEW_PANEL_INSTRUCTIONS: &str = include_str!("built_in_steps/review_panel.md");

/// What each Role of the review panel looks at.
const REVIEW_FOCUS: [(&str, &str); 3] = [
    (
        "correctness-reviewer",
        include_str!("built_in_steps/review_focus/correctness.md"),
    ),
    (
        "scope-reviewer",
        include_str!("built_in_steps/review_focus/scope.md"),
    ),
    (
        "tests-reviewer",
        include_str!("built_in_steps/review_focus/tests.md"),
    ),
];

/// What the agent is told for a `fix` Step.
const FIX_INSTRUCTIONS: &str = include_str!("built_in_steps/fix.md");

/// What the agent is told for a `ci_fix` Step.
const CI_FIX_INSTRUCTIONS: &str = include_str!("built_in_steps/ci_fix.md");

/// Where an agent works when the Run works on a repository.
const REPOSITORY_INSTRUCTIONS: &str = include_str!("built_in_steps/repository.md");

/// How a worker does the built-in Step `step`, if it is one: what
/// [`BuiltInSteps`] serves, for a check that runs without a worker, as a
/// Factory Definition's.
pub fn built_in_step_use(step: &StepName) -> Option<StepUse> {
    let built_in = BuiltInStep::named(step)?;
    // A new Flow may not name the Steps past a merge: only a Run whose
    // Flow, in its workflow's input, still names them runs them, and
    // that Flow is never read again.
    if matches!(built_in, BuiltInStep::Deploy | BuiltInStep::Verify) {
        return None;
    }
    if built_in.agent_role().is_some() {
        Some(StepUse::Agent)
    } else {
        Some(StepUse::Work)
    }
}

/// One of the Steps [`BuiltInSteps`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltInStep {
    /// Acknowledges the request, to check the engine end to end.
    Reply,
    /// An agent plans the change the request asks for.
    Plan,
    /// An agent writes the change its plan describes.
    Write,
    /// An agent reviews the change on the Run's branch.
    Review,
    /// An agent changes what a review or a person asked of the change.
    Fix,
    /// An agent fixes what made the pull request's checks fail.
    CiFix,
    /// The Run's branch is proposed as a pull request.
    Publish,
    /// The pull request's checks are waited for.
    Ci,
    /// A person is shown the commit whose checks passed, to merge.
    ReadyToMerge,
    /// The commit a person agreed to is merged.
    Merge,
    /// The Run's base is merged into its branch, which lacks a commit of
    /// it: by GitHub, or by an agent that resolves their conflict; then the
    /// repository's checks run on it again.
    Update,
    /// A verifier checks the head of the Run's branch does what the
    /// request asks, before the merge.
    Accept,
    /// The factory checks the Run's plan, then a plan reviewer reads it,
    /// before a person does.
    PlanReview,
    /// The child tasks of a plan too big for one Run are filed.
    Split,
    /// Once waited for the merge to deploy. Only a Run started on a `code`
    /// Flow that named it reaches it, once merged: it ends `off` at once,
    /// which ends the Run `success`.
    Deploy,
    /// Once had a verifier check the merged change. Only a Run started on
    /// a `code` Flow that named it reaches it, once merged: it ends `pass`
    /// at once, which ends the Run `success`.
    Verify,
}

impl BuiltInStep {
    fn named(step: &StepName) -> Option<Self> {
        match step.as_str() {
            "reply" => Some(Self::Reply),
            "plan" => Some(Self::Plan),
            "write" => Some(Self::Write),
            "review" => Some(Self::Review),
            "fix" => Some(Self::Fix),
            "ci_fix" => Some(Self::CiFix),
            "publish" => Some(Self::Publish),
            "ci" => Some(Self::Ci),
            "ready_to_merge" => Some(Self::ReadyToMerge),
            "merge" => Some(Self::Merge),
            "update" => Some(Self::Update),
            "accept" => Some(Self::Accept),
            "plan_review" => Some(Self::PlanReview),
            "split" => Some(Self::Split),
            "deploy" => Some(Self::Deploy),
            "verify" => Some(Self::Verify),
            _ => None,
        }
    }

    /// The Role an agent fills to do it, when an agent does it, in a
    /// session to stop or let go of. A Run started before its Flow named
    /// Roles goes on with these.
    fn agent_role(self) -> Option<Role> {
        let role = match self {
            Self::Plan => "planner",
            Self::Write | Self::Update => "writer",
            Self::Review => "reviewer",
            Self::Fix | Self::CiFix => "fixer",
            Self::Accept | Self::Verify => VERIFIER,
            Self::PlanReview => PLAN_REVIEWER,
            Self::Reply
            | Self::Split
            | Self::Publish
            | Self::Ci
            | Self::ReadyToMerge
            | Self::Merge
            | Self::Deploy => {
                return None;
            }
        };
        Role::try_from(role.to_string()).ok()
    }
}

/// Why a built-in Step's work failed.
#[derive(Debug, thiserror::Error)]
pub enum BuiltInStepError {
    /// No built-in Step has this name.
    #[error("no built-in step is named {0}")]
    UnknownStep(StepName),
    /// The Step was handed to an agent, and no agent does it.
    #[error("no agent does step {0}")]
    NotAnAgentStep(StepName),
    /// The Step works on the Run's repository, and the Run has none.
    #[error("step {0} works on the run's repository, and the run has none")]
    NoRepository(StepName),
    /// The agent could not do the Step.
    #[error("the agent failed: {report:?}")]
    Agent {
        /// Why.
        report: rootcause::Report,
        /// Whether asking the agent again may succeed.
        retryable: bool,
        /// Whether the agent's quota ran out.
        out_of_quota: bool,
        /// Whether the agent was still at work when the wait for it ended.
        overdue: bool,
        /// Whether another attempt took the Step on to a later prompt.
        superseded: bool,
    },
    /// The agent's machine refused the Step's session, and said why.
    #[error("the agent's machine refused the session: {0}")]
    Refused(String),
    /// The agent ended the Step with an outcome the Step does not have.
    #[error("the agent ended step {step} with {outcome}, which it does not end with")]
    UnexpectedOutcome {
        /// The Step.
        step: StepName,
        /// The outcome the agent handed back.
        outcome: Outcome,
    },
    /// A pull request Step could not be done.
    #[error(transparent)]
    PullRequest(#[from] PullRequestStepError),
    /// The Step goes on from a question its Run's records show no answer
    /// to.
    #[error("step {0}'s question has no answer to go on with")]
    Unanswered(u16),
    /// The repository's setup does not read, failed, or cannot run where
    /// the agent works: the Step ends `failed` with this, for its owner.
    #[error("the repository's setup failed: {0}")]
    Setup(String),
    /// A child task could not be filed now: asking again may file it.
    #[error("a child task could not be filed: {0:?}")]
    ChildTask(rootcause::Report),
    /// Document storage made a child task under another id than asked:
    /// asking again would make another.
    #[error("{0}")]
    ChildTaskElsewhere(String),
}

impl BuiltInStepError {
    fn agent(error: impl std::error::Error + AgentFailure + Send + Sync + 'static) -> Self {
        if let Some(reason) = error.refused() {
            return BuiltInStepError::Refused(reason.to_owned());
        }
        BuiltInStepError::Agent {
            retryable: error.retryable(),
            out_of_quota: error.out_of_quota(),
            overdue: error.overdue(),
            superseded: error.superseded(),
            report: rootcause::Report::new(error).into_dynamic(),
        }
    }
}

impl Retryable for BuiltInStepError {
    fn retryable(&self) -> bool {
        match self {
            BuiltInStepError::Agent { retryable, .. } => *retryable,
            BuiltInStepError::PullRequest(error) => error.retryable(),
            BuiltInStepError::ChildTask(_) => true,
            BuiltInStepError::UnknownStep(_)
            | BuiltInStepError::NotAnAgentStep(_)
            | BuiltInStepError::NoRepository(_)
            | BuiltInStepError::UnexpectedOutcome { .. }
            | BuiltInStepError::Unanswered(_)
            | BuiltInStepError::ChildTaskElsewhere(_)
            | BuiltInStepError::Setup(_)
            | BuiltInStepError::Refused(_) => false,
        }
    }
}

impl AgentFailure for BuiltInStepError {
    fn out_of_quota(&self) -> bool {
        match self {
            BuiltInStepError::Agent { out_of_quota, .. } => *out_of_quota,
            BuiltInStepError::UnknownStep(_)
            | BuiltInStepError::NotAnAgentStep(_)
            | BuiltInStepError::NoRepository(_)
            | BuiltInStepError::UnexpectedOutcome { .. }
            | BuiltInStepError::PullRequest(_)
            | BuiltInStepError::Unanswered(_)
            | BuiltInStepError::ChildTask(_)
            | BuiltInStepError::ChildTaskElsewhere(_)
            | BuiltInStepError::Setup(_)
            | BuiltInStepError::Refused(_) => false,
        }
    }

    fn overdue(&self) -> bool {
        match self {
            BuiltInStepError::Agent { overdue, .. } => *overdue,
            BuiltInStepError::UnknownStep(_)
            | BuiltInStepError::NotAnAgentStep(_)
            | BuiltInStepError::NoRepository(_)
            | BuiltInStepError::UnexpectedOutcome { .. }
            | BuiltInStepError::PullRequest(_)
            | BuiltInStepError::Unanswered(_)
            | BuiltInStepError::ChildTask(_)
            | BuiltInStepError::ChildTaskElsewhere(_)
            | BuiltInStepError::Setup(_)
            | BuiltInStepError::Refused(_) => false,
        }
    }

    fn refused(&self) -> Option<&str> {
        match self {
            BuiltInStepError::Refused(reason) => Some(reason),
            _ => None,
        }
    }

    fn superseded(&self) -> bool {
        match self {
            BuiltInStepError::Agent { superseded, .. } => *superseded,
            BuiltInStepError::UnknownStep(_)
            | BuiltInStepError::NotAnAgentStep(_)
            | BuiltInStepError::NoRepository(_)
            | BuiltInStepError::UnexpectedOutcome { .. }
            | BuiltInStepError::PullRequest(_)
            | BuiltInStepError::Unanswered(_)
            | BuiltInStepError::ChildTask(_)
            | BuiltInStepError::ChildTaskElsewhere(_)
            | BuiltInStepError::Setup(_)
            | BuiltInStepError::Refused(_) => false,
        }
    }
}

fn outcomes(names: &[&str]) -> Vec<Outcome> {
    names
        .iter()
        .map(|name| Outcome::try_from((*name).to_string()).unwrap_or_else(|_| Outcome::failed()))
        .collect()
}

/// The built-in Steps, over the agent that does the agent Steps, the pull
/// request Steps, and where `split` files child tasks.
#[derive(Debug, Clone)]
pub struct BuiltInSteps<A, G, R, M, C = NoChildTasks> {
    agent: A,
    pull_requests: PullRequestSteps<G, R, M>,
    child_tasks: C,
}

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
{
    /// The built-in Steps, with `agent` doing the agent Steps, filing no
    /// child task until [`Self::with_child_tasks`].
    pub fn new(agent: A, pull_requests: PullRequestSteps<G, R, M>) -> Self {
        Self {
            agent,
            pull_requests,
            child_tasks: NoChildTasks,
        }
    }
}

impl<A, G, R, M, C> BuiltInSteps<A, G, R, M, C>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
    C: ChildTasks,
{
    /// The built-in Steps, with `split` filing child tasks in `tasks`.
    pub fn with_child_tasks<C2: ChildTasks>(self, tasks: C2) -> BuiltInSteps<A, G, R, M, C2> {
        BuiltInSteps {
            agent: self.agent,
            pull_requests: self.pull_requests,
            child_tasks: tasks,
        }
    }

    /// Hand `step` to an agent of the Role its entry names, else of `role`,
    /// with `instructions`, and check it ends with one of `outcomes`. When
    /// the Run works on a repository, the agent's session works in it, on
    /// the Run's branch.
    async fn ask_agent(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        self.ask_agent_by(step, instructions, outcomes, input, None)
            .await
    }

    /// [`Self::ask_agent`], waiting for the answer until `answer_by` at the
    /// latest.
    async fn ask_agent_by(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
        answer_by: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<StepReport, BuiltInStepError> {
        let mut outcomes = outcomes;
        if input.may_ask {
            outcomes.push(Outcome::question());
        }
        // A Step that goes on from its agent's question prompts the session
        // the agent asked in with the owner's answer, and one that goes on
        // from a Step whose agent stopped without a result with the owner's
        // ask to try again: the agent has the Step's instructions and its
        // work so far there.
        let role = agent_role(step, input.role)
            .ok_or_else(|| BuiltInStepError::NotAnAgentStep(step.clone()))?;
        let asked_prompt = match input.resumes {
            Some(asked) => {
                let answered = going_on_prompt(input.earlier, asked, input.questions_left)
                    .or_else(|| retry_prompt(input.earlier, asked))
                    .ok_or(BuiltInStepError::Unanswered(asked))?;
                format!("{answered}{}", owner_notes_brief(input.owner_notes))
            }
            None => prompt(instructions, &outcomes, input, reads_only(step)),
        };
        let setup = self.setup(input).await?;
        let asked_prompt = match &setup {
            Some(setup) if input.resumes.is_none() => {
                format!("{asked_prompt}{}", setup_note(setup))
            }
            _ => asked_prompt,
        };
        let task = AgentTask {
            key: input.key,
            owner: input.owner.clone(),
            step: step.clone(),
            prompt: asked_prompt,
            outcomes: outcomes.clone(),
            repo_url: input.repository.map(|repository| repository.url.clone()),
            branch_of: input.retry.map(|retry| retry.branch_of),
            answer_by,
            resumes: input.resumes,
            // Should the asking Step's session be gone, a new one is told
            // the whole Step, the owner's answers in it, and that its
            // earlier session's work may be lost.
            afresh: input.resumes.map(|_| {
                format!(
                    "{}{}{}",
                    prompt(instructions, &outcomes, input, reads_only(step)),
                    setup.as_ref().map(setup_note).unwrap_or_default(),
                    SESSION_LOST_NOTE
                )
            }),
            setup,
            role,
        };
        let answer = match self.agent.answer(task).await {
            Ok(answer) => answer,
            // An agent that stopped without a result, even once reminded,
            // ends the Step `no_result`, its last message the output, for
            // the workflow to ask the Run's owner. A reviewer's fails the
            // Step as before: its review round asks no one.
            Err(error) => {
                if let Some(report) = error.setup_failed() {
                    return Err(BuiltInStepError::Setup(report.to_string()));
                }
                if !input.reviewer
                    && let Some(last_words) = error.stopped_without_result()
                {
                    return Ok(StepReport {
                        outcome: Outcome::no_result(),
                        output: last_words.to_string(),
                    });
                }
                return Err(BuiltInStepError::agent(error));
            }
        };

        if !outcomes.contains(&answer.outcome) {
            return Err(BuiltInStepError::UnexpectedOutcome {
                step: step.clone(),
                outcome: answer.outcome,
            });
        }
        Ok(StepReport {
            outcome: answer.outcome,
            output: answer.text,
        })
    }

    /// The setup the Run's base declares for its repository, which each
    /// new agent session of the Step runs before its first prompt: none
    /// when the Run works on no repository, or the base declares none or an
    /// empty one. A file that does not read fails the Step, saying why.
    async fn setup(
        &self,
        input: StepInput<'_>,
    ) -> Result<Option<RepositorySetup>, BuiltInStepError> {
        if input.repository.is_none() {
            return Ok(None);
        }
        let Some(text) = self.pull_requests.declared_setup(&input).await? else {
            return Ok(None);
        };
        let setup = RepositorySetup::parse(&text).map_err(|error| {
            BuiltInStepError::Setup(format!(
                "The repository's setup could not run, so the agent did not start: {error}"
            ))
        })?;
        Ok(Some(setup).filter(|setup| !setup.is_empty()))
    }

    /// [`Self::ask_agent`] for a Step that only makes sense on the Run's
    /// repository.
    async fn ask_agent_in_repository(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        if input.repository.is_none() {
            return Err(BuiltInStepError::NoRepository(step.clone()));
        }
        self.ask_agent(step, instructions, outcomes, input).await
    }
}

impl<A, G, R, M, C> StepExecutor for BuiltInSteps<A, G, R, M, C>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
    C: ChildTasks,
{
    type Err = BuiltInStepError;

    fn serves(&self, step: &StepName) -> Option<StepUse> {
        built_in_step_use(step)
    }

    async fn run(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        // A setup that does not read, fails or cannot run ends the Step
        // `failed`, with what its owner needs to see, rather than a
        // failure's fixed output; no agent turn was spent on it.
        match self.run_built_in(step, input).await {
            Err(BuiltInStepError::Setup(output)) => Ok(StepReport {
                outcome: Outcome::failed(),
                output,
            }),
            ran => ran,
        }
    }

    async fn stop(
        &self,
        step: &StepName,
        role: Option<&Role>,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), BuiltInStepError> {
        match agent_role(step, role) {
            Some(role) => self
                .agent
                .stop(key, &role, owner)
                .await
                .map_err(BuiltInStepError::agent),
            // A reply is done before anything could stop it, and a pull
            // request Step holds nothing: a cancelled one is dropped.
            None => Ok(()),
        }
    }

    async fn release(
        &self,
        step: &StepName,
        role: Option<&Role>,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), BuiltInStepError> {
        match agent_role(step, role) {
            Some(role) => self
                .agent
                .release(key, &role, owner)
                .await
                .map_err(BuiltInStepError::agent),
            // Only an agent Step holds a session.
            None => Ok(()),
        }
    }

    async fn steer(
        &self,
        role: &Role,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        steer: SessionSteer,
    ) -> Result<(), BuiltInStepError> {
        self.agent
            .steer(key, role, owner, session, steer)
            .await
            .map_err(BuiltInStepError::agent)
    }
}

impl<A, G, R, M, C> BuiltInSteps<A, G, R, M, C>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
    C: ChildTasks,
{
    async fn run_built_in(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let built_in =
            BuiltInStep::named(step).ok_or_else(|| BuiltInStepError::UnknownStep(step.clone()))?;
        match built_in {
            BuiltInStep::Reply => Ok(StepReport {
                outcome: Outcome::done(),
                output: format!("Received the request: {}", input.request),
            }),
            // On a repository, the plan ends with its size block, which
            // `plan_review` checks, and its acceptance checks, which the
            // Step keeps written out for the person at its Gate.
            BuiltInStep::Plan if input.repository.is_some() => {
                let file = self.pull_requests.base_verify_file(&input).await?;
                let instructions = format!(
                    "{PLAN_INSTRUCTIONS}{PLAN_SIZE_INSTRUCTIONS}{PLAN_ACCEPTANCE_INSTRUCTIONS}{}",
                    verify_note(&file)
                );
                let plan = self
                    .ask_agent(step, &instructions, vec![Outcome::done()], input)
                    .await?;
                if plan.outcome != Outcome::done() {
                    return Ok(plan);
                }
                Ok(StepReport {
                    output: plan_output(&plan.output),
                    ..plan
                })
            }
            BuiltInStep::Plan => {
                self.ask_agent(step, PLAN_INSTRUCTIONS, vec![Outcome::done()], input)
                    .await
            }
            BuiltInStep::Write => {
                let outcomes = vec![Outcome::done(), Outcome::unchanged()];
                if input.repository.is_some() {
                    self.work_and_check(step, WRITE_INSTRUCTIONS, outcomes, input, None, true)
                        .await
                } else {
                    self.ask_agent(step, WRITE_INSTRUCTIONS, outcomes, input)
                        .await
                }
            }
            BuiltInStep::Review => match input.review {
                Some(round) if input.reviewer || in_round(round, input.key.seq) => {
                    self.review_in_round(step, round, input).await
                }
                _ => {
                    let outcomes = outcomes(&["pass", "changes"]);
                    self.ask_agent_in_repository(step, REVIEW_INSTRUCTIONS, outcomes, input)
                        .await
                }
            },
            BuiltInStep::Fix => {
                if input.repository.is_none() {
                    return Err(BuiltInStepError::NoRepository(step.clone()));
                }
                let blockers = input
                    .review
                    .filter(|round| round.blockers().next().is_some());
                let instructions = fix_instructions(blockers);
                self.work_and_check(
                    step,
                    &instructions,
                    vec![Outcome::done()],
                    input,
                    blockers,
                    true,
                )
                .await
            }
            BuiltInStep::CiFix => {
                let outcomes = vec![Outcome::done()];
                self.ask_agent_in_repository(step, CI_FIX_INSTRUCTIONS, outcomes, input)
                    .await
            }
            BuiltInStep::Publish => Ok(self.pull_requests.publish(input).await?),
            BuiltInStep::Ci => Ok(self.pull_requests.ci(input).await?),
            BuiltInStep::ReadyToMerge => Ok(self.pull_requests.ready_to_merge(input).await?),
            BuiltInStep::Merge => Ok(self.pull_requests.merge(input).await?),
            BuiltInStep::Update => self.update_branch(step, input).await,
            BuiltInStep::Accept => self.accept_change(step, input).await,
            BuiltInStep::PlanReview => self.review_plan(step, input).await,
            BuiltInStep::Split => self.split_task(input).await,
            BuiltInStep::Deploy | BuiltInStep::Verify => {
                // An earlier attempt of an old `verify` may have left its
                // round of acceptance checks open: it never ends now.
                let repo = self.pull_requests.records();
                let rounds = repo
                    .check_rounds(input.key)
                    .await
                    .map_err(checks_round::records)?;
                for round in rounds.iter().filter(|round| round.end.is_none()) {
                    repo.end_check_round(
                        input.key,
                        round.round,
                        CheckRoundOutcome::Failed,
                        ROUND_ENDED_AT_MERGE,
                    )
                    .await
                    .map_err(checks_round::records)?;
                }
                Ok(StepReport {
                    outcome: accept::outcome(past_merge_outcome(step.as_str()).unwrap_or("off")),
                    output: ENDED_AT_MERGE.to_string(),
                })
            }
        }
    }
}

/// What the agent is told of the repository's `setup`, which ran in its
/// checkout before its first prompt: the commands, and the values they ran
/// with, which its own commands may need too.
fn setup_note(setup: &RepositorySetup) -> String {
    let mut note = String::from("\n");
    let names: Vec<&str> = setup
        .setup
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    if !names.is_empty() {
        note.push_str(&format!(
            "The factory ran the repository's setup (`{SETUP_FILE}`) in your checkout before this prompt: {}.\n",
            names.join(", ")
        ));
    }
    if !setup.env.is_empty() {
        let exports = setup.exports();
        let fence = fence_for(&exports);
        let said = if names.is_empty() {
            format!("The repository's setup (`{SETUP_FILE}`) gives these values")
        } else {
            "It ran with these values".to_string()
        };
        note.push_str(&format!(
            "{said}; run your own commands with them too:\n\n{fence}sh\n{exports}{fence}\n"
        ));
    }
    note
}

/// What the planner is told when the repository's verify file turns
/// acceptance off, or does not read; nothing otherwise.
fn verify_note(file: &Result<VerifyFile, VerifyFileError>) -> String {
    match file {
        Ok(file) if !file.verify => {
            "\nThe repository turns its verify phase off: the factory runs \
             none of these checks, but write them for the person who reads the plan.\n"
                .to_string()
        }
        Ok(_) => String::new(),
        Err(error) => format!("\n{}: the change is checked.\n", escape(&error.to_string())),
    }
}

/// The Role an agent fills to do `step`: the one its entry names, else the
/// one the built-in Step implies. None when no agent does it.
fn agent_role(step: &StepName, named: Option<&Role>) -> Option<Role> {
    let implied = BuiltInStep::named(step)?.agent_role()?;
    Some(named.cloned().unwrap_or(implied))
}

/// Whether the Step at `seq` is one of `round`'s reviewers.
fn in_round(round: &ReviewRound, seq: u16) -> bool {
    round.step_seqs().contains(&seq)
}

/// What the fix is told: with the Blockers of the latest review round,
/// `blockers`, when it found some, and how to say what it did of each.
fn fix_instructions(blockers: Option<&ReviewRound>) -> String {
    match blockers {
        Some(round) => format!("{FIX_INSTRUCTIONS}\n{}", blockers_brief(round)),
        None => FIX_INSTRUCTIONS.to_string(),
    }
}

/// The most bytes of the Run's earlier Steps a prompt carries.
const HISTORY_BUDGET: usize = 48 * 1024;

/// The agent's prompt: the Step's instructions, the outcomes it may end
/// with, how to ask the Run's owner when it may, where it works when the
/// Run works on a repository ([`workplace`]), the Run's request, the
/// questions the owner answered ([`answers_brief`]), the notes the owner
/// left on the Run's task since its latest Gate ([`owner_notes_brief`]),
/// then the Run's earlier Steps ([`history`]).
fn prompt(
    instructions: &str,
    outcomes: &[Outcome],
    input: StepInput<'_>,
    reads_only: bool,
) -> String {
    let outcomes = outcomes
        .iter()
        .map(|outcome| format!("`{outcome}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let ask = if input.may_ask {
        ask_instructions(input.questions_left)
    } else {
        String::new()
    };
    let workplace = input
        .repository
        .map(|repository| workplace(repository, &input.branch(), input.base_commit))
        .unwrap_or_default();
    // Only an agent that changes its checkout is told what the failed Step
    // may have left in it: one that reads only keeps, finishes or discards
    // nothing.
    let workplace = if input.repository.is_some() && input.resumes_failed_run() && !reads_only {
        format!("{workplace}{RESUMED_RUN_NOTE}")
    } else {
        workplace
    };
    format!(
        "{instructions}\nEnd the step with one of these outcomes: {outcomes}.\n{ask}{workplace}\n\
         ## Request\n\n{}\n{}{}{}",
        input.request,
        answers_brief(input.earlier),
        owner_notes_brief(input.owner_notes),
        history(input.earlier)
    )
}

/// What the first Step of a retry is told of its checkout: the retried
/// Run's Step failed in it, and may have left unfinished changes behind.
pub(crate) const RESUMED_RUN_NOTE: &str = "\nThis run retries a run that failed at this step: \
the branch already holds what that run's earlier steps pushed, and your working tree may \
still hold the failed step's unfinished, uncommitted changes. Run `git status` and \
`git log` first, and keep, finish or discard those changes on purpose before you go on.\n";

/// Whether the agent doing `step` only reads its checkout, as the Step's
/// instructions say, whatever Role its entry names: the plan, the plan
/// review, a review (alone or on a panel), and the verifier's checks
/// change, commit and push nothing.
fn reads_only(step: &StepName) -> bool {
    matches!(
        BuiltInStep::named(step),
        Some(
            BuiltInStep::Plan
                | BuiltInStep::PlanReview
                | BuiltInStep::Review
                | BuiltInStep::Accept
                | BuiltInStep::Verify
        )
    )
}

/// Where the agent works: a checkout of the Run's repository, on the Run's
/// branch `branch`, made from `base_commit` when the Run pins one, else
/// from its base branch.
fn workplace(repository: &Repository, branch: &str, base_commit: Option<&str>) -> String {
    let start = match base_commit {
        Some(commit) => format!("commit `{commit}` (fetch it if your checkout lacks it)"),
        None => format!("`origin/{}`", repository.base_branch),
    };
    format!(
        "\n{}",
        REPOSITORY_INSTRUCTIONS
            .replace("{branch}", branch)
            .replace("{start}", &start)
            .replace("{base}", &repository.base_branch)
            .replace("{repository}", &repository.url)
    )
}

/// What the Run's earlier Steps produced and how a person answered their
/// Gates, so a later Step reads the plan it carries out and what people said
/// about it: the latest visit of each entry, newest first, as many as fit
/// in [`HISTORY_BUDGET`]; of a review panel's entry, its latest Step of each
/// Role. A loop that visits an entry again and again adds
/// nothing but its latest round. The newest Step is never left out: when it
/// alone is past the budget, its end is cut. A person's answers to the
/// Flow's Gates of the Steps left out follow, newest first, as many as fit:
/// a note sent with a plan's `revise` stays in the planner's prompt after a
/// review sends the plan back again.
fn history(earlier: &[Step]) -> String {
    let mut latest: Vec<&Step> = Vec::new();
    for step in earlier.iter().rev() {
        let kept = latest
            .iter()
            .any(|kept| kept.entry == step.entry && kept.role == step.role);
        if step.end.is_some() && !kept {
            latest.push(step);
        }
    }

    let latest_seqs: Vec<u16> = latest.iter().map(|step| step.seq).collect();
    let mut history = String::new();
    let mut left_out = 0;
    for step in latest {
        let section = section(step);
        if history.len() + section.len() <= HISTORY_BUDGET {
            history.push_str(&section);
        } else if history.is_empty() {
            history.push_str(cut(&section, HISTORY_BUDGET - CUT_NOTE.len()));
            history.push_str(CUT_NOTE);
        } else {
            left_out += 1;
        }
    }

    let answers = earlier_answers(earlier, &latest_seqs);
    if !answers.is_empty() {
        let heading = "\n## Earlier answers at the run's gates\n\n";
        let mut section = String::from(heading);
        let mut answers_left_out = 0;
        for answer in answers {
            if history.len() + section.len() + answer.len() <= HISTORY_BUDGET {
                section.push_str(&answer);
            } else {
                answers_left_out += 1;
            }
        }
        if section.len() > heading.len() {
            history.push_str(&section);
        }
        left_out += answers_left_out;
    }
    if left_out > 0 {
        history.push_str(&format!(
            "\n({left_out} earlier steps are left out: the prompt has no room for them.)\n"
        ));
    }
    history
}

/// The longest note of an earlier answer a prompt carries, in bytes.
const ANSWER_NOTE_BUDGET: usize = 2048;

/// How a person answered the Flow's Gates of the `earlier` Steps not in
/// `shown`, newest first, one line each: their words are escaped, as in a
/// Step's section.
fn earlier_answers(earlier: &[Step], shown: &[u16]) -> Vec<String> {
    earlier
        .iter()
        .rev()
        .filter(|step| !shown.contains(&step.seq))
        .filter_map(|step| {
            let gate = step
                .gate
                .as_ref()
                .filter(|gate| gate.kind == GateKind::Flow)?;
            let answer = gate.answer.as_ref()?;
            let note = answer
                .note
                .as_deref()
                .map_or("(no note)".to_string(), |note| {
                    escape(cut(note, ANSWER_NOTE_BUDGET)).replace('\n', " ")
                });
            Some(format!(
                "- Step {}, visit {}: a person was asked \"{}\" and answered `{}`: {note}\n",
                step.entry,
                step.visit,
                escape(&gate.question).replace('\n', " "),
                answer.answer
            ))
        })
        .collect()
}

/// What ends a section cut to fit the prompt.
const CUT_NOTE: &str = "\n\n(The rest of this step is left out: the prompt has no room for it.)\n";

/// The longest start of `text` at most `max` bytes long, cut between two
/// characters.
pub(crate) fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One earlier Step: its outcome and output, and its Gate's answer.
fn section(step: &Step) -> String {
    let Some(end) = &step.end else {
        return String::new();
    };
    // A review panel's reviewer, or a verifier, wrote its output with what
    // the change's own checks printed, and a plan reviewer about a plan:
    // it is data, fenced as the briefs fence the findings, never
    // instructions.
    let reviewed = step.role.as_ref().is_some_and(|role| {
        REVIEWER_ROLES.contains(&role.as_str())
            || role.as_str() == VERIFIER
            || role.as_str() == PLAN_REVIEWER
    });
    let output = if reviewed {
        fenced_review(&end.output)
    } else {
        end.output.clone()
    };
    let mut section = format!(
        "\n## Step {}, visit {}: {}\n\n{output}\n",
        step.entry, step.visit, end.outcome
    );

    let answer = step
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref().map(|answer| (&gate.question, answer)));
    // An agent may have asked the question, and the note is a person's
    // words: both are escaped, as findings are, so neither reads as a
    // heading or opens a block.
    if let Some((question, answer)) = answer {
        let note = answer
            .note
            .as_deref()
            .map_or("(no note)".to_string(), escape);
        section.push_str(&format!(
            "\nA person was asked \"{}\" and answered `{}`: {note}\n",
            escape(question),
            answer.answer
        ));
    }
    section
}
