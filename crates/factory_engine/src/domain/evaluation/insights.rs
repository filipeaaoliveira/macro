//! Evaluation insights: the live Scorers' judgments of a person's Runs over
//! a period, how each Experiment's arms did, and their Benchmark suites.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use factory_records::domain::evaluation::{
    AgentChoices, BENCHMARK_FLOW, EvaluationRecords, EvaluationRepository, MAX_OPEN_TRIALS,
    NewSuite, SuiteInsert, TrialKey,
};
use factory_records::domain::models::{RunId, RunOutcome, TeamMembership};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::self_improvement::{
    DefinitionNaming, Improvement, ImprovementState, SelfImprovementRepository, UndoDismissal,
};
use macro_user_id::user_id::MacroUserIdStr;
use serde::Serialize;
use uuid::Uuid;

use super::{EvaluationDefinition, SelfImprovementDefinition, check_choices};
use crate::domain::insights::{InsightsError, InsightsQuery, TokenCounts};
use crate::domain::models::{ActingAgent, FactoryError};
use crate::domain::ports::FactoryService;
use crate::domain::self_improvement::{NamerStanding, namer_standing};

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

/// The Agents a configuration gives Roles, by name.
fn named_agents(agents: &AgentChoices) -> BTreeMap<String, String> {
    agents
        .iter()
        .map(|(role, agent)| (role.as_str().to_string(), agent.as_str().to_string()))
        .collect()
}

/// A Scorer and its live judgments of the period's Runs.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScorerInsight {
    /// Its name.
    pub name: String,
    /// What it judges; none for a Scorer no longer defined whose
    /// judgments remain.
    pub instructions: Option<String>,
    /// The Flows whose Runs it judges; every Flow when empty.
    pub flows: Vec<String>,
    /// The percentage of those Runs it judges; none when no longer defined.
    pub sample_percent: Option<u8>,
    /// How many of the period's Runs it judged.
    pub judged: u32,
    /// How many of them scored half or more.
    pub passed: u32,
    /// Their mean score, from 0 to 1; none when it judged none.
    pub mean_score: Option<f64>,
}

/// One Scorer's judgments of one arm's Runs.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmScoreInsight {
    /// The Scorer.
    pub scorer: String,
    /// How many of the arm's Runs it judged.
    pub judged: u32,
    /// Their mean score, from 0 to 1.
    pub mean_score: f64,
}

/// How one arm's Runs did.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmInsight {
    /// Its name.
    pub name: String,
    /// The Agents it gives Roles, by Role; empty for the arm that keeps the
    /// agent definition's.
    pub agents: BTreeMap<String, String>,
    /// How many of the period's Runs took it.
    pub runs: u32,
    /// How many of them ended, but for those that ended `infrastructure`:
    /// their work never ran.
    pub ended: u32,
    /// How many ended `success`.
    pub succeeded: u32,
    /// How many had a Step the arm's Agent could not start, which another
    /// Agent of its Role took: their results are not the arm's alone.
    pub fell_back: u32,
    /// The Scorers' judgments of its Runs.
    pub scores: Vec<ArmScoreInsight>,
}

/// An Experiment and how its arms did over the period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentInsight {
    /// Its name.
    pub name: String,
    /// The Flow whose Runs it splits; none for one no longer defined.
    pub flow: Option<String>,
    /// Whether it still assigns new Runs.
    pub running: bool,
    /// Its arms, as defined, then any no longer defined that took Runs.
    pub arms: Vec<ArmInsight>,
}

/// A Benchmark one may start a suite of.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSummary {
    /// Its name.
    pub name: String,
    /// Its frozen tasks, by name.
    pub tasks: Vec<String>,
    /// The Scorers that judge each trial.
    pub scorers: Vec<String>,
}

/// A Scorer's judgment of one trial.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrialScore {
    /// The Scorer.
    pub scorer: String,
    /// From 0 to 1.
    pub score: f64,
}

/// One trial of a suite.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrialInsight {
    /// The frozen task.
    pub task: String,
    /// Its Run.
    pub run_id: RunId,
    /// How its Run ended; none while it goes.
    pub run_outcome: Option<RunOutcome>,
    /// Its mechanical score, from 0 to 1: 1 when its checks went green, 0.5
    /// when they stayed red, 0 otherwise; none until its Run ended, and
    /// none for a Run that ended `infrastructure`, whose work never ran.
    pub score: Option<f64>,
    /// The tokens its agents used; none until its Run ended, and none for a
    /// Run that ended `infrastructure`.
    pub tokens: Option<TokenCounts>,
    /// Its Benchmark's Scorers' judgments.
    pub scores: Vec<TrialScore>,
}

