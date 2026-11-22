//! How the factory judges and compares its Runs, as a workspace defines it
//! (the [`EvaluationDefinition`]):
//!
//! - a Scorer judges a sample of finished Runs on one dimension, with a
//!   small model;
//! - an Experiment splits a Flow's Runs into arms by their task, each arm
//!   giving some Roles other Agents, and insights compare the arms;
//! - a Benchmark replays frozen tasks as trial Runs on one configuration,
//!   and records each trial's score and tokens.
//!
//! Sampling and arm assignment are decided from a hash, never a random
//! number: a retried judge samples the same Runs, and a task keeps its arm
//! each time it runs.

pub mod insights;
pub mod service;
#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};

use factory_records::domain::evaluation::{AgentChoices, BENCHMARK_FLOW, MAX_OPEN_TRIALS, RunArm};
use factory_records::domain::models::{MAX_REQUEST_BYTES, Role, RunId, checked_name};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::agents::AgentDefinition;
use crate::domain::built_in_flows::BuiltInFlow;

/// The most bytes of a Scorer's instructions.
pub const MAX_INSTRUCTIONS_BYTES: usize = 8192;

/// A Scorer: what it judges, which Flows' Runs, and what share of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScorerDefinition {
    /// Its name, as its judgments are kept under.
    pub name: String,
    /// What the model is told to judge, on one dimension, from 0 to 1.
    pub instructions: String,
    /// The Flows whose Runs it judges; every Flow when empty. A Benchmark's
    /// trials are judged only by the Scorers the Benchmark names.
    #[serde(default)]
    pub flows: Vec<String>,
    /// The percentage of those finished Runs it judges, from 0 to 100.
    pub sample_percent: u8,
    /// What its judge reads beside the request, the outcome and the Steps'
    /// outputs; nothing more when empty. A value not listed in
    /// [`ScorerRead`] does not read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<ScorerRead>,
}

/// Something more a Scorer's judge reads of a Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScorerRead {
    /// The change the Run merged, or its branch's head proposes when it
    /// merged nothing, as a unified diff cut to a budget.
    Diff,
}

impl ScorerDefinition {
    /// Whether its judge reads the Run's change.
    pub fn reads_diff(&self) -> bool {
        self.reads.contains(&ScorerRead::Diff)
    }

    /// Whether it judges `run_id`, a finished Run of `flow`: a Run of one of
    /// its Flows, in its sample.
    pub fn samples(&self, flow: Option<&str>, run_id: RunId) -> bool {
        let of_its_flows = self.flows.is_empty()
            || flow.is_some_and(|flow| self.flows.iter().any(|listed| listed == flow));
        of_its_flows && bucket(&self.name, run_id.as_uuid(), 100) < u64::from(self.sample_percent)
    }
}

/// One arm of an Experiment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArmDefinition {
    /// Its name.
    pub name: String,
    /// Its share of the split, against the other arms' weights.
    #[serde(default = "one")]
    pub weight: u16,
    /// The Agents it gives Roles in place of the agent definition's first;
    /// none for the arm that keeps the definition's.
    #[serde(default)]
    pub agents: AgentChoices,
}

fn one() -> u16 {
    1
}

/// An Experiment: the Flow whose Runs it splits, and its arms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentDefinition {
    /// Its name.
    pub name: String,
    /// The Flow whose Runs on a task it takes.
    pub flow: String,
    /// Its arms, at least two.
    pub arms: Vec<ArmDefinition>,
}

impl ExperimentDefinition {
    /// The arm a Run on the task `task_id` takes: picked by the weights
    /// from a hash of the Experiment and the task, so the task keeps it.
    pub fn arm_for(&self, task_id: Uuid) -> &ArmDefinition {
        let total: u64 = self.arms.iter().map(|arm| u64::from(arm.weight)).sum();
        let mut left = bucket(&self.name, task_id, total);
        for arm in &self.arms {
            let weight = u64::from(arm.weight);
            if left < weight {
                return arm;
            }
            left -= weight;
        }
        // The bucket is below the weights' sum, so an arm took it above;
        // the checks keep at least two arms.
        &self.arms[0]
    }
}

