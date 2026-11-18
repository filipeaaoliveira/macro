//! Evaluation insights: the live Scorers' judgments of a person's Runs over
//! a period, how each Experiment's arms did, and their Benchmark suites.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use factory_records::domain::evaluation::{
    AgentChoices, BENCHMARK_FLOW, EvaluationRecords, EvaluationRepository, MAX_OPEN_TRIALS,
    NewSuite, SuiteInsert, TrialKey,
};
use factory_records::domain::models::{RunId, RunOutcome};
use macro_user_id::user_id::MacroUserIdStr;
use serde::Serialize;
use uuid::Uuid;

use super::{EvaluationDefinition, check_choices};
use crate::domain::insights::{InsightsError, InsightsQuery, TokenCounts};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

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
    /// How many of them ended.
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
    /// when they stayed red, 0 otherwise; none until its Run ended.
    pub score: Option<f64>,
    /// The tokens its agents used; none until its Run ended.
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
    /// How many trials have their result.
    pub scored: u32,
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
    /// `agents`: one trial Run for each of its frozen tasks.
    fn start_suite(
        &self,
        actor: MacroUserIdStr<'static>,
        benchmark: &str,
        agents: AgentChoices,
    ) -> impl Future<Output = Result<SuiteStarted, SuiteError>> + Send;
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
            let mut score_sum = 0.0;
            let trials = suite
                .trials
                .into_iter()
                .map(|trial| {
                    let trial_tokens = trial.result.map(|result| {
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
                        score: trial.result.map(|result| result.score),
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
    }
}

impl<R, F> EvaluationService for EvaluationServiceImpl<R, F>
where
    R: EvaluationRepository,
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
        Ok(evaluation_insights(definition, &query, records))
    }

    #[tracing::instrument(skip(self, actor, agents))]
    async fn start_suite(
        &self,
        actor: MacroUserIdStr<'static>,
        benchmark: &str,
        agents: AgentChoices,
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
}