/// One suite: a Benchmark replayed on one configuration.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuiteInsight {
    /// The suite.
    pub id: Uuid,
    /// The Benchmark it replayed.
    pub benchmark: String,
    /// The Agents its configuration gave Roles, by Role.
    pub agents: BTreeMap<String, String>,
    /// When it started.
    pub created_at: DateTime<Utc>,
    /// Its trials, by task.
    pub trials: Vec<TrialInsight>,
    /// How many trials have their result. A trial whose Run ended
    /// `infrastructure` never had its work run, so it is never among them.
    pub scored: u32,
    /// How many trials' Runs ended `infrastructure`: left out of the
    /// suite's score and tokens rather than counted against its
    /// configuration.
    pub infrastructure: u32,
    /// Their mean score; none until one has.
    pub mean_score: Option<f64>,
    /// The tokens of the trials that have their result.
    pub tokens: TokenCounts,
}

/// The evaluation insights of a period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationInsights {
    /// The period's first day.
    pub from: NaiveDate,
    /// Its last day, included.
    pub to: NaiveDate,
    /// The time zone its days are in.
    pub time_zone: String,
    /// The Flow the Runs were read for; none for every Flow.
    pub flow: Option<String>,
    /// Each Scorer, defined or with judgments in the period.
    pub scorers: Vec<ScorerInsight>,
    /// Each Experiment, defined or with Runs in the period.
    pub experiments: Vec<ExperimentInsight>,
    /// The Benchmarks one may start a suite of.
    pub benchmarks: Vec<BenchmarkSummary>,
    /// The suites started in the period, newest first.
    pub suites: Vec<SuiteInsight>,
    /// The caller's team's self-improvement: its pending failures and its
    /// latest self-improvement Run; none when the team names no Factory
    /// Definition.
    pub self_improvement: Option<SelfImprovementInsight>,
}

/// How many of a team's pending failures come from one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingSource {
    /// The Scorer's name, or `flawed` for the Runs marked flawed.
    pub source: String,
    /// How many.
    pub count: u32,
}

/// A team's latest self-improvement Run.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImprovementInsight {
    /// Its id, which undoing its dismissal names.
    pub id: Uuid,
    /// Its Run; none while it starts, when it was refused, or once the
    /// Run is gone. Only its owner reads it.
    pub run_id: Option<RunId>,
    /// Where it is.
    pub state: ImprovementState,
    /// Why its Run could not start, when refused.
    pub refusal: Option<String>,
    /// How many failures it groups.
    pub failures: u32,
    /// When it began.
    pub created_at: DateTime<Utc>,
    /// When it was settled.
    pub settled_at: Option<DateTime<Utc>>,
    /// How many of its failures are dismissed: a person rejected it (closed
    /// its pull request unmerged, or cancelled its Run), so they count
    /// toward no other Run until the dismissal is undone.
    pub dismissed: u32,
    /// When the dismissal of its failures was undone, sending them back to
    /// pending; none while it was not.
    pub dismissal_undone_at: Option<DateTime<Utc>>,
    /// Whether the caller may undo the dismissal now: it holds dismissed
    /// failures, was not undone, and they are a person (never an agent's
    /// account) who named the Definition and is still an admin of the
    /// team, or who owns its Run.
    pub can_undo_dismissal: bool,
}

/// What undoing a rejected self-improvement Run's dismissal did.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DismissalUndoneResponse {
    /// How many failures went back to pending: none when it was undone
    /// already.
    pub restored: u32,
    /// When it was undone, the first time.
    pub undone_at: DateTime<Utc>,
}

/// Why undoing a self-improvement Run's dismissal was refused.
#[derive(Debug, thiserror::Error)]
pub enum UndoDismissalError {
    /// The caller's team has no self-improvement Run of that id.
    #[error("no self-improvement run of your team has this id")]
    NotFound,
    /// The caller is neither the admin who named the team's Definition (a
    /// namer no longer an admin is not) nor the Run's owner.
    #[error(
        "only the admin who named the factory definition, or the owner of the self-improvement run, can undo its dismissal"
    )]
    NotAllowed,
    /// The caller is not the person in their own signed-in session, or
    /// signs in as an agent's account.
    #[error("only the person, signed in to Macro, can undo a dismissal")]
    NotAPerson,
    /// It holds no dismissed failure: no person rejected it, or it is open.
    #[error("the self-improvement run has no dismissed failures: no person rejected it")]
    NothingDismissed,
    /// Anything else.
    #[error("undoing the dismissal failed: {0:?}")]
    Internal(rootcause::Report),
}