/// A frozen task: a request kept as it was, to replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenTask {
    /// Its name within the Benchmark.
    pub name: String,
    /// What a trial of it is asked to do.
    pub request: String,
    /// The commit its trials' write starts from, a full commit id; when
    /// unset, the repository's default branch's head as a suite starts.
    #[serde(default)]
    pub base_commit: Option<String>,
}

/// A Benchmark: frozen tasks on one repository, and the Scorers that judge
/// its trials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BenchmarkDefinition {
    /// Its name.
    pub name: String,
    /// The GitHub repository its trials work on, as a Run's `repoUrl`.
    pub repository: String,
    /// Its tasks, at least one.
    pub tasks: Vec<FrozenTask>,
    /// The Scorers that judge each trial, beside its mechanical score; none
    /// by default, so a Benchmark costs no judging unless it asks.
    #[serde(default)]
    pub scorers: Vec<String>,
}

/// The Scorers, Experiments and Benchmarks of a factory: a team's Factory
/// Definition's (`scorers.json`) or, for a team that names none, the
/// deployment's (`FACTORY_EVALUATION`), both this JSON. Reading it checks
/// what holds in itself; [`Self::checked`] checks it against the Flows and
/// Agents it is used with. A Run keeps its Definition's as it wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EvaluationDefinitionFields", rename_all = "camelCase")]
pub struct EvaluationDefinition {
    scorers: Vec<ScorerDefinition>,
    experiments: Vec<ExperimentDefinition>,
    benchmarks: Vec<BenchmarkDefinition>,
    #[serde(skip_serializing_if = "SelfImprovementDefinition::is_default")]
    self_improvement: SelfImprovementDefinition,
}

/// The fewest failures, by default, that start a self-improvement Run.
pub const DEFAULT_IMPROVEMENT_FAILURES: u16 = 25;

/// How old, in days by default, the oldest pending failure is before a
/// self-improvement Run starts with fewer.
pub const DEFAULT_IMPROVEMENT_OLDEST_DAYS: u16 = 7;

/// The most failures a self-improvement Run may wait for.
pub const MAX_IMPROVEMENT_FAILURES: u16 = 1000;

/// The most days a self-improvement Run may wait for its oldest failure.
pub const MAX_IMPROVEMENT_OLDEST_DAYS: u16 = 365;

/// When a team's Factory Definition starts a self-improvement Run against
/// itself: once its pending failures (Scorers' judgments below the pass
/// score, and Runs marked flawed) reach `failures`, or the oldest is
/// `oldest_days` old. Only a Definition's `scorers.json` is read for it: a
/// team that names none starts none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelfImprovementDefinition {
    /// Whether the Definition improves itself at all; on by default.
    #[serde(default = "on")]
    pub enabled: bool,
    /// How many pending failures start a Run, from 1 to
    /// [`MAX_IMPROVEMENT_FAILURES`].
    #[serde(default = "default_failures")]
    pub failures: u16,
    /// How many days old the oldest pending failure is before a Run starts
    /// with fewer, from 1 to [`MAX_IMPROVEMENT_OLDEST_DAYS`].
    #[serde(default = "default_oldest_days")]
    pub oldest_days: u16,
}

fn on() -> bool {
    true
}

fn default_failures() -> u16 {
    DEFAULT_IMPROVEMENT_FAILURES
}

fn default_oldest_days() -> u16 {
    DEFAULT_IMPROVEMENT_OLDEST_DAYS
}

impl Default for SelfImprovementDefinition {
    fn default() -> Self {
        Self {
            enabled: true,
            failures: DEFAULT_IMPROVEMENT_FAILURES,
            oldest_days: DEFAULT_IMPROVEMENT_OLDEST_DAYS,
        }
    }
}

impl SelfImprovementDefinition {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A team's Factory Definition's Scorers, Experiments and Benchmarks, with
/// the Agents its arms and a suite's configuration choose among.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamEvaluation {
    /// Its Scorers, Experiments and Benchmarks.
    pub evaluation: EvaluationDefinition,
    /// Its Agents.
    pub agents: AgentDefinition,
}

/// An [`EvaluationDefinition`] as written, before its checks.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvaluationDefinitionFields {
    #[serde(default)]
    scorers: Vec<ScorerDefinition>,
    #[serde(default)]
    experiments: Vec<ExperimentDefinition>,
    #[serde(default)]
    benchmarks: Vec<BenchmarkDefinition>,
    #[serde(default)]
    self_improvement: SelfImprovementDefinition,
}

