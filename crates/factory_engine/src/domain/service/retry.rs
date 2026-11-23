//! Retrying a Run that ended `failed`, `infrastructure` or `blocked`: a new
//! Run of the same task, Flow and Factory Definition commit, on the branch
//! of the Run it retries, that keeps the Steps the retried Run took before
//! the entry it stopped at and starts its workflow there. The plan, the
//! branch and the review rounds already done are not done again.

use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::evaluation::{BENCHMARK_FLOW, EvaluationRepository};
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    EntryId, MAX_STEPS, NewRetry, QueueHold, RetryInsert, RunId, RunOutcome, Step,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::self_improvement::{IMPROVE_FLOW, SelfImprovementRepository};
use factory_records::domain::standing_acts::{StandingAct, StandingActRepository};
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;

use super::{FactoryServiceImpl, internal};
use crate::domain::built_in_flows::walked_flow;
use crate::domain::flow::Flow;
use crate::domain::models::{ActingAgent, FactoryError, RunDetail, RunStart, StartAt};
use crate::domain::ports::{
    PullRequests, RunWorkflows, StepExecutor, TaskDirectory, UserDirectory,
};
use crate::domain::token_budget::budget_of;

/// How a Run may end for a retry to start it again: on its work, a machine
/// or a limit, not on a person's word or a finished change.
pub(crate) fn retriable(outcome: RunOutcome) -> bool {
    matches!(
        outcome,
        RunOutcome::Failed | RunOutcome::Infrastructure | RunOutcome::Blocked
    )
}

/// Where a retry of a Run that took `steps` in `flow` starts: the entry of
/// its latest Step, and the place of that visit's first Step (a review
/// panel's first reviewer, or the Step an agent's question went on from),
/// so that visit runs again whole. The visit is the run of Steps right
/// before the latest, at consecutive places, of the same entry and visit
/// number: a visit number alone does not name a visit, since a reset
/// counts an entry's visits from 1 again. The Steps before it are kept. A
/// Run that took no Step starts again at the Flow's first entry, keeping
/// nothing. None when its latest Step's entry is not in the Flow.
pub(crate) fn retry_start(flow: &Flow, steps: &[Step]) -> Option<(EntryId, u16)> {
    let mut taken: Vec<&Step> = steps.iter().collect();
    taken.sort_by_key(|step| step.seq);
    let Some(latest) = taken.last() else {
        return flow.entries.first().map(|entry| (entry.id.clone(), 1));
    };
    if !flow.entries.iter().any(|entry| entry.id == latest.entry) {
        return None;
    }
    let mut first_seq = latest.seq;
    for step in taken.iter().rev().skip(1) {
        let same_visit = step.entry == latest.entry && step.visit == latest.visit;
        if !same_visit || step.seq.checked_add(1) != Some(first_seq) {
            break;
        }
        first_seq = step.seq;
    }
    Some((latest.entry.clone(), first_seq))
}

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + EvaluationRepository
        + StandingActRepository
        + SelfImprovementRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// Retry `actor`'s Run `id`, as [`crate::domain::ports::FactoryService::retry_run`]
    /// does.
    #[tracing::instrument(skip(self, actor), fields(retry_id = tracing::field::Empty))]
    pub(super) async fn retry(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        acting: Option<ActingAgent>,
        in_person: bool,
    ) -> Result<RunDetail, FactoryError> {
        let run = self.owned_run(actor, id).await?;
        // A retry starts a Run: an agent retries one under a Definition
        // with Standing Roles only when its bot fills one that allows a
        // start.
        let agents = self.acting_agents(id, acting).await?;
        let started_by = self
            .standing_actor(actor, agents.as_ref(), StandingAct::Start, acting)
            .await?;
        let Some(end) = &run.end else {
            return Err(FactoryError::NotRetriable);
        };
        if !retriable(end.outcome) {
            return Err(FactoryError::NotRetriable);
        }
        // A Benchmark trial and a self-improvement Run belong to their
        // suite and improvement, which start their own.
        if matches!(run.flow.as_deref(), Some(BENCHMARK_FLOW | IMPROVE_FLOW)) {
            return Err(FactoryError::NotRetriable);
        }
        // The retry walks the Flow the Run kept as it started; a Run kept
        // before Runs kept their Flow has none to walk again.
        let flow = self
            .repo
            .run_flow(id)
            .await
            .map_err(internal)?
            .filter(|kept| kept.definition.is_some())
            .and_then(|kept| walked_flow(id, kept))
            .ok_or(FactoryError::NotRetriable)?;
        let steps = self.repo.list_steps(id).await.map_err(internal)?;
        let (entry, first_seq) = retry_start(&flow, &steps).ok_or(FactoryError::NotRetriable)?;
        // A retry's Steps take places past the ones it keeps: a chain of
        // retries ends once the places run out.
        if first_seq.checked_add(MAX_STEPS).is_none() {
            return Err(FactoryError::NotRetriable);
        }

        // A retry is held to the Queue limit a Run its owner starts now
        // would be: a Run keeps its rules, not their limit.
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        let definition = self.team_definition(actor, team).await?;
        let rules = self
            .run_rules(team, run.project_id, definition.as_ref())
            .await?;
        let asking_flows = self.asking_flows();
        let hold = QueueHold {
            limit: rules.definition.queue_limit,
            asking_flows: &asking_flows,
        };
        // Whether the retry's request asks for the Control Files it names
        // is the retrier's to say, as a new Run's is its creator's: an
        // agent's account, or an agent acting, is never its owner in
        // person.
        let created_in_person = in_person
            && acting.is_none()
            && !self.repo.is_agent_account(actor).await.map_err(internal)?;
        let retry_id = RunId::generate();
        tracing::Span::current().record("retry_id", tracing::field::display(retry_id));
        let retry = NewRetry {
            id: retry_id,
            retried: id,
            first_seq,
            created_in_person,
        };
        let retried = match self.repo.retry_run(retry, hold).await.map_err(internal)? {
            RetryInsert::Inserted(retried) => *retried,
            RetryInsert::NotEnded => return Err(FactoryError::NotRetriable),
            RetryInsert::Retried => return Err(FactoryError::AlreadyRetried),
            RetryInsert::QueueFull(places) => {
                return Err(FactoryError::QueueFull {
                    limit: hold.limit.unwrap_or_default(),
                    set_by: rules.source,
                    waiting: places.waiting,
                    starting: places.starting,
                });
            }
        };

        // The agent that retried it is kept as the one that started it,
        // before its workflow starts.
        if let Some(started_by) = &started_by
            && let Err(error) = self
                .repo
                .record_standing_act(retry_id, StandingAct::Start, None, started_by)
                .await
        {
            self.forget_unstarted(retry_id).await;
            return Err(internal(error).into());
        }

        let start = RunStart {
            run_id: retry_id,
            flow,
            token_budget: budget_of(&retried).is_some(),
            start_at: Some(StartAt { entry, first_seq }),
        };
        if let Err(error) = self.workflows.start(&start).await {
            self.forget_unstarted(retry_id).await;
            return Err(error.into());
        }

        let kept = self.repo.list_steps(retry_id).await.map_err(internal)?;
        let rounds = self.repo.review_rounds(retry_id).await.map_err(internal)?;
        self.detail(retried, kept, rounds).await
    }
}