fn undo_internal(error: impl std::error::Error + Send + Sync + 'static) -> UndoDismissalError {
    UndoDismissalError::Internal(internal(error))
}

/// Whether `actor`, a person on the team (`membership`), may undo the
/// dismissal of `improvement`'s failures: they named the team's Definition
/// (`naming`) and are still an admin of the team, as a self-improvement Run
/// starts only for such a namer ([`namer_standing`]), or they own its Run,
/// whoever may have rejected it in Macro. The caller refuses an agent's
/// account first.
async fn may_undo<R>(
    repo: &R,
    actor: &MacroUserIdStr<'static>,
    membership: &TeamMembership,
    naming: Option<&DefinitionNaming>,
    improvement: &Improvement,
) -> Result<bool, rootcause::Report>
where
    R: SelfImprovementRepository,
{
    if membership.admin && naming.is_some_and(|naming| naming.named_by.as_ref() == Some(actor)) {
        return Ok(true);
    }
    let Some(run_id) = improvement.run_id else {
        return Ok(false);
    };
    let facts = repo.failure_facts(run_id).await.map_err(internal)?;
    Ok(facts.is_some_and(|facts| &facts.user_id == actor))
}

/// A team's self-improvement against its Factory Definition: the failures
/// waiting for a Run, what starts one, and the latest one.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SelfImprovementInsight {
    /// The Definition's repository.
    pub repo_url: String,
    /// Whether the Definition improves itself.
    pub enabled: bool,
    /// How many pending failures start a Run.
    pub failures_threshold: u16,
    /// How many days old the oldest pending failure is before a Run starts
    /// with fewer.
    pub oldest_days: u16,
    /// Where the admin who named the Definition stands: its
    /// self-improvement Runs are theirs, and none starts unless they are
    /// still an admin of the team.
    pub namer: NamerStanding,
    /// How many failures wait.
    pub pending: u32,
    /// When the oldest was kept.
    pub oldest_pending_at: Option<DateTime<Utc>>,
    /// How many wait from each source, most first.
    pub pending_by_source: Vec<PendingSource>,
    /// The latest self-improvement Run, open or not.
    pub last: Option<ImprovementInsight>,
}

/// A suite that started.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuiteStarted {
    /// The suite.
    pub id: Uuid,
    /// Its trials' Runs, by task.
    pub runs: BTreeMap<String, RunId>,
    /// The task whose Run was refused, and why: the suite stopped there,
    /// and the trials before it go on. None when every trial started.
    pub refused: Option<RefusedTrial>,
}

/// A trial whose Run was refused.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefusedTrial {
    /// The frozen task.
    pub task: String,
    /// Why its Run was refused.
    pub reason: String,
}

/// Errors of starting a suite.
#[derive(Debug, thiserror::Error)]
pub enum SuiteError {
    /// The definition has no Benchmark of that name.
    #[error("no benchmark is named {0}")]
    UnknownBenchmark(String),
    /// The configuration names an Agent the agent definition does not have.
    #[error(transparent)]
    Configuration(#[from] super::EvaluationDefinitionError),
    /// A trial's Run was refused, or the Benchmark's repository cannot be
    /// reached, before any trial started.
    #[error(transparent)]
    Run(#[from] FactoryError),
    /// A suite of the Benchmark the caller started still runs.
    #[error("a suite of benchmark {0} you started still runs")]
    Running(String),
    /// The suite's trials would take the caller past the running trials
    /// they may have.
    #[error(
        "you have {open} trials running; a suite of {trials} more would pass the {MAX_OPEN_TRIALS} you may have"
    )]
    TooManyTrials {
        /// The caller's running trials.
        open: u32,
        /// The suite's trials.
        trials: u16,
    },
    /// Anything else.
    #[error("starting a suite failed: {0:?}")]
    Internal(rootcause::Report),
}