/// What is wrong with an evaluation definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationDefinitionError {
    /// A name breaks the name rules.
    #[error(transparent)]
    InvalidName(#[from] factory_records::domain::models::InvalidName),
    /// Two of one kind share a name.
    #[error("two {kind}s are named {name}")]
    Duplicate {
        /// Scorer, Experiment, arm, Benchmark or task.
        kind: &'static str,
        /// The name.
        name: String,
    },
    /// A Scorer's instructions are empty or too long.
    #[error("scorer {0} must have instructions of at most {MAX_INSTRUCTIONS_BYTES} bytes")]
    Instructions(String),
    /// A Scorer samples more than all of its Runs.
    #[error("scorer {0} must sample from 0 to 100 percent of its runs")]
    Sample(String),
    /// A Flow that is not a built-in Flow, or the Benchmark's own.
    #[error("{0} is not a flow runs are started from")]
    UnknownFlow(String),
    /// An Experiment has fewer than two arms, or an arm of no weight.
    #[error("experiment {0} must have at least two arms, each of weight 1 or more")]
    Arms(String),
    /// Two Experiments of one Flow both give the same Role an Agent, so a
    /// Run in both would not know whose to take.
    #[error("experiments {first} and {second} both choose the agent of role {role}")]
    SharedRole {
        /// The first Experiment.
        first: String,
        /// The second.
        second: String,
        /// The Role.
        role: Role,
    },
    /// A Benchmark has no task, or a task's request is empty or too long,
    /// or the base commit it pins is not a full commit id.
    #[error(
        "benchmark {0} must have tasks, each with a request of at most {MAX_REQUEST_BYTES} bytes \
         and any base commit as 40 lowercase hex digits"
    )]
    Tasks(String),
    /// A Benchmark has more tasks than a person may have trials running.
    #[error("benchmark {0} must have at most {MAX_OPEN_TRIALS} tasks")]
    TooManyTasks(String),
    /// The self-improvement thresholds are out of range.
    #[error(
        "selfImprovement must wait for 1 to {MAX_IMPROVEMENT_FAILURES} failures and 1 to \
         {MAX_IMPROVEMENT_OLDEST_DAYS} days"
    )]
    SelfImprovement,
    /// A Benchmark names a Scorer the definition does not have.
    #[error("benchmark {benchmark} names scorer {scorer}, which the definition does not have")]
    UnknownScorer {
        /// The Benchmark.
        benchmark: String,
        /// The Scorer.
        scorer: String,
    },
    /// An arm or a configuration gives a Role an Agent the agent definition
    /// does not have, or there is no agent definition.
    #[error("{whose} gives role {role} agent {agent}, which the agent definition does not have")]
    UnknownAgent {
        /// The arm or configuration.
        whose: String,
        /// The Role.
        role: Role,
        /// The Agent.
        agent: String,
    },
    /// An arm or a configuration gives a Role an Agent whose bot is Codex's
    /// cloud agent ([`bot_id::CODEX_BOT_ID`]): its sessions cannot hand a
    /// Step's result back, so the Role's Step would never end. Whether a
    /// Role lists the Agent does not matter: the choice gives it the Step.
    #[error(
        "{whose} gives role {role} agent {agent}, which runs on the Codex cloud agent, whose \
         sessions cannot hand back a step's result: run Codex with macrod on a machine \
         (`macrod service --agent codex`) and use the bot id of that machine's agent"
    )]
    CloudCodexAgent {
        /// The arm or configuration.
        whose: String,
        /// The Role.
        role: Role,
        /// The Agent.
        agent: String,
    },
}

/// Fails when two of `names` are the same.
fn unique<'a>(
    kind: &'static str,
    names: impl IntoIterator<Item = &'a String>,
) -> Result<(), EvaluationDefinitionError> {
    let mut seen = BTreeSet::new();
    for name in names {
        checked_name(kind, name.clone())?;
        if !seen.insert(name) {
            return Err(EvaluationDefinitionError::Duplicate {
                kind,
                name: name.clone(),
            });
        }
    }
    Ok(())
}

