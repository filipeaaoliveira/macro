//! PostgreSQL implementation of the [`RunRepository`] port.

mod automations;
mod board;
mod check_rounds;
mod gate_replies;
mod insights;
mod main_hold;
mod review_rounds;
mod standing_rules;
mod steering;
#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::automations::{RunAutomation, TriggerKind};
use crate::domain::models::{
    AgentName, AskedBack, CheckResult, CheckRound, CheckRoundOutcome, CheckRoundStart,
    ClaimedSession, ControlFileChanges, EntryId, Gate, GateAnswer, GateKind, HandBack, HandedBack,
    InvalidName, NewRun, NewSteer, Outcome, PassedOver, QueueHold, QueuePlaces, Repository, Role,
    Run, RunEnd, RunId, RunInsert, RunOutcome, SessionStep, StandInAnswer, SteerRefusal, Steered,
    Step, StepEnd, StepKey, StepSession, StepSteering, StepTerminal, StoppingSession, TaskLink,
    TeamMembership,
};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules, StandingRule};

/// The foreign key from a Run to its task document.
const TASK_FOREIGN_KEY: &str = "factory_run_task_id_fkey";

/// The index that keeps an Automation from starting a second Run on a
/// task while one it started runs there.
const AUTOMATION_TASK_OPEN_INDEX: &str = "factory_run_automation_task_open_idx";

/// The index that keeps a firing from starting a second Run.
const AUTOMATION_FIRING_INDEX: &str = "factory_run_automation_firing_idx";

/// The foreign key from a Gate's answer to the user who gave it.
const GATE_USER_FOREIGN_KEY: &str = "factory_gate_user_id_fkey";

/// Postgres-backed Run repository.
#[derive(Debug, Clone)]
pub struct PgRunRepository {
    pool: PgPool,
}

impl PgRunRepository {
    /// Create a repository backed by the provided pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The answer the Gate of `key` keeps; none when the Gate is not
    /// recorded or has no answer.
    pub(crate) async fn kept_answer(
        &self,
        key: StepKey,
    ) -> Result<Option<GateAnswer>, PgRunRepositoryErr> {
        let stored = sqlx::query!(
            r#"
            SELECT answer, note, user_id, rule, approved_control_files, reply_confidence,
                by_stand_in, agent_session_id, answered_at
            FROM factory_gate
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        let (answer, answered_at) = match (stored.answer, stored.answered_at) {
            (Some(answer), Some(answered_at)) => (answer, answered_at),
            (None, None) => return Ok(None),
            _ => return Err(PgRunRepositoryErr::PartialGate),
        };
        Ok(Some(GateAnswer {
            answer: Outcome::try_from(answer)?,
            note: stored.note,
            user_id: stored
                .user_id
                .map(|id| {
                    MacroUserIdStr::try_from(id.clone())
                        .map_err(|_| PgRunRepositoryErr::InvalidUserId(id))
                })
                .transpose()?,
            rule: stored
                .rule
                .map(|rule| {
                    rule.parse::<StandingRule>()
                        .map_err(|_| PgRunRepositoryErr::UnknownStandingRules(vec![rule]))
                })
                .transpose()?,
            approved_control_files: stored.approved_control_files,
            reply_confidence: stored.reply_confidence.map(percent).transpose()?,
            by_stand_in: stored.by_stand_in,
            agent_session_id: stored.agent_session_id,
            answered_at,
        }))
    }

    /// Whether a factory Step works, or worked, in the agent session
    /// `session`. Other services ask it to tell a Step's session apart; the
    /// index on `session_id` serves it.
    #[tracing::instrument(err, skip(self))]
    pub async fn is_step_session(&self, session: Uuid) -> Result<bool, PgRunRepositoryErr> {
        let is_step_session = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM factory_step WHERE session_id = $1) AS "is_step_session!""#,
            session,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(is_step_session)
    }
}