/// The evaluation use cases.
pub trait EvaluationService: Send + Sync + 'static {
    /// The evaluation insights of `actor`'s Runs and suites over `query`'s
    /// period.
    fn evaluation(
        &self,
        actor: &MacroUserIdStr<'static>,
        query: InsightsQuery,
    ) -> impl Future<Output = Result<EvaluationInsights, InsightsError>> + Send;

    /// Start a suite of `benchmark` as `actor`, on the configuration
    /// `agents`: one trial Run for each of its frozen tasks. For the agent
    /// `acting`, under a Definition with Standing Roles, its bot must fill
    /// one that allows `start`, as for any Run it starts.
    fn start_suite(
        &self,
        actor: MacroUserIdStr<'static>,
        benchmark: &str,
        agents: AgentChoices,
        acting: Option<ActingAgent>,
    ) -> impl Future<Output = Result<SuiteStarted, SuiteError>> + Send;

    /// Undo, as `actor`, the dismissal of the failures their team's rejected
    /// self-improvement Run `id` dismissed, so they are pending again and
    /// count toward the next one. Only the admin who named the team's
    /// Definition or the owner of its Run may; undoing again answers the
    /// first undo and changes nothing.
    fn undo_dismissal(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
    ) -> impl Future<Output = Result<DismissalUndoneResponse, UndoDismissalError>> + Send;
}

/// Evaluation over the records, the Run API (which starts trials as any
/// Run), and the definition.
pub struct EvaluationServiceImpl<R, F> {
    repo: R,
    runs: Arc<F>,
    definition: Arc<EvaluationDefinition>,
    agents: Option<crate::domain::agents::AgentDefinition>,
}

impl<R, F> EvaluationServiceImpl<R, F> {
    /// Create the service. `agents`, the agent definition, says which
    /// Agents a suite's configuration may name.
    pub fn new(
        repo: R,
        runs: Arc<F>,
        definition: Arc<EvaluationDefinition>,
        agents: Option<crate::domain::agents::AgentDefinition>,
    ) -> Self {
        Self {
            repo,
            runs,
            definition,
            agents,
        }
    }
}