/// Whether `commit` is a full commit id: 40 lowercase hex digits.
pub fn is_commit_id(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl TryFrom<EvaluationDefinitionFields> for EvaluationDefinition {
    type Error = EvaluationDefinitionError;

    fn try_from(fields: EvaluationDefinitionFields) -> Result<Self, Self::Error> {
        unique("scorer", fields.scorers.iter().map(|scorer| &scorer.name))?;
        for scorer in &fields.scorers {
            if scorer.instructions.trim().is_empty()
                || scorer.instructions.len() > MAX_INSTRUCTIONS_BYTES
            {
                return Err(EvaluationDefinitionError::Instructions(scorer.name.clone()));
            }
            if scorer.sample_percent > 100 {
                return Err(EvaluationDefinitionError::Sample(scorer.name.clone()));
            }
        }

        unique(
            "experiment",
            fields.experiments.iter().map(|experiment| &experiment.name),
        )?;
        // Each Role a Flow's Experiments choose for, and the first
        // Experiment that does.
        let mut chosen: BTreeMap<(&str, &Role), &str> = BTreeMap::new();
        for experiment in &fields.experiments {
            unique("arm", experiment.arms.iter().map(|arm| &arm.name))?;
            if experiment.arms.len() < 2 || experiment.arms.iter().any(|arm| arm.weight == 0) {
                return Err(EvaluationDefinitionError::Arms(experiment.name.clone()));
            }
            let roles: BTreeSet<&Role> = experiment
                .arms
                .iter()
                .flat_map(|arm| arm.agents.keys())
                .collect();
            for role in roles {
                let first = chosen
                    .entry((experiment.flow.as_str(), role))
                    .or_insert(&experiment.name);
                if *first != experiment.name {
                    return Err(EvaluationDefinitionError::SharedRole {
                        first: (*first).to_string(),
                        second: experiment.name.clone(),
                        role: role.clone(),
                    });
                }
            }
        }

        unique(
            "benchmark",
            fields.benchmarks.iter().map(|benchmark| &benchmark.name),
        )?;
        for benchmark in &fields.benchmarks {
            unique("task", benchmark.tasks.iter().map(|task| &task.name))?;
            let unreadable = benchmark.tasks.iter().any(|task| {
                task.request.trim().is_empty()
                    || task.request.len() > MAX_REQUEST_BYTES
                    || task
                        .base_commit
                        .as_deref()
                        .is_some_and(|commit| !is_commit_id(commit))
            });
            if benchmark.tasks.is_empty() || unreadable {
                return Err(EvaluationDefinitionError::Tasks(benchmark.name.clone()));
            }
            // A suite starts every task at once, so one with more tasks than
            // a person may have trials running could never start.
            if benchmark.tasks.len() > usize::from(MAX_OPEN_TRIALS) {
                return Err(EvaluationDefinitionError::TooManyTasks(
                    benchmark.name.clone(),
                ));
            }
            for scorer in &benchmark.scorers {
                if !fields.scorers.iter().any(|defined| defined.name == *scorer) {
                    return Err(EvaluationDefinitionError::UnknownScorer {
                        benchmark: benchmark.name.clone(),
                        scorer: scorer.clone(),
                    });
                }
            }
        }

        let improvement = fields.self_improvement;
        if !(1..=MAX_IMPROVEMENT_FAILURES).contains(&improvement.failures)
            || !(1..=MAX_IMPROVEMENT_OLDEST_DAYS).contains(&improvement.oldest_days)
        {
            return Err(EvaluationDefinitionError::SelfImprovement);
        }

        Ok(Self {
            scorers: fields.scorers,
            experiments: fields.experiments,
            benchmarks: fields.benchmarks,
            self_improvement: improvement,
        })
    }
}

/// Fails unless `agents` names only Agents of the agent definition, none
/// of them on Codex's cloud agent: a choice gives its Agent the Role's
/// Step, which that agent could never hand back.
pub fn check_choices(
    whose: &str,
    choices: &AgentChoices,
    agents: Option<&AgentDefinition>,
) -> Result<(), EvaluationDefinitionError> {
    for (role, agent) in choices {
        let Some(defined) = agents.and_then(|agents| agents.agent(agent)) else {
            return Err(EvaluationDefinitionError::UnknownAgent {
                whose: whose.to_string(),
                role: role.clone(),
                agent: agent.as_str().to_string(),
            });
        };
        if defined.bot == bot_id::CODEX_BOT_ID {
            return Err(EvaluationDefinitionError::CloudCodexAgent {
                whose: whose.to_string(),
                role: role.clone(),
                agent: agent.as_str().to_string(),
            });
        }
    }
    Ok(())
}

impl EvaluationDefinition {
    /// The deployment's definition, for a team that names no Factory
    /// Definition: `definition`, from `FACTORY_EVALUATION`, checked against
    /// the built-in Flows and `agents`, the deployment's agent definition;
    /// an empty one when unset.
    pub fn configured(
        definition: Option<Self>,
        agents: Option<&AgentDefinition>,
    ) -> Result<Self, EvaluationDefinitionError> {
        definition
            .unwrap_or_default()
            .checked(|flow| BuiltInFlow::named(flow).is_some(), agents)
    }

    /// Itself, once every Flow its Scorers and Experiments name is one
    /// `starts` says Runs are started of (never a Benchmark's), and its
    /// arms name only Agents of `agents`.
    pub fn checked(
        self,
        starts: impl Fn(&str) -> bool,
        agents: Option<&AgentDefinition>,
    ) -> Result<Self, EvaluationDefinitionError> {
        let startable = |flow: &str| flow != BENCHMARK_FLOW && starts(flow);
        for scorer in &self.scorers {
            if let Some(flow) = scorer.flows.iter().find(|flow| !startable(flow)) {
                return Err(EvaluationDefinitionError::UnknownFlow(flow.clone()));
            }
        }
        for experiment in &self.experiments {
            if !startable(&experiment.flow) {
                return Err(EvaluationDefinitionError::UnknownFlow(
                    experiment.flow.clone(),
                ));
            }
            for arm in &experiment.arms {
                let whose = format!("arm {} of experiment {}", arm.name, experiment.name);
                check_choices(&whose, &arm.agents, agents)?;
            }
        }
        Ok(self)
    }

    /// Its Scorers.
    pub fn scorers(&self) -> &[ScorerDefinition] {
        &self.scorers
    }

    /// Its Experiments.
    pub fn experiments(&self) -> &[ExperimentDefinition] {
        &self.experiments
    }

    /// Its Benchmarks.
    pub fn benchmarks(&self) -> &[BenchmarkDefinition] {
        &self.benchmarks
    }

    /// When a Definition with it starts a self-improvement Run.
    pub fn self_improvement(&self) -> SelfImprovementDefinition {
        self.self_improvement
    }

    /// The Scorer named `name`.
    pub fn scorer(&self, name: &str) -> Option<&ScorerDefinition> {
        self.scorers.iter().find(|scorer| scorer.name == name)
    }

    /// The Benchmark named `name`.
    pub fn benchmark(&self, name: &str) -> Option<&BenchmarkDefinition> {
        self.benchmarks
            .iter()
            .find(|benchmark| benchmark.name == name)
    }

    /// The arms a new Run of `flow` on the task `task_id` takes: one for
    /// each Experiment of the Flow. A Run on no task takes none, so the
    /// arms split tasks, never one task's Runs.
    pub fn assign_arms(&self, flow: &str, task_id: Option<Uuid>) -> Vec<RunArm> {
        let Some(task_id) = task_id else {
            return Vec::new();
        };
        self.experiments
            .iter()
            .filter(|experiment| experiment.flow == flow)
            .map(|experiment| {
                let arm = experiment.arm_for(task_id);
                RunArm {
                    experiment: experiment.name.clone(),
                    arm: arm.name.clone(),
                    agents: arm.agents.clone(),
                }
            })
            .collect()
    }
}

/// Where `key` falls among `buckets` for `name`: the first eight bytes of
/// the SHA-256 of both, so every worker and every retry agrees.
fn bucket(name: &str, key: Uuid, buckets: u64) -> u64 {
    let mut hash = Sha256::new();
    hash.update(name.as_bytes());
    hash.update([0]);
    hash.update(key.as_bytes());
    let digest = hash.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first) % buckets.max(1)
}