/// Errors produced by the Postgres Run repository.
#[derive(Debug, thiserror::Error)]
pub enum PgRunRepositoryErr {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored name breaks the rules its column checks.
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
    /// A stored Run outcome is not one this code knows.
    #[error("unknown run outcome {0:?}")]
    UnknownRunOutcome(String),
    /// The board read a place for a Run that this code does not know.
    #[error("unknown board state {0:?}")]
    UnknownBoardState(String),
    /// A stored round of checks ended with an outcome this code does not
    /// know.
    #[error("unknown check round outcome {0:?}")]
    UnknownCheckRoundOutcome(String),
    /// A stored review round's outcome, or a finding's kind, is not one
    /// this code knows.
    #[error("unknown review round outcome or finding kind {0:?}")]
    UnknownReviewWord(String),
    /// A stored finding has a first line but no last, or the other way
    /// round.
    #[error("a stored finding has only one end of its lines")]
    PartialLines,
    /// A stored visit or place does not fit its number.
    #[error("visit or place {0} is out of range")]
    OutOfRange(i32),
    /// A stored Run or Step has only part of its end.
    #[error("a stored end has an outcome but no time, or the other way round")]
    PartialEnd,
    /// A verified round's results do not read as each check's name, exit
    /// code and output.
    #[error("a round's check results do not read")]
    UnreadableCheckResults,
    /// A stored Run has a repository but no base branch, or the other way
    /// round.
    #[error("a stored run has a repository but no base branch, or the other way round")]
    PartialRepository,
    /// A stored Run has only part of the Control Files its branch changed.
    #[error("a stored run has only part of the control files its branch changed")]
    PartialControlFiles,
    /// A stored Run has only part of its Standing Rules.
    #[error("a stored run has only part of its standing rules")]
    PartialStandingRules,
    /// A stored Gate kind, rule or rules' source is not one this code
    /// knows.
    #[error("unknown standing rules {0:?}")]
    UnknownStandingRules(Vec<String>),
    /// A stored Run has only part of the Automation that started it.
    #[error("a stored run has only part of the automation that started it")]
    PartialAutomation,
    /// A stored Automation's trigger kind is not one this code knows.
    #[error("unknown automation trigger kind {0:?}")]
    UnknownTriggerKind(String),
    /// A stored Automation's trigger does not read.
    #[error("a stored automation's trigger does not read: {0}")]
    UnreadableTrigger(serde_json::Error),
    /// A stored firing has neither a Run nor a reason it started none, or
    /// a reason this code does not know.
    #[error("a stored automation firing ended without a run or a known reason")]
    UnreadableFiring,
    /// A stored Gate has only part of its question or its answer.
    #[error("a stored gate has only part of its question or its answer")]
    PartialGate,
    /// The Step does not exist.
    #[error("the step does not exist")]
    NoSuchStep,
    /// A stored steer's action is not one this code knows.
    #[error(transparent)]
    UnknownSteerAction(#[from] crate::domain::models::UnknownSteerAction),
    /// A stored task id is not a UUID.
    #[error("stored task id {0:?} is not a UUID")]
    InvalidTaskId(String),
    /// A stored user id does not read as one.
    #[error("stored user id {0:?} does not read as one")]
    InvalidUserId(String),
    /// A stored Step has passed over a different number of Agents than it
    /// has reasons for.
    #[error("a stored step's passed over agents and their reasons do not pair up")]
    PartialPassedOver,
    /// A stored hold has only part of its start, head and failed checks.
    #[error("a stored main hold has only part of its start, head and failed checks")]
    PartialHold,
    /// A stored required checks state is not one this code knows.
    #[error("unknown required checks state {0:?}")]
    UnknownRequiredChecks(String),
    /// A pull request number does not fit its column or its type.
    #[error("a pull request number is out of range")]
    NumberOutOfRange,
}

struct RunRow {
    id: Uuid,
    request: String,
    user_id: String,
    flow: Option<String>,
    created_at: DateTime<Utc>,
    task_id: Option<String>,
    project_id: Option<Uuid>,
    repo_url: Option<String>,
    base_branch: Option<String>,
    created_in_person: bool,
    control_file_patterns: Option<Vec<String>>,
    control_files_team_id: Option<Uuid>,
    tampered_files: Option<Vec<String>>,
    named_control_files: Option<Vec<String>>,
    control_files_unlisted: bool,
    standing_rules: Option<Vec<String>>,
    standing_rules_source: Option<String>,
    standing_rules_project_id: Option<Uuid>,
    automation_id: Option<Uuid>,
    automation_name: Option<String>,
    automation_trigger: Option<String>,
    pull_request_url: Option<String>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunRow> for Run {
    type Error = PgRunRepositoryErr;

    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.ended_at) {
            (Some(outcome), Some(ended_at)) => Some(RunEnd {
                outcome: outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))?,
                ended_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        let repository = match (row.repo_url, row.base_branch) {
            (Some(url), Some(base_branch)) => Some(Repository { url, base_branch }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialRepository),
        };
        let standing_rules = match (row.standing_rules, row.standing_rules_source) {
            (Some(kinds), Some(source)) => Some(RunRules {
                definition: OrchestratorDefinition::from_kinds(&kinds)
                    .ok_or(PgRunRepositoryErr::UnknownStandingRules(kinds))?,
                source: source
                    .parse::<RulesSource>()
                    .map_err(|_| PgRunRepositoryErr::UnknownStandingRules(vec![source]))?,
                project_id: row.standing_rules_project_id,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialStandingRules),
        };
        let automation = match (
            row.automation_id,
            row.automation_name,
            row.automation_trigger,
        ) {
            (Some(id), Some(name), Some(trigger)) => Some(RunAutomation {
                id,
                name,
                trigger: trigger
                    .parse::<TriggerKind>()
                    .map_err(|_| PgRunRepositoryErr::UnknownTriggerKind(trigger))?,
            }),
            (None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialAutomation),
        };
        Ok(Run {
            id: RunId::from_uuid(row.id),
            request: row.request,
            user_id: MacroUserIdStr::try_from(row.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
            flow: row.flow,
            created_at: row.created_at,
            task_id: row
                .task_id
                .map(|id| Uuid::parse_str(&id).map_err(|_| PgRunRepositoryErr::InvalidTaskId(id)))
                .transpose()?,
            project_id: row.project_id,
            repository,
            created_in_person: row.created_in_person,
            control_file_patterns: row.control_file_patterns,
            control_files_team_id: row.control_files_team_id,
            control_files: match (row.tampered_files, row.named_control_files) {
                (Some(tampered), Some(named)) => Some(ControlFileChanges {
                    tampered,
                    named,
                    unlisted: row.control_files_unlisted,
                }),
                (None, None) => None,
                _ => return Err(PgRunRepositoryErr::PartialControlFiles),
            },
            pull_request_url: row.pull_request_url,
            standing_rules,
            automation,
            end,
        })
    }
}

struct StepRow {
    run_id: Uuid,
    seq: i32,
    entry: String,
    visit: i32,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    output: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    question: Option<String>,
    answers: Option<Vec<String>>,
    opened_at: Option<DateTime<Utc>>,
    answer: Option<String>,
    note: Option<String>,
    user_id: Option<String>,
    approved_control_files: Option<Vec<String>>,
    rule: Option<String>,
    reply_confidence: Option<i16>,
    by_stand_in: Option<bool>,
    agent_session_id: Option<Uuid>,
    asked_back_reply: Option<String>,
    asked_back_at: Option<DateTime<Utc>>,
    stand_in_answer: Option<String>,
    stand_in_reason: Option<String>,
    stand_in_confidence: Option<i16>,
    stand_in_until: Option<DateTime<Utc>>,
    answered_at: Option<DateTime<Utc>>,
    role: Option<String>,
    agent: Option<String>,
    passed_over: Vec<String>,
    pass_reasons: Vec<String>,
    terminal_session_id: Option<Uuid>,
    paused_at: Option<DateTime<Utc>>,
    stop_asked_at: Option<DateTime<Utc>>,
    between_turns: bool,
    pause_ran_out_at: Option<DateTime<Utc>>,
}

impl StepRow {
    /// The Step's Gate, from the columns its left join filled.
    fn gate(&mut self) -> Result<Option<Gate>, PgRunRepositoryErr> {
        let answer = match (self.answer.take(), self.answered_at) {
            (Some(answer), Some(answered_at)) => Some(GateAnswer {
                answer: Outcome::try_from(answer)?,
                note: self.note.take(),
                user_id: self
                    .user_id
                    .take()
                    .map(|id| {
                        MacroUserIdStr::try_from(id.clone())
                            .map_err(|_| PgRunRepositoryErr::InvalidUserId(id))
                    })
                    .transpose()?,
                approved_control_files: self.approved_control_files.take(),
                rule: self
                    .rule
                    .take()
                    .map(|rule| {
                        rule.parse::<StandingRule>()
                            .map_err(|_| PgRunRepositoryErr::UnknownStandingRules(vec![rule]))
                    })
                    .transpose()?,
                reply_confidence: self.reply_confidence.map(percent).transpose()?,
                by_stand_in: self.by_stand_in.unwrap_or(false),
                agent_session_id: self.agent_session_id,
                answered_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialGate),
        };
        let asked_back = match (self.asked_back_reply.take(), self.asked_back_at) {
            (Some(reply), Some(at)) => Some(AskedBack { reply, at }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialGate),
        };
        let stand_in = match (
            self.stand_in_answer.take(),
            self.stand_in_reason.take(),
            self.stand_in_confidence,
            self.stand_in_until,
        ) {
            (Some(answer), Some(reason), Some(confidence), Some(until)) => Some(StandInAnswer {
                answer: Outcome::try_from(answer)?,
                reason,
                confidence: percent(confidence)?,
                until,
            }),
            (None, None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialGate),
        };

        match (self.question.take(), self.answers.take(), self.opened_at) {
            (Some(question), Some(answers), Some(opened_at)) => Ok(Some(Gate {
                kind: GateKind::of(
                    self.outcome
                        .clone()
                        .map(Outcome::try_from)
                        .transpose()?
                        .as_ref(),
                ),
                question,
                answers: answers
                    .into_iter()
                    .map(Outcome::try_from)
                    .collect::<Result<_, _>>()?,
                opened_at,
                answer,
                asked_back,
                stand_in,
            })),
            (None, None, None) if answer.is_none() => Ok(None),
            _ => Err(PgRunRepositoryErr::PartialGate),
        }
    }
}

/// The places `user_id` holds in their Queue, counted up to `limit` in
/// all: their Runs that have not ended and wait on them in person at a Gate
/// or a question (not a Gate the stand-in answered, while the owner may
/// still override it), and those of `asking_flows` that have not reached a
/// Gate yet, so a Run just started holds the place it will ask for. A Run
/// of a Flow with no Gate of its own holds one only once it asks something.
/// As `list_waiting_runs`: the person's open Runs through
/// `factory_run_user_open_idx`, each checked by the Gates' primary key, up
/// to the limit, under a timeout that also bounds the rest of the
/// transaction.
async fn queue_places(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: &MacroUserIdStr<'static>,
    asking_flows: &[&str],
    limit: u16,
) -> Result<QueuePlaces, PgRunRepositoryErr> {
    sqlx::query!("SET LOCAL statement_timeout = '5s'")
        .execute(&mut **transaction)
        .await?;
    let asking_flows: Vec<String> = asking_flows
        .iter()
        .map(|flow| (*flow).to_string())
        .collect();
    let places = sqlx::query!(
        r#"
        SELECT
            count(*) FILTER (WHERE places.waiting) AS "waiting!",
            count(*) FILTER (WHERE NOT places.waiting) AS "starting!"
        FROM (
            SELECT EXISTS (
                SELECT 1 FROM factory_gate g
                WHERE g.run_id = r.id AND g.answer IS NULL
                    AND g.stand_in_answer IS NULL
            ) AS waiting
            FROM factory_run r
            WHERE r.user_id = $1
                AND r.ended_at IS NULL
                AND (
                    EXISTS (
                        SELECT 1 FROM factory_gate g
                        WHERE g.run_id = r.id AND g.answer IS NULL
                            AND g.stand_in_answer IS NULL
                    )
                    OR (
                        r.flow = ANY($2)
                        AND NOT EXISTS (SELECT 1 FROM factory_gate g WHERE g.run_id = r.id)
                    )
                )
            LIMIT $3
        ) places
        "#,
        user_id.as_ref(),
        &asking_flows,
        i64::from(limit),
    )
    .fetch_one(&mut **transaction)
    .await?;
    let count = |size: i64| {
        u16::try_from(size)
            .map_err(|_| PgRunRepositoryErr::OutOfRange(i32::try_from(size).unwrap_or(i32::MAX)))
    };
    Ok(QueuePlaces {
        waiting: count(places.waiting)?,
        starting: count(places.starting)?,
    })
}

/// A stored confidence, which the schema keeps between 0 and 100 percent.
fn percent(stored: i16) -> Result<u8, PgRunRepositoryErr> {
    u8::try_from(stored).map_err(|_| PgRunRepositoryErr::OutOfRange(i32::from(stored)))
}

/// A stored visit or place, which the schema keeps between 1 and 65535.
fn number(stored: i32) -> Result<u16, PgRunRepositoryErr> {
    u16::try_from(stored).map_err(|_| PgRunRepositoryErr::OutOfRange(stored))
}

impl TryFrom<StepRow> for Step {
    type Error = PgRunRepositoryErr;

    fn try_from(mut row: StepRow) -> Result<Self, Self::Error> {
        let gate = row.gate()?;
        let end = match (row.outcome, row.output, row.ended_at) {
            (Some(outcome), Some(output), Some(ended_at)) => Some(StepEnd {
                outcome: Outcome::try_from(outcome)?,
                output,
                ended_at,
            }),
            (None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        if row.passed_over.len() != row.pass_reasons.len() {
            return Err(PgRunRepositoryErr::PartialPassedOver);
        }
        let passed_over = row
            .passed_over
            .into_iter()
            .zip(row.pass_reasons)
            .map(|(agent, reason)| {
                Ok(PassedOver {
                    agent: AgentName::try_from(agent)?,
                    reason,
                })
            })
            .collect::<Result<_, InvalidName>>()?;
        Ok(Step {
            seq: number(row.seq)?,
            entry: EntryId::try_from(row.entry)?,
            visit: number(row.visit)?,
            started_at: row.started_at,
            end,
            gate,
            role: row.role.map(Role::try_from).transpose()?,
            agent: row.agent.map(AgentName::try_from).transpose()?,
            passed_over,
            checks: Vec::new(),
            terminal: StepTerminal {
                session_id: row.terminal_session_id,
                paused_at: row.paused_at,
                stop_asked_at: row.stop_asked_at,
                between_turns: row.between_turns,
                ends_by: None,
                pause_ran_out_at: row.pause_ran_out_at,
                steers: Vec::new(),
            },
        })
    }
}

impl RunRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    async fn insert_run(&self, run: NewRun<'_>) -> Result<RunInsert, Self::Err> {
        self.insert_run_in_queue(run, QueueHold::UNLIMITED).await
    }

    #[tracing::instrument(err, skip_all, fields(id = %run.id, limit = ?hold.limit))]
    async fn insert_run_in_queue(
        &self,
        run: NewRun<'_>,
        hold: QueueHold<'_>,
    ) -> Result<RunInsert, Self::Err> {
        // Every start takes a lock on its owner's Queue, so starts at the
        // same time, a person's, the Orchestrator's or an Automation's, are
        // taken one after the other: one under a limit counts the places
        // held, those of the starts before it among them, and keeps the Run
        // in the same transaction. A start under no limit takes the lock
        // too, so a limited start never counts while one is half kept.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL lock_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended('factory_queue:' || $1, 0))",
            run.user_id.as_ref(),
        )
        .execute(&mut *transaction)
        .await?;
        if let Some(limit) = hold.limit {
            let places =
                queue_places(&mut transaction, run.user_id, hold.asking_flows, limit).await?;
            if places.held() >= limit {
                return Ok(RunInsert::QueueFull(places));
            }
        }
        let inserted = sqlx::query_as!(
            RunRow,
            r#"
            INSERT INTO factory_run (
                id, flow, request, user_id, task_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, standing_rules, standing_rules_source,
                standing_rules_project_id, automation_id, automation_name, automation_trigger,
                automation_firing_key, project_id
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18
            )
            RETURNING
                id, request, user_id, flow, created_at, task_id, project_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, tampered_files, named_control_files, control_files_unlisted,
                standing_rules, standing_rules_source, standing_rules_project_id,
                automation_id, automation_name, automation_trigger, pull_request_url, outcome, ended_at
            "#,
            run.id.as_uuid(),
            run.flow,
            run.request,
            run.user_id.as_ref(),
            run.task_id.map(|task_id| task_id.to_string()),
            run.repository.map(|repository| repository.url.as_str()),
            run.repository
                .map(|repository| repository.base_branch.as_str()),
            run.created_in_person,
            run.control_file_patterns,
            run.control_files_team_id,
            &run.standing_rules.definition.kinds(),
            run.standing_rules.source.as_ref(),
            run.standing_rules.project_id,
            run.automation.map(|start| start.automation.id),
            run.automation.map(|start| start.automation.name.as_str()),
            run.automation.map(|start| start.automation.trigger.as_ref()),
            run.automation.map(|start| start.firing_key),
            run.project_id,
        )
        .fetch_one(&mut *transaction)
        .await;

        match inserted {
            Ok(row) => {
                let run = Run::try_from(row)?;
                transaction.commit().await?;
                Ok(RunInsert::Inserted(Box::new(run)))
            }
            Err(sqlx::Error::Database(error)) if error.constraint() == Some(TASK_FOREIGN_KEY) => {
                Ok(RunInsert::NoSuchTask)
            }
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some(AUTOMATION_TASK_OPEN_INDEX) =>
            {
                Ok(RunInsert::TaskBusy)
            }
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some(AUTOMATION_FIRING_INDEX) =>
            {
                Ok(RunInsert::FiringStarted)
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn link_task(&self, id: RunId, task_id: Uuid) -> Result<TaskLink, Self::Err> {
        let linked = sqlx::query!(
            "UPDATE factory_run SET task_id = $2 WHERE id = $1",
            id.as_uuid(),
            task_id.to_string(),
        )
        .execute(&self.pool)
        .await;

        match linked {
            Ok(done) if done.rows_affected() == 0 => Ok(TaskLink::NoSuchRun),
            Ok(_) => Ok(TaskLink::Linked),
            Err(sqlx::Error::Database(error)) if error.constraint() == Some(TASK_FOREIGN_KEY) => {
                Ok(TaskLink::NoSuchTask)
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_merge_head(&self, id: RunId, sha: &str) -> Result<(), Self::Err> {
        sqlx::query!(
            "UPDATE factory_run SET merge_head_sha = $2 WHERE id = $1",
            id.as_uuid(),
            sha,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_pull_request_url(&self, id: RunId, url: &str) -> Result<(), Self::Err> {
        sqlx::query!(
            "UPDATE factory_run SET pull_request_url = $2 WHERE id = $1",
            id.as_uuid(),
            url,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, changes))]
    async fn set_control_file_changes(
        &self,
        id: RunId,
        changes: Option<&ControlFileChanges>,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_run
            SET tampered_files = $2, named_control_files = $3, control_files_unlisted = $4
            WHERE id = $1
            "#,
            id.as_uuid(),
            changes.map(|changes| changes.tampered.as_slice()),
            changes.map(|changes| changes.named.as_slice()),
            changes.is_some_and(|changes| changes.unlisted),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn display_name(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Option<String>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT mui.first_name, mui.last_name
            FROM macro_user_info mui
            JOIN "User" u ON mui.macro_user_id = u.macro_user_id
            WHERE u.id = $1
            "#,
            user_id.as_ref(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| display_name(row.first_name.as_deref(), row.last_name.as_deref())))
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn is_agent_account(&self, user_id: &MacroUserIdStr<'static>) -> Result<bool, Self::Err> {
        let is_agent = sqlx::query_scalar!(
            r#"SELECT "isAgent" FROM "User" WHERE id = $1"#,
            user_id.as_ref(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(is_agent.unwrap_or(false))
    }

    // A person is on at most one team (`team_user_user_id_unique`), and its
    // owner is on it as `owner`.
    #[tracing::instrument(err, skip(self, user_id))]
    async fn team_of(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Option<TeamMembership>, Self::Err> {
        let membership = sqlx::query!(
            r#"
            SELECT tu.team_id, (tu.team_role IN ('admin', 'owner') AND NOT u."isAgent") AS "admin!"
            FROM team_user tu
            JOIN "User" u ON u.id = tu.user_id
            WHERE tu.user_id = $1
            LIMIT 1
            "#,
            user_id.as_ref(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(membership.map(|row| TeamMembership {
            team_id: row.team_id,
            admin: row.admin,
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn team_control_files(&self, team_id: Uuid) -> Result<Option<Vec<String>>, Self::Err> {
        let patterns = sqlx::query_scalar!(
            "SELECT control_files FROM factory_team_settings WHERE team_id = $1",
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(patterns.flatten())
    }

    // The row also keeps the team's Standing Rules: going back to the
    // default patterns clears them from it, and removes a row left with
    // neither setting.
    #[tracing::instrument(err, skip(self, patterns))]
    async fn set_team_control_files(
        &self,
        team_id: Uuid,
        patterns: Option<&[String]>,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query!(
            r#"
            INSERT INTO factory_team_settings (team_id, control_files)
            VALUES ($1, $2)
            ON CONFLICT (team_id)
            DO UPDATE SET control_files = EXCLUDED.control_files, updated_at = now()
            "#,
            team_id,
            patterns,
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query!(
            r#"
            DELETE FROM factory_team_settings
            WHERE team_id = $1 AND control_files IS NULL AND standing_rules IS NULL
            "#,
            team_id,
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn merge_head(&self, id: RunId) -> Result<Option<String>, Self::Err> {
        let sha = sqlx::query_scalar!(
            "SELECT merge_head_sha FROM factory_run WHERE id = $1",
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(sha.flatten())
    }

    #[tracing::instrument(err, skip(self))]
    async fn delete_run(&self, id: RunId) -> Result<(), Self::Err> {
        sqlx::query!("DELETE FROM factory_run WHERE id = $1", id.as_uuid())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn get_run(&self, id: RunId) -> Result<Option<Run>, Self::Err> {
        let row = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, flow, created_at, task_id, project_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, tampered_files, named_control_files, control_files_unlisted,
                standing_rules, standing_rules_source, standing_rules_project_id,
                automation_id, automation_name, automation_trigger, pull_request_url, outcome, ended_at
            FROM factory_run
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Run::try_from).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, Self::Err> {
        let rows = sqlx::query_as!(
            StepRow,
            r#"
            SELECT
                s.run_id,
                s.seq,
                s.entry,
                s.visit,
                s.started_at,
                s.outcome,
                s.output,
                s.ended_at,
                g.question AS "question?",
                g.answers AS "answers?",
                g.opened_at AS "opened_at?",
                g.answer,
                g.note,
                g.user_id,
                g.approved_control_files,
                g.rule,
                g.reply_confidence,
                g.by_stand_in AS "by_stand_in?",
                g.agent_session_id AS "agent_session_id?",
                g.asked_back_reply,
                g.asked_back_at,
                g.stand_in_answer,
                g.stand_in_reason,
                g.stand_in_confidence,
                g.stand_in_until,
                g.answered_at,
                s.role,
                s.agent,
                s.passed_over,
                s.pass_reasons,
                CASE WHEN s.ended_at IS NULL AND s.session_released_at IS NULL
                    THEN s.session_id END AS terminal_session_id,
                s.paused_at,
                s.stop_asked_at,
                s.steer_closed AND s.ended_at IS NULL AS "between_turns!",
                s.pause_ran_out_at
            FROM factory_step s
            LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE s.run_id = $1
            ORDER BY s.seq
            "#,
            id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        let mut rounds = self.runs_check_rounds(&[id.as_uuid()]).await?;
        let mut steers = self.runs_steers(&[id.as_uuid()]).await?;

        let mut steps = Vec::with_capacity(rows.len());
        for row in rows {
            let mut step = Step::try_from(row)?;
            step.checks = rounds.remove(&(id, step.seq)).unwrap_or_default();
            step.terminal.steers = steers.remove(&(id, step.seq)).unwrap_or_default();
            steps.push(step);
        }
        Ok(steps)
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn list_task_runs(
        &self,
        task_id: Uuid,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> Result<Vec<Run>, Self::Err> {
        // `factory_run_task_owner_idx` reads one person's Runs on the task
        // newest first and stops at the limit; the timeout bounds the read
        // all the same.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, flow, created_at, task_id, project_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, tampered_files, named_control_files, control_files_unlisted,
                standing_rules, standing_rules_source, standing_rules_project_id,
                automation_id, automation_name, automation_trigger, pull_request_url, outcome, ended_at
            FROM factory_run
            WHERE task_id = $1 AND user_id = $2
            ORDER BY created_at DESC, id DESC
            LIMIT $3
            "#,
            task_id.to_string(),
            user_id.as_ref(),
            i64::from(limit),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(Run::try_from).collect()
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn list_waiting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> Result<Vec<Run>, Self::Err> {
        // `factory_run_user_open_idx` reads the person's Runs that have not
        // ended, newest first; each is checked for an unanswered Gate by
        // the Gates' primary key, until the limit. The timeout bounds the
        // read all the same.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, flow, created_at, task_id, project_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, tampered_files, named_control_files, control_files_unlisted,
                standing_rules, standing_rules_source, standing_rules_project_id,
                automation_id, automation_name, automation_trigger, pull_request_url, outcome, ended_at
            FROM factory_run r
            WHERE r.user_id = $1
                AND r.ended_at IS NULL
                AND EXISTS (
                    SELECT 1 FROM factory_gate g
                    WHERE g.run_id = r.id AND g.answer IS NULL
                )
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $2
            "#,
            user_id.as_ref(),
            i64::from(limit),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(Run::try_from).collect()
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn list_starting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        asking_flows: &[&str],
        limit: u16,
    ) -> Result<Vec<Run>, Self::Err> {
        // As `list_waiting_runs`: the person's open Runs through
        // `factory_run_user_open_idx`, each checked for a Gate by the
        // Gates' primary key, until the limit, under a timeout.
        let asking_flows: Vec<String> = asking_flows
            .iter()
            .map(|flow| (*flow).to_string())
            .collect();
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, flow, created_at, task_id, project_id, repo_url, base_branch, created_in_person,
                control_file_patterns, control_files_team_id, tampered_files, named_control_files, control_files_unlisted,
                standing_rules, standing_rules_source, standing_rules_project_id,
                automation_id, automation_name, automation_trigger, pull_request_url, outcome, ended_at
            FROM factory_run r
            WHERE r.user_id = $1
                AND r.ended_at IS NULL
                AND r.flow = ANY($2)
                AND NOT EXISTS (SELECT 1 FROM factory_gate g WHERE g.run_id = r.id)
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $3
            "#,
            user_id.as_ref(),
            &asking_flows,
            i64::from(limit),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(Run::try_from).collect()
    }

    #[tracing::instrument(err, skip(self, ids), fields(runs = ids.len()))]
    async fn list_runs_steps(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<Step>>, Self::Err> {
        let ids: Vec<Uuid> = ids.iter().map(RunId::as_uuid).collect();
        let rows = sqlx::query_as!(
            StepRow,
            r#"
            SELECT
                s.run_id,
                s.seq,
                s.entry,
                s.visit,
                s.started_at,
                s.outcome,
                s.output,
                s.ended_at,
                g.question AS "question?",
                g.answers AS "answers?",
                g.opened_at AS "opened_at?",
                g.answer,
                g.note,
                g.user_id,
                g.approved_control_files,
                g.rule,
                g.reply_confidence,
                g.by_stand_in AS "by_stand_in?",
                g.agent_session_id AS "agent_session_id?",
                g.asked_back_reply,
                g.asked_back_at,
                g.stand_in_answer,
                g.stand_in_reason,
                g.stand_in_confidence,
                g.stand_in_until,
                g.answered_at,
                s.role,
                s.agent,
                s.passed_over,
                s.pass_reasons,
                CASE WHEN s.ended_at IS NULL AND s.session_released_at IS NULL
                    THEN s.session_id END AS terminal_session_id,
                s.paused_at,
                s.stop_asked_at,
                s.steer_closed AND s.ended_at IS NULL AS "between_turns!",
                s.pause_ran_out_at
            FROM factory_step s
            LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE s.run_id = ANY($1)
            ORDER BY s.run_id, s.seq
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut rounds = self.runs_check_rounds(&ids).await?;
        let mut steers = self.runs_steers(&ids).await?;

        let mut steps: BTreeMap<RunId, Vec<Step>> = BTreeMap::new();
        for row in rows {
            let run_id = RunId::from_uuid(row.run_id);
            let mut step = Step::try_from(row)?;
            step.checks = rounds.remove(&(run_id, step.seq)).unwrap_or_default();
            step.terminal.steers = steers.remove(&(run_id, step.seq)).unwrap_or_default();
            steps.entry(run_id).or_default().push(step);
        }
        Ok(steps)
    }

    #[tracing::instrument(err, skip(self), fields(entry = %entry))]
    async fn start_step(&self, key: StepKey, entry: &EntryId, visit: u16) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_step (run_id, seq, entry, visit)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            entry.as_str(),
            i32::from(visit),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, output), fields(outcome = %outcome))]
    async fn end_step(
        &self,
        key: StepKey,
        outcome: &Outcome,
        output: &str,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET outcome = $3, output = $4, ended_at = now()
            WHERE run_id = $1 AND seq = $2 AND ended_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            outcome.as_str(),
            output,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, question, answers))]
    async fn open_gate(
        &self,
        key: StepKey,
        question: &str,
        answers: &[Outcome],
    ) -> Result<(), Self::Err> {
        let answers: Vec<String> = answers.iter().map(|a| a.as_str().to_owned()).collect();
        sqlx::query!(
            r#"
            INSERT INTO factory_gate (run_id, seq, question, answers)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            question,
            &answers,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self), fields(answer = %answer, rule = %rule))]
    async fn answer_gate_by_rule(
        &self,
        key: StepKey,
        answer: &Outcome,
        rule: StandingRule,
    ) -> Result<Option<GateAnswer>, Self::Err> {
        let updated = sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = $3, rule = $4, answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.as_str(),
            rule.as_ref(),
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated == 0 {
            tracing::info!("the gate had an answer, or is not recorded: reading what it keeps");
        }

        // What the Gate keeps is read back, whichever answer that is.
        self.kept_answer(key).await
    }

    #[tracing::instrument(err, skip(self, note, user_id), fields(answer = %answer))]
    async fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
        approved_control_files: Option<&[String]>,
        agent_session: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        // The answer is recorded once the Run's workflow took it, so an
        // account deleted since the API checked it must not stop it: the
        // answer is kept with no one as its author.
        let answered = sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = $3, note = $4, user_id = $5, approved_control_files = $6,
                agent_session_id = $7, answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.as_str(),
            note,
            Some(user_id.as_ref()),
            approved_control_files,
            agent_session,
        )
        .execute(&self.pool)
        .await;
        match answered {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some(GATE_USER_FOREIGN_KEY) =>
            {
                sqlx::query!(
                    r#"
                    UPDATE factory_gate
                    SET answer = $3, note = $4, user_id = NULL, approved_control_files = $5,
                        agent_session_id = $6, answered_at = now()
                    WHERE run_id = $1 AND seq = $2 AND answer IS NULL
                    "#,
                    key.run_id.as_uuid(),
                    i32::from(key.seq),
                    answer.as_str(),
                    note,
                    approved_control_files,
                    agent_session,
                )
                .execute(&self.pool)
                .await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self, offered))]
    async fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        role: &Role,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, Self::Err> {
        let offered: Vec<String> = offered.iter().map(|o| o.as_str().to_owned()).collect();
        // The first attempt of a Step records its session; a retry finds it.
        let claimed = sqlx::query!(
            r#"
            UPDATE factory_step
            SET session_id = $3, action_id = $4, offered = $5, role = $6, agent = $7,
                bot_id = $8
            WHERE run_id = $1 AND seq = $2 AND session_id IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session.session_id,
            session.action_id,
            &offered,
            role.as_str(),
            session.agent.as_ref().map(AgentName::as_str),
            session.bot,
        )
        .execute(&self.pool)
        .await?;
        if claimed.rows_affected() == 1 {
            return Ok(ClaimedSession { session, new: true });
        }

        let recorded = self
            .claimed_session(key)
            .await?
            .ok_or(PgRunRepositoryErr::NoSuchStep)?;
        Ok(ClaimedSession {
            session: recorded,
            new: false,
        })
    }

    #[tracing::instrument(err, skip(self, from, to))]
    async fn pass_session(
        &self,
        key: StepKey,
        from: Uuid,
        role: &Role,
        passed: &PassedOver,
        to: Option<&StepSession>,
    ) -> Result<bool, Self::Err> {
        // A new session has not been asked to open, nor seen open, and the
        // agent handed nothing back in the one it leaves. With no next
        // session the Step keeps its own, and only the pass is recorded. A
        // session claimed before Steps recorded their Role and Agent gets
        // both, so they pair up with the Agents passed over. With no next
        // session, an Agent already recorded last is not recorded again, as
        // when overlapping attempts both find the Role exhausted. A Step
        // that took a later prompt has an Agent at work, and its checks
        // rounds go on in that session.
        let to_agent = to.and_then(|to| to.agent.as_ref()).map(AgentName::as_str);
        let recorded = sqlx::query!(
            r#"
            UPDATE factory_step s
            SET session_id = COALESCE($4, s.session_id),
                action_id = COALESCE($5, s.action_id),
                role = COALESCE(s.role, $6),
                agent = COALESCE($7, s.agent, $8),
                bot_id = CASE WHEN $4::uuid IS NULL THEN s.bot_id ELSE $10 END,
                passed_over = array_append(s.passed_over, $8),
                pass_reasons = array_append(s.pass_reasons, $9),
                prepared_at = CASE WHEN $4::uuid IS NULL THEN s.prepared_at END,
                opened_at = CASE WHEN $4::uuid IS NULL THEN s.opened_at END,
                steer_action_id = CASE WHEN $4::uuid IS NULL THEN s.steer_action_id END,
                steer_closed = CASE WHEN $4::uuid IS NULL THEN s.steer_closed ELSE false END
            FROM factory_run r
            WHERE r.id = s.run_id AND s.run_id = $1 AND s.seq = $2 AND s.session_id = $3
                AND s.session_released_at IS NULL AND s.handed_at IS NULL
                AND s.turn_action_id IS NULL
                AND s.ended_at IS NULL AND r.ended_at IS NULL
                AND (
                    $4::uuid IS NOT NULL
                    OR s.passed_over[cardinality(s.passed_over)] IS DISTINCT FROM $8
                )
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            from,
            to.map(|to| to.session_id),
            to.map(|to| to.action_id),
            role.as_str(),
            to_agent,
            passed.agent.as_str(),
            passed.reason,
            to.and_then(|to| to.bot),
        )
        .execute(&self.pool)
        .await?;
        Ok(recorded.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn claimed_session(&self, key: StepKey) -> Result<Option<StepSession>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT session_id AS "session_id!", action_id AS "action_id!", agent, bot_id
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StepSession {
                session_id: row.session_id,
                action_id: row.action_id,
                agent: row.agent.map(AgentName::try_from).transpose()?,
                bot: row.bot_id,
            })
        })
        .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_prepare(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, Self::Err> {
        let prepared_at = sqlx::query_scalar!(
            r#"
            UPDATE factory_step s
            SET prepared_at = now()
            FROM factory_run r
            WHERE r.id = s.run_id AND s.run_id = $1 AND s.seq = $2
                AND s.session_id = $3 AND s.session_released_at IS NULL
                AND s.ended_at IS NULL AND r.ended_at IS NULL
            RETURNING s.prepared_at AS "prepared_at!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(prepared_at)
    }

    #[tracing::instrument(err, skip(self))]
    async fn forget_prepare(
        &self,
        key: StepKey,
        prepared_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET prepared_at = NULL
            WHERE run_id = $1 AND seq = $2 AND prepared_at = $3 AND opened_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            prepared_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_open(&self, key: StepKey, session_id: Uuid) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET opened_at = now()
            WHERE run_id = $1 AND seq = $2
                AND session_id = $3 AND opened_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn stopping_session(
        &self,
        key: StepKey,
        lands_within: std::time::Duration,
    ) -> Result<Option<StoppingSession>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT
                session_id AS "session_id!",
                COALESCE(turn_action_id, action_id) AS "action_id!",
                agent,
                bot_id,
                opened_at IS NOT NULL AS "opened!",
                COALESCE(prepared_at > now() - make_interval(secs => $3), false)
                    AS "may_still_open!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            lands_within.as_secs_f64(),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StoppingSession {
                session: StepSession {
                    session_id: row.session_id,
                    action_id: row.action_id,
                    agent: row.agent.map(AgentName::try_from).transpose()?,
                    bot: row.bot_id,
                },
                opened: row.opened,
                may_still_open: row.may_still_open,
            })
        })
        .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn release_session(&self, key: StepKey) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET session_released_at = now()
            WHERE run_id = $1 AND seq = $2
                AND session_id IS NOT NULL AND session_released_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn holds_session(&self, key: StepKey, session_id: Uuid) -> Result<bool, Self::Err> {
        let holds = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM factory_step s
                JOIN factory_run r ON r.id = s.run_id
                WHERE s.run_id = $1 AND s.seq = $2
                    AND s.session_id = $3
                    AND s.session_released_at IS NULL
                    AND s.ended_at IS NULL
                    AND r.ended_at IS NULL
            ) AS "holds!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(holds)
    }

    #[tracing::instrument(err, skip(self))]
    async fn session_step(&self, session_id: Uuid) -> Result<Option<SessionStep>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT
                s.run_id,
                s.seq,
                r.user_id,
                s.offered AS "offered!",
                s.ended_at IS NOT NULL AS "ended!"
            FROM factory_step s
            JOIN factory_run r ON r.id = s.run_id
            WHERE s.session_id = $1
            ORDER BY s.ended_at IS NULL DESC, s.seq DESC
            LIMIT 1
            "#,
            session_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(SessionStep {
            key: StepKey {
                run_id: RunId::from_uuid(row.run_id),
                seq: number(row.seq)?,
            },
            owner: MacroUserIdStr::try_from(row.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
            offered: row
                .offered
                .into_iter()
                .map(Outcome::try_from)
                .collect::<Result<_, _>>()?,
            ended: row.ended,
        }))
    }

    #[tracing::instrument(err, skip(self, hand_back), fields(outcome = %hand_back.outcome))]
    async fn hand_back(&self, session_id: Uuid, hand_back: &HandBack) -> Result<bool, Self::Err> {
        let kept = sqlx::query!(
            r#"
            UPDATE factory_step
            SET handed_outcome = $2, handed_output = $3, handed_at = now()
            WHERE session_id = $1 AND ended_at IS NULL
            "#,
            session_id,
            hand_back.outcome.as_str(),
            hand_back.output,
        )
        .execute(&self.pool)
        .await?;
        Ok(kept.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn handed_back(&self, key: StepKey, action_id: Uuid) -> Result<HandedBack, Self::Err> {
        // The hand back kept is the latest prompt's: the first prompt's
        // until a later one is recorded.
        let row = sqlx::query!(
            r#"
            SELECT
                COALESCE(turn_action_id, action_id) = $3 AS "latest!",
                handed_outcome,
                handed_output
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            action_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) if !row.latest => Ok(HandedBack::Superseded),
            Some(row) => match (row.handed_outcome, row.handed_output) {
                (Some(outcome), Some(output)) => Ok(HandedBack::Handed(HandBack {
                    outcome: Outcome::try_from(outcome)?,
                    output,
                })),
                _ => Ok(HandedBack::Nothing),
            },
            // A Step with no session has no prompt yet.
            None => Ok(HandedBack::Nothing),
        }
    }

    #[tracing::instrument(err, skip(self, offered))]
    async fn begin_turn(
        &self,
        key: StepKey,
        session_id: Uuid,
        action_id: Uuid,
        place: u16,
        offered: &[Outcome],
    ) -> Result<bool, Self::Err> {
        self.record_turn(key, session_id, action_id, place, offered)
            .await
    }

    #[tracing::instrument(err, skip(self, steer), fields(action = steer.action().as_str()))]
    async fn steer(
        &self,
        key: StepKey,
        steer: &NewSteer,
    ) -> Result<Result<Steered, SteerRefusal>, Self::Err> {
        self.record_steer(key, steer).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn unsteer(
        &self,
        key: StepKey,
        prompt_id: Uuid,
        previous: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        self.undo_steer(key, prompt_id, previous).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn steering(&self, key: StepKey) -> Result<StepSteering, Self::Err> {
        self.read_steering(key).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn pause_ran_out(&self, key: StepKey) -> Result<(), Self::Err> {
        self.record_pause_ran_out(key).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn close_steering(
        &self,
        key: StepKey,
        turn: Uuid,
        followed: Option<Uuid>,
    ) -> Result<bool, Self::Err> {
        self.stop_steering(key, turn, followed).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn check_rounds(&self, key: StepKey) -> Result<Vec<CheckRound>, Self::Err> {
        self.step_check_rounds(key).await
    }

    #[tracing::instrument(err, skip(self, start), fields(round = start.round))]
    async fn start_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> Result<(), Self::Err> {
        self.insert_check_round(key, start).await
    }

    #[tracing::instrument(err, skip(self, report))]
    async fn end_check_round(
        &self,
        key: StepKey,
        round: u8,
        outcome: CheckRoundOutcome,
        report: &str,
    ) -> Result<(), Self::Err> {
        self.finish_check_round(key, round, outcome, report).await
    }

    #[tracing::instrument(err, skip(self, result), fields(check = %result.name))]
    async fn record_check_result(
        &self,
        key: StepKey,
        round: u8,
        commit: &str,
        result: &CheckResult,
    ) -> Result<(), Self::Err> {
        self.add_check_result(key, round, commit, result).await
    }

    #[tracing::instrument(err, skip(self, report))]
    async fn end_check_round_on_quota(
        &self,
        key: StepKey,
        round: u8,
        report: &str,
    ) -> Result<(), Self::Err> {
        self.quota_ends_check_round(key, round, report).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn send_back_check_round(
        &self,
        key: StepKey,
        round: u8,
        fix_action_id: Uuid,
    ) -> Result<Option<Uuid>, Self::Err> {
        self.fix_check_round(key, round, fix_action_id).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), Self::Err> {
        // The Run's end and, for a cancelled Run, its open Steps' ends land
        // together, so a Step can't end after the Run that cancelled it.
        let mut transaction = self.pool.begin().await?;
        let ended = sqlx::query!(
            r#"
            UPDATE factory_run
            SET outcome = $2, ended_at = now()
            WHERE id = $1 AND outcome IS NULL
            "#,
            id.as_uuid(),
            outcome.as_ref(),
        )
        .execute(&mut *transaction)
        .await?;
        if ended.rows_affected() == 1 && outcome == RunOutcome::Cancelled {
            sqlx::query!(
                r#"
                UPDATE factory_step
                SET outcome = 'cancelled', output = 'The run was cancelled.', ended_at = now()
                WHERE run_id = $1 AND ended_at IS NULL
                "#,
                id.as_uuid(),
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

/// Stands in for a name part a person never filled in.
const UNSET_NAME: &str = "N/A";

/// The name `first` and `last` make, as Macro shows a person: the parts they
/// set, none when they set neither.
fn display_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let part = |part: Option<&str>| {
        part.map(str::trim)
            .filter(|part| !part.is_empty() && *part != UNSET_NAME)
            .map(str::to_owned)
    };
    match (part(first), part(last)) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only),
        (Some(first), Some(last)) => Some(format!("{first} {last}")),
    }
}