/// The insights of `records`, with each defined Scorer, Experiment and
/// Benchmark listed whether or not it has records in the period.
pub(crate) fn evaluation_insights(
    definition: &EvaluationDefinition,
    query: &InsightsQuery,
    records: EvaluationRecords,
) -> EvaluationInsights {
    let mut tallies: BTreeMap<String, _> = records
        .scorers
        .into_iter()
        .map(|tally| (tally.scorer.clone(), tally))
        .collect();
    let mut scorers: Vec<ScorerInsight> = definition
        .scorers()
        .iter()
        .map(|scorer| {
            let tally = tallies.remove(&scorer.name);
            ScorerInsight {
                name: scorer.name.clone(),
                instructions: Some(scorer.instructions.clone()),
                flows: scorer.flows.clone(),
                sample_percent: Some(scorer.sample_percent),
                judged: tally.as_ref().map_or(0, |tally| tally.judged),
                passed: tally.as_ref().map_or(0, |tally| tally.passed),
                mean_score: tally.map(|tally| tally.mean_score),
            }
        })
        .collect();
    scorers.extend(tallies.into_values().map(|tally| ScorerInsight {
        name: tally.scorer,
        instructions: None,
        flows: Vec::new(),
        sample_percent: None,
        judged: tally.judged,
        passed: tally.passed,
        mean_score: Some(tally.mean_score),
    }));

    let mut arm_scores: BTreeMap<(String, String), Vec<ArmScoreInsight>> = BTreeMap::new();
    for score in records.arm_scores {
        arm_scores
            .entry((score.experiment, score.arm))
            .or_default()
            .push(ArmScoreInsight {
                scorer: score.scorer,
                judged: score.judged,
                mean_score: score.mean_score,
            });
    }
    let mut arms: BTreeMap<String, Vec<ArmInsight>> = BTreeMap::new();
    for tally in records.arms {
        let scores = arm_scores
            .remove(&(tally.experiment.clone(), tally.arm.clone()))
            .unwrap_or_default();
        arms.entry(tally.experiment).or_default().push(ArmInsight {
            name: tally.arm,
            agents: named_agents(&tally.agents),
            runs: tally.runs,
            ended: tally.ended,
            succeeded: tally.succeeded,
            fell_back: tally.fell_back,
            scores,
        });
    }
    let mut experiments: Vec<ExperimentInsight> = definition
        .experiments()
        .iter()
        .map(|experiment| {
            let mut recorded = arms.remove(&experiment.name).unwrap_or_default();
            let defined: BTreeSet<&str> = experiment
                .arms
                .iter()
                .map(|arm| arm.name.as_str())
                .collect();
            let mut listed: Vec<ArmInsight> = experiment
                .arms
                .iter()
                .map(
                    |arm| match recorded.iter().position(|taken| taken.name == arm.name) {
                        Some(place) => recorded.remove(place),
                        None => ArmInsight {
                            name: arm.name.clone(),
                            agents: named_agents(&arm.agents),
                            runs: 0,
                            ended: 0,
                            succeeded: 0,
                            fell_back: 0,
                            scores: Vec::new(),
                        },
                    },
                )
                .collect();
            listed.extend(
                recorded
                    .into_iter()
                    .filter(|arm| !defined.contains(arm.name.as_str())),
            );
            ExperimentInsight {
                name: experiment.name.clone(),
                flow: Some(experiment.flow.clone()),
                running: true,
                arms: listed,
            }
        })
        .collect();
    experiments.extend(arms.into_iter().map(|(name, arms)| ExperimentInsight {
        name,
        flow: None,
        running: false,
        arms,
    }));

    let benchmarks = definition
        .benchmarks()
        .iter()
        .map(|benchmark| BenchmarkSummary {
            name: benchmark.name.clone(),
            tasks: benchmark
                .tasks
                .iter()
                .map(|task| task.name.clone())
                .collect(),
            scorers: benchmark.scorers.clone(),
        })
        .collect();

    let suites = records
        .suites
        .into_iter()
        .map(|suite| {
            let mut tokens = TokenCounts::default();
            let mut scored = 0u32;
            let mut infrastructure = 0u32;
            let mut score_sum = 0.0;
            let trials = suite
                .trials
                .into_iter()
                .map(|trial| {
                    // A trial lost to infrastructure is left out even with
                    // a result, as one scored before such trials kept none
                    // has.
                    let lost = trial.run_outcome == Some(RunOutcome::Infrastructure);
                    if lost {
                        infrastructure += 1;
                    }
                    let result = trial.result.filter(|_| !lost);
                    let trial_tokens = result.map(|result| {
                        scored += 1;
                        score_sum += result.score;
                        let counts = TokenCounts {
                            input: result.tokens.input,
                            output: result.tokens.output,
                            cache_read: result.tokens.cache_read,
                            cache_write: result.tokens.cache_write,
                        };
                        tokens.input += counts.input;
                        tokens.output += counts.output;
                        tokens.cache_read += counts.cache_read;
                        tokens.cache_write += counts.cache_write;
                        counts
                    });
                    TrialInsight {
                        task: trial.task,
                        run_id: trial.run_id,
                        run_outcome: trial.run_outcome,
                        score: result.map(|result| result.score),
                        tokens: trial_tokens,
                        scores: trial
                            .scores
                            .into_iter()
                            .map(|(scorer, score)| TrialScore { scorer, score })
                            .collect(),
                    }
                })
                .collect();
            SuiteInsight {
                id: suite.id,
                benchmark: suite.benchmark,
                agents: named_agents(&suite.agents),
                created_at: suite.created_at,
                trials,
                scored,
                infrastructure,
                mean_score: (scored > 0).then(|| score_sum / f64::from(scored)),
                tokens,
            }
        })
        .collect();

    EvaluationInsights {
        from: query.from(),
        to: query.to(),
        time_zone: query.time_zone().to_string(),
        flow: query.flow().map(str::to_string),
        scorers,
        experiments,
        benchmarks,
        suites,
        self_improvement: None,
    }
}

impl<R, F> EvaluationServiceImpl<R, F>
where
    R: RunRepository + SelfImprovementRepository,
{
    /// `actor`'s team's self-improvement under `thresholds`; none when the
    /// team names no Definition.
    async fn self_improvement(
        &self,
        actor: &MacroUserIdStr<'static>,
        thresholds: SelfImprovementDefinition,
    ) -> Result<Option<SelfImprovementInsight>, InsightsError> {
        let Some(team) = self
            .repo
            .team_of(actor)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?
        else {
            return Ok(None);
        };
        let Some(naming) = self
            .repo
            .definition_naming(team.team_id)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?
        else {
            return Ok(None);
        };
        let (namer, _) = namer_standing(&self.repo, team.team_id, &naming)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?;
        let pending = self
            .repo
            .pending_failures(team.team_id, &naming.repo_url)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?;
        let last = self
            .repo
            .latest_improvement(team.team_id)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?;
        Ok(Some(SelfImprovementInsight {
            enabled: thresholds.enabled,
            failures_threshold: thresholds.failures,
            oldest_days: thresholds.oldest_days,
            namer,
            pending: pending.count,
            oldest_pending_at: pending.oldest,
            pending_by_source: pending
                .by_source
                .into_iter()
                .map(|(source, count)| PendingSource { source, count })
                .collect(),
            last: match last {
                Some(last) => {
                    let undoable = last.dismissed > 0 && last.dismissal_undone.is_none();
                    let can_undo_dismissal = undoable
                        && !self
                            .repo
                            .is_agent_account(actor)
                            .await
                            .map_err(|error| InsightsError::Internal(internal(error)))?
                        && may_undo(&self.repo, actor, &team, Some(&naming), &last)
                            .await
                            .map_err(InsightsError::Internal)?;
                    Some(ImprovementInsight {
                        id: last.id,
                        run_id: last.run_id,
                        state: last.state,
                        refusal: last.refusal,
                        failures: last.failures,
                        created_at: last.created_at,
                        settled_at: last.settled_at,
                        dismissed: last.dismissed,
                        dismissal_undone_at: last.dismissal_undone.map(|undone| undone.at),
                        can_undo_dismissal,
                    })
                }
                None => None,
            },
            repo_url: naming.repo_url,
        }))
    }
}

impl<R, F> EvaluationService for EvaluationServiceImpl<R, F>
where
    R: EvaluationRepository + RunRepository + SelfImprovementRepository,
    F: FactoryService,
{
    #[tracing::instrument(skip(self, actor))]
    async fn evaluation(
        &self,
        actor: &MacroUserIdStr<'static>,
        query: InsightsQuery,
    ) -> Result<EvaluationInsights, InsightsError> {
        let scope = query.scope(actor)?;
        // A team's Definition lists its own Scorers, Experiments and
        // Benchmarks; one that does not read lists none, never the
        // deployment's, and its records still show.
        let team = match self.runs.team_evaluation(actor).await {
            Ok(team) => team.map(|team| team.evaluation),
            Err(FactoryError::Internal(report)) => return Err(InsightsError::Internal(report)),
            Err(error) => {
                tracing::warn!(%error, "the team's definition did not read for its evaluation");
                Some(EvaluationDefinition::default())
            }
        };
        let definition = team.as_ref().unwrap_or(&self.definition);
        let records = self
            .repo
            .read_evaluation(&scope)
            .await
            .map_err(|error| InsightsError::Internal(internal(error)))?;
        let mut insights = evaluation_insights(definition, &query, records);
        // Only a Definition improves itself, under its own thresholds; one
        // that does not read shows the defaults.
        let thresholds = team
            .as_ref()
            .map(EvaluationDefinition::self_improvement)
            .unwrap_or_default();
        insights.self_improvement = self.self_improvement(actor, thresholds).await?;
        Ok(insights)
    }

    #[tracing::instrument(skip(self, actor, agents, acting))]
    async fn start_suite(
        &self,
        actor: MacroUserIdStr<'static>,
        benchmark: &str,
        agents: AgentChoices,
        acting: Option<ActingAgent>,
    ) -> Result<SuiteStarted, SuiteError> {
        // A team's Definition holds its own Benchmarks and the Agents a
        // configuration chooses among; a team with none takes the
        // deployment's.
        let team = self.runs.team_evaluation(&actor).await?;
        let (evaluation, defined_agents) = match &team {
            Some(team) => (&team.evaluation, Some(&team.agents)),
            None => (&*self.definition, self.agents.as_ref()),
        };
        let Some(definition) = evaluation.benchmark(benchmark) else {
            return Err(SuiteError::UnknownBenchmark(benchmark.to_string()));
        };
        check_choices(
            &format!("the configuration of benchmark {benchmark}"),
            &agents,
            defined_agents,
        )?;
        // Each trial is a Run the caller starts: an agent starting the
        // suite for them must hold a Standing Role that allows `start`,
        // checked once, before anything is kept, and kept as each trial's
        // start.
        let started_by = self
            .runs
            .standing_start(&actor, defined_agents, acting)
            .await?;

        // The caller must reach the repository before anything is kept;
        // its default branch's head, read once, is where every task that
        // pins no commit starts, so the suite's trials replay one code.
        let head = self
            .runs
            .benchmark_base(&actor, &definition.repository)
            .await?;
        let trials = u16::try_from(definition.tasks.len()).unwrap_or(u16::MAX);

        // The suite is recorded first, so each trial's Run, recorded and
        // started by the Run API, is kept with it before its workflow
        // starts. A task whose Run is refused stops the suite there: the
        // trials already started go on, and the caller is told which task
        // was refused. A suite none of whose trials started is deleted.
        let id = Uuid::now_v7();
        let inserted = self
            .repo
            .insert_suite(&NewSuite {
                id,
                user_id: &actor,
                benchmark,
                agents: &agents,
                trials,
            })
            .await
            .map_err(|error| SuiteError::Internal(internal(error)))?;
        match inserted {
            SuiteInsert::Inserted => {}
            SuiteInsert::Running => return Err(SuiteError::Running(benchmark.to_string())),
            SuiteInsert::TooManyTrials(open) => {
                return Err(SuiteError::TooManyTrials { open, trials });
            }
        }
        let mut runs = BTreeMap::new();
        let mut refused = None;
        for task in &definition.tasks {
            let trial = TrialKey {
                suite_id: id,
                task: task.name.clone(),
            };
            let base_commit = task.base_commit.clone().unwrap_or_else(|| head.clone());
            let started = self
                .runs
                .create_trial_run(
                    actor.clone(),
                    BENCHMARK_FLOW.to_string(),
                    task.request.clone(),
                    definition.repository.clone(),
                    trial,
                    base_commit,
                    started_by.clone(),
                )
                .await;
            match started {
                Ok(run) => {
                    runs.insert(task.name.clone(), run.run.id);
                }
                Err(error) if runs.is_empty() => {
                    if let Err(cleanup) = self.repo.delete_suite(id).await {
                        tracing::error!(error = ?cleanup, "a suite with no trial was not deleted");
                    }
                    return Err(error.into());
                }
                Err(error) => {
                    // The tasks after it never start, so they hold no place
                    // of the caller's running trials.
                    let started = u16::try_from(runs.len()).unwrap_or(u16::MAX);
                    if let Err(error) = self.repo.set_planned_trials(id, started).await {
                        tracing::error!(?error, "a stopped suite kept its planned trials");
                    }
                    refused = Some(RefusedTrial {
                        task: task.name.clone(),
                        reason: error.to_string(),
                    });
                    break;
                }
            }
        }
        Ok(SuiteStarted { id, runs, refused })
    }

    #[tracing::instrument(
        skip(self, actor),
        fields(improvement_id = %id, restored = tracing::field::Empty)
    )]
    async fn undo_dismissal(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
    ) -> Result<DismissalUndoneResponse, UndoDismissalError> {
        // An agent's account signs in as a person does, yet never is one.
        if self
            .repo
            .is_agent_account(actor)
            .await
            .map_err(undo_internal)?
        {
            return Err(UndoDismissalError::NotAPerson);
        }
        let Some(team) = self.repo.team_of(actor).await.map_err(undo_internal)? else {
            return Err(UndoDismissalError::NotFound);
        };
        let Some(improvement) = self
            .repo
            .improvement(team.team_id, id)
            .await
            .map_err(undo_internal)?
        else {
            return Err(UndoDismissalError::NotFound);
        };
        let naming = self
            .repo
            .definition_naming(team.team_id)
            .await
            .map_err(undo_internal)?;
        if !may_undo(&self.repo, actor, &team, naming.as_ref(), &improvement)
            .await
            .map_err(UndoDismissalError::Internal)?
        {
            return Err(UndoDismissalError::NotAllowed);
        }
        // The pending failures are read afresh at each of the team's Run
        // ends; only the Definition's thresholds are kept for a while, so
        // nothing kept needs forgetting.
        let undone = self
            .repo
            .undo_dismissal(team.team_id, id, actor)
            .await
            .map_err(undo_internal)?;
        let (restored, undone) = match undone {
            UndoDismissal::Undone { restored, undone } => (restored, undone),
            UndoDismissal::AlreadyUndone(undone) => (0, undone),
            UndoDismissal::NothingDismissed => return Err(UndoDismissalError::NothingDismissed),
            UndoDismissal::NotFound => return Err(UndoDismissalError::NotFound),
        };
        tracing::Span::current().record("restored", restored);
        Ok(DismissalUndoneResponse {
            restored,
            undone_at: undone.at,
        })
    }
}
