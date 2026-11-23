//! The Postgres Run repository's Experiment arms, Scorers' judgments and
//! Benchmark suites ([`EvaluationRepository`]).

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};

use macro_user_id::user_id::MacroUserIdStr;
use sqlx::types::Json;
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::evaluation::{
    AgentChoices, ArmScore, ArmTally, BENCHMARK_FLOW, CalibrationTally, EvaluationRecords,
    EvaluationRepository, Judgment, MAX_OPEN_TRIALS, MAX_REASON_BYTES, NewSuite, PASS_SCORE,
    RuleTally, RunAgentRule, RunAgents, RunArm, RunJudgment, SUITE_STARTING_SECS, SUITES_READ,
    ScorerJudge, ScorerLabel, ScorerTally, ScoringStep, ScoringSubject, SubjectTrial, SuiteInsert,
    SuiteRecord, TrialKey, TrialRecord, TrialResult, TrialTokens,
};
use crate::domain::insights::InsightsScope;
use crate::domain::models::{AgentName, EntryId, Outcome, Role, RunId, RunOutcome};
use crate::domain::ports::RunRepository;

/// A count the schema keeps far below `u32::MAX`: insights read a capped
/// number of Runs.
fn count(stored: i64) -> u32 {
    u32::try_from(stored).unwrap_or(u32::MAX)
}

/// A token count as stored: never negative.
fn tokens(stored: i64) -> u64 {
    u64::try_from(stored).unwrap_or_default()
}

/// A token count to store: far below `i64::MAX` for any real trial.
fn stored_tokens(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

fn agents_json(agents: &AgentChoices) -> Result<serde_json::Value, PgRunRepositoryErr> {
    serde_json::to_value(agents).map_err(PgRunRepositoryErr::UnreadableAgents)
}

fn agents_of(stored: serde_json::Value) -> Result<AgentChoices, PgRunRepositoryErr> {
    serde_json::from_value(stored).map_err(PgRunRepositoryErr::UnreadableAgents)
}

fn run_outcome(stored: Option<String>) -> Result<Option<RunOutcome>, PgRunRepositoryErr> {
    stored
        .map(|outcome| {
            outcome
                .parse::<RunOutcome>()
                .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))
        })
        .transpose()
}

/// `reason` cut to at most [`MAX_REASON_BYTES`], on a character boundary.
fn kept_reason(reason: &str) -> &str {
    if reason.len() <= MAX_REASON_BYTES {
        return reason;
    }
    let mut end = MAX_REASON_BYTES;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    &reason[..end]
}

impl EvaluationRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, arms), fields(arms = arms.len()))]
    async fn record_arms(&self, run_id: RunId, arms: &[RunArm]) -> Result<(), Self::Err> {
        for arm in arms {
            sqlx::query!(
                r#"
                INSERT INTO factory_run_arm (run_id, experiment, arm, agents)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (run_id, experiment) DO NOTHING
                "#,
                run_id.as_uuid(),
                arm.experiment,
                arm.arm,
                agents_json(&arm.agents)?,
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    #[tracing::instrument(err, skip(self, rules), fields(rules = rules.len()))]
    async fn record_agent_rules(
        &self,
        run_id: RunId,
        rules: &[RunAgentRule],
    ) -> Result<(), Self::Err> {
        for rule in rules {
            sqlx::query!(
                r#"
                INSERT INTO factory_run_agent_rule (run_id, role, rule, agent)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (run_id, role) DO NOTHING
                "#,
                run_id.as_uuid(),
                rule.role.as_str(),
                rule.rule,
                rule.agent.as_str(),
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_task_tags_unread(&self, run_id: RunId) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_run_task_tags_unread (run_id)
            VALUES ($1)
            ON CONFLICT (run_id) DO NOTHING
            "#,
            run_id.as_uuid(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(runs = run_ids.len()))]
    async fn runs_task_tags_unread(&self, run_ids: &[RunId]) -> Result<BTreeSet<RunId>, Self::Err> {
        let ids: Vec<Uuid> = run_ids.iter().map(RunId::as_uuid).collect();
        let unread = sqlx::query_scalar!(
            r#"
            SELECT run_id
            FROM factory_run_task_tags_unread
            WHERE run_id = ANY($1)
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(unread.into_iter().map(RunId::from_uuid).collect())
    }

    #[tracing::instrument(err, skip_all, fields(runs = run_ids.len()))]
    async fn runs_agent_rules(
        &self,
        run_ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<RunAgentRule>>, Self::Err> {
        let ids: Vec<Uuid> = run_ids.iter().map(RunId::as_uuid).collect();
        let rows = sqlx::query!(
            r#"
            SELECT run_id, role, rule, agent
            FROM factory_run_agent_rule
            WHERE run_id = ANY($1)
            ORDER BY run_id, role
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut rules: BTreeMap<RunId, Vec<RunAgentRule>> = BTreeMap::new();
        for row in rows {
            rules
                .entry(RunId::from_uuid(row.run_id))
                .or_default()
                .push(RunAgentRule {
                    role: Role::try_from(row.role)?,
                    rule: row.rule,
                    agent: AgentName::try_from(row.agent)?,
                });
        }
        Ok(rules)
    }

    #[tracing::instrument(err, skip(self))]
    async fn run_agents(&self, run_id: RunId) -> Result<RunAgents, Self::Err> {
        // A Run takes Agents from its arms, or from its suite when it is a
        // trial. The definition's checks keep two arms of one Run from
        // choosing for the same Role.
        let rows = sqlx::query!(
            r#"
            SELECT agents AS "agents!: Json<serde_json::Value>", false AS "trial!"
            FROM factory_run_arm
            WHERE run_id = $1
            UNION ALL
            SELECT s.agents, true
            FROM factory_benchmark_trial t
            JOIN factory_benchmark_suite s ON s.id = t.suite_id
            WHERE t.run_id = $1
            "#,
            run_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        let mut agents = RunAgents::default();
        for row in rows {
            agents.choices.extend(agents_of(row.agents.0)?);
            agents.trial |= row.trial;
        }
        for rule in self
            .runs_agent_rules(&[run_id])
            .await?
            .remove(&run_id)
            .unwrap_or_default()
        {
            agents.ruled.insert(rule.role, rule.agent);
        }
        Ok(agents)
    }

    #[tracing::instrument(err, skip(self))]
    async fn trial_of(&self, run_id: RunId) -> Result<Option<SubjectTrial>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT t.suite_id, t.task, s.benchmark,
                   t.scored_at IS NOT NULL AS "scored!",
                   t.branch_deleted_at IS NOT NULL AS "branch_deleted!"
            FROM factory_benchmark_trial t
            JOIN factory_benchmark_suite s ON s.id = t.suite_id
            WHERE t.run_id = $1
            "#,
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| SubjectTrial {
            key: TrialKey {
                suite_id: row.suite_id,
                task: row.task,
            },
            benchmark: row.benchmark,
            scored: row.scored,
            branch_deleted: row.branch_deleted,
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn trial_base_commit(&self, run_id: RunId) -> Result<Option<String>, Self::Err> {
        Ok(sqlx::query_scalar!(
            "SELECT base_commit FROM factory_benchmark_trial WHERE run_id = $1",
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_branch_deleted(&self, trial: &TrialKey) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_benchmark_trial
            SET branch_deleted_at = now()
            WHERE suite_id = $1 AND task = $2 AND branch_deleted_at IS NULL
            "#,
            trial.suite_id,
            trial.task,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_planned_trials(&self, suite_id: Uuid, trials: u16) -> Result<(), Self::Err> {
        sqlx::query!(
            "UPDATE factory_benchmark_suite SET planned_trials = $2 WHERE id = $1",
            suite_id,
            i32::from(trials),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn delete_suite(&self, suite_id: Uuid) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            DELETE FROM factory_benchmark_suite s
            WHERE s.id = $1
              AND NOT EXISTS (SELECT 1 FROM factory_benchmark_trial t WHERE t.suite_id = s.id)
            "#,
            suite_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn scoring_subject(&self, run_id: RunId) -> Result<Option<ScoringSubject>, Self::Err> {
        let Some(run) = sqlx::query!(
            r#"
            SELECT r.user_id, r.flow, r.request, r.repo_url, r.base_branch, r.outcome,
                r.definition_evaluation, COALESCE(r.branch_run_id, r.id) AS "branch_of!"
            FROM factory_run r
            WHERE r.id = $1
            "#,
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let steps = sqlx::query!(
            r#"
            SELECT entry, outcome, output, session_id
            FROM factory_step
            WHERE run_id = $1
            ORDER BY seq
            "#,
            run_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|step| {
            Ok(ScoringStep {
                entry: EntryId::try_from(step.entry)?,
                outcome: step.outcome.map(Outcome::try_from).transpose()?,
                output: step.output,
                session_id: step.session_id,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        let trial = self.trial_of(run_id).await?;
        // A trial's judgments are its own; a live Run's are the live
        // Scorers'.
        let judged = match &trial {
            Some(trial) => {
                sqlx::query_scalar!(
                    r#"
                    SELECT scorer FROM factory_benchmark_trial_score
                    WHERE suite_id = $1 AND task = $2
                    ORDER BY scorer
                    "#,
                    trial.key.suite_id,
                    trial.key.task,
                )
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_scalar!(
                    "SELECT scorer FROM factory_run_score WHERE run_id = $1 ORDER BY scorer",
                    run_id.as_uuid(),
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        let mut sessions = RunRepository::step_sessions(self, run_id).await?;
        sessions.sort_unstable();

        Ok(Some(ScoringSubject {
            run_id,
            user_id: MacroUserIdStr::try_from(run.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(run.user_id))?,
            branch_of: RunId::from_uuid(run.branch_of),
            flow: run.flow,
            request: run.request,
            repo_url: run.repo_url,
            base_branch: run.base_branch,
            outcome: run_outcome(run.outcome)?,
            steps,
            sessions,
            trial,
            judged,
            evaluation: run.definition_evaluation,
        }))
    }

    #[tracing::instrument(err, skip(self, judgment), fields(scorer = %judgment.scorer))]
    async fn record_run_score(&self, run_id: RunId, judgment: &Judgment) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_run_score (run_id, scorer, score, reason, model, instructions_sha256)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (run_id, scorer) DO NOTHING
            "#,
            run_id.as_uuid(),
            judgment.scorer,
            judgment.score,
            kept_reason(&judgment.reason),
            judgment.model,
            judgment.instructions_digest,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, judgment), fields(scorer = %judgment.scorer))]
    async fn record_trial_score(
        &self,
        trial: &TrialKey,
        judgment: &Judgment,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_benchmark_trial_score (suite_id, task, scorer, score, reason, model)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (suite_id, task, scorer) DO NOTHING
            "#,
            trial.suite_id,
            trial.task,
            judgment.scorer,
            judgment.score,
            kept_reason(&judgment.reason),
            judgment.model,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, result))]
    async fn record_trial_result(
        &self,
        trial: &TrialKey,
        result: &TrialResult,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_benchmark_trial
            SET score = $3, input_tokens = $4, output_tokens = $5,
                cache_read_tokens = $6, cache_write_tokens = $7, scored_at = now()
            WHERE suite_id = $1 AND task = $2 AND scored_at IS NULL
            "#,
            trial.suite_id,
            trial.task,
            result.score,
            stored_tokens(result.tokens.input),
            stored_tokens(result.tokens.output),
            stored_tokens(result.tokens.cache_read),
            stored_tokens(result.tokens.cache_write),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, suite), fields(benchmark = suite.benchmark))]
    async fn insert_suite(&self, suite: &NewSuite<'_>) -> Result<SuiteInsert, Self::Err> {
        // One person's starts are counted one after the other: the lock on
        // their id holds a second start until the first is kept. Its trials
        // are recorded one by one after it, so a suite that started lately
        // and has fewer trials than it planned is still starting: the
        // trials it has yet to record count as running, and it runs. Any
        // other suite runs while one of its trials' Runs has not ended.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended('factory_benchmark_suite:' || $1, 0))",
            suite.user_id.as_ref(),
        )
        .execute(&mut *transaction)
        .await?;
        let open = sqlx::query!(
            r#"
            WITH suites AS (
                SELECT
                    s.benchmark,
                    (
                        SELECT count(*)
                        FROM factory_benchmark_trial t
                        JOIN factory_run r ON r.id = t.run_id
                        WHERE t.suite_id = s.id AND r.ended_at IS NULL
                    ) AS running_trials,
                    CASE
                        WHEN s.created_at > now() - make_interval(secs => $3) THEN greatest(
                            s.planned_trials - (
                                SELECT count(*) FROM factory_benchmark_trial t WHERE t.suite_id = s.id
                            ),
                            0
                        )
                        ELSE 0
                    END AS starting_trials
                FROM factory_benchmark_suite s
                WHERE s.user_id = $1
            )
            SELECT
                coalesce(sum(running_trials + starting_trials), 0)::bigint AS "open_trials!",
                coalesce(
                    bool_or(benchmark = $2 AND running_trials + starting_trials > 0),
                    false
                ) AS "running!"
            FROM suites
            "#,
            suite.user_id.as_ref(),
            suite.benchmark,
            SUITE_STARTING_SECS,
        )
        .fetch_one(&mut *transaction)
        .await?;
        if open.running {
            return Ok(SuiteInsert::Running);
        }
        let open_trials = count(open.open_trials);
        if open_trials + u32::from(suite.trials) > u32::from(MAX_OPEN_TRIALS) {
            return Ok(SuiteInsert::TooManyTrials(open_trials));
        }
        sqlx::query!(
            r#"
            INSERT INTO factory_benchmark_suite (id, user_id, benchmark, agents, planned_trials)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO NOTHING
            "#,
            suite.id,
            suite.user_id.as_ref(),
            suite.benchmark,
            agents_json(suite.agents)?,
            i32::from(suite.trials),
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SuiteInsert::Inserted)
    }

    #[tracing::instrument(err, skip(self))]
    async fn insert_trial(
        &self,
        trial: &TrialKey,
        run_id: RunId,
        base_commit: &str,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_benchmark_trial (suite_id, task, run_id, base_commit)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (suite_id, task) DO NOTHING
            "#,
            trial.suite_id,
            trial.task,
            run_id.as_uuid(),
            base_commit,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, scope), fields(flow = ?scope.flow, cap = scope.cap))]
    async fn read_evaluation(&self, scope: &InsightsScope) -> Result<EvaluationRecords, Self::Err> {
        // Every read sees one snapshot, as the Run insights' do.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;

        // The same Runs the Run insights read: the person's newest in the
        // period, up to the cap, through `factory_run_user_created_idx`.
        // Benchmark trials are left out.
        let ids = sqlx::query_scalar!(
            r#"
            SELECT id
            FROM factory_run
            WHERE user_id = $1
              AND created_at >= $2
              AND created_at < $3
              AND ($4::text IS NULL OR flow = $4)
              AND flow IS DISTINCT FROM $6
            ORDER BY created_at DESC, id DESC
            LIMIT $5
            "#,
            scope.user_id.as_ref(),
            scope.from,
            scope.to,
            scope.flow.as_deref(),
            i64::from(scope.cap),
            BENCHMARK_FLOW,
        )
        .fetch_all(&mut *transaction)
        .await?;

        let scorers = sqlx::query!(
            r#"
            SELECT scorer,
                   count(*) AS "judged!",
                   count(*) FILTER (WHERE score >= $2) AS "passed!",
                   avg(score) AS "mean_score!"
            FROM factory_run_score
            WHERE run_id = ANY($1)
            GROUP BY scorer
            ORDER BY scorer
            "#,
            &ids,
            PASS_SCORE,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| ScorerTally {
            scorer: row.scorer,
            judged: count(row.judged),
            passed: count(row.passed),
            mean_score: row.mean_score,
        })
        .collect();

        // A retry keeps the arms and Agent Rules of the Run it retries, so
        // a Run and its retries are one attempt of an arm or a rule: it is
        // counted once, as its newest Run, the one that was not retried,
        // ended, and the Steps that Run kept count with it.
        //
        // An arm's Agents are the same for each of its Runs while the
        // Experiment is unchanged; the newest Run's are shown. A Run fell
        // back when one of its Steps of a Role the arm chooses for was
        // taken by another Agent (the chosen one could not start, or the
        // agent definition no longer has it), or passed over the chosen one
        // and no Agent took it. A Run that ended `infrastructure` never had
        // its work run, so it is not among the arm's ended Runs.
        let arms = sqlx::query!(
            r#"
            SELECT a.experiment, a.arm,
                   (array_agg(a.agents ORDER BY r.created_at DESC))[1]
                       AS "agents!: Json<serde_json::Value>",
                   count(*) AS "runs!",
                   count(r.outcome) FILTER (WHERE r.outcome <> 'infrastructure') AS "ended!",
                   count(*) FILTER (WHERE r.outcome = 'success') AS "succeeded!",
                   count(*) FILTER (
                       WHERE EXISTS (
                           SELECT 1
                           FROM factory_step st
                           WHERE st.run_id = a.run_id
                             AND (
                                 (a.agents ->> st.role) <> st.agent
                                 OR (a.agents ->> st.role) = ANY (st.passed_over)
                             )
                       )
                   ) AS "fell_back!"
            FROM factory_run_arm a
            JOIN factory_run r ON r.id = a.run_id
            WHERE a.run_id = ANY($1)
              AND NOT EXISTS (SELECT 1 FROM factory_run n WHERE n.retried_from = a.run_id)
            GROUP BY a.experiment, a.arm
            ORDER BY a.experiment, a.arm
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(ArmTally {
                experiment: row.experiment,
                arm: row.arm,
                agents: agents_of(row.agents.0)?,
                runs: count(row.runs),
                ended: count(row.ended),
                succeeded: count(row.succeeded),
                fell_back: count(row.fell_back),
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        let arm_scores = sqlx::query!(
            r#"
            SELECT a.experiment, a.arm, s.scorer,
                   count(*) AS "judged!",
                   avg(s.score) AS "mean_score!"
            FROM factory_run_arm a
            JOIN factory_run_score s ON s.run_id = a.run_id
            WHERE a.run_id = ANY($1)
              AND NOT EXISTS (SELECT 1 FROM factory_run n WHERE n.retried_from = a.run_id)
            GROUP BY a.experiment, a.arm, s.scorer
            ORDER BY a.experiment, a.arm, s.scorer
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| ArmScore {
            experiment: row.experiment,
            arm: row.arm,
            scorer: row.scorer,
            judged: count(row.judged),
            mean_score: row.mean_score,
        })
        .collect();

        // As for an arm: a rule's Agent is the newest Run's, and a Run fell
        // back when another Agent took a Step of the Role, or the rule's
        // was passed over.
        let rules = sqlx::query!(
            r#"
            SELECT g.role, g.rule,
                   (array_agg(g.agent ORDER BY r.created_at DESC))[1] AS "agent!",
                   count(*) AS "runs!",
                   count(r.outcome) FILTER (WHERE r.outcome <> 'infrastructure') AS "ended!",
                   count(*) FILTER (WHERE r.outcome = 'success') AS "succeeded!",
                   count(*) FILTER (
                       WHERE EXISTS (
                           SELECT 1
                           FROM factory_step st
                           WHERE st.run_id = g.run_id
                             AND st.role = g.role
                             AND (st.agent <> g.agent OR g.agent = ANY (st.passed_over))
                       )
                   ) AS "fell_back!"
            FROM factory_run_agent_rule g
            JOIN factory_run r ON r.id = g.run_id
            WHERE g.run_id = ANY($1)
              AND NOT EXISTS (SELECT 1 FROM factory_run n WHERE n.retried_from = g.run_id)
            GROUP BY g.role, g.rule
            ORDER BY g.role, g.rule
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(RuleTally {
                role: Role::try_from(row.role)?,
                rule: row.rule,
                agent: AgentName::try_from(row.agent)?,
                runs: count(row.runs),
                ended: count(row.ended),
                succeeded: count(row.succeeded),
                fell_back: count(row.fell_back),
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        let suites = self.read_suites(&mut transaction, scope).await?;

        transaction.commit().await?;
        Ok(EvaluationRecords {
            scorers,
            arms,
            arm_scores,
            rules,
            suites,
        })
    }

    #[tracing::instrument(err, skip(self))]
    async fn run_judgments(&self, run_id: RunId) -> Result<Vec<RunJudgment>, Self::Err> {
        let rows = sqlx::query!(
            r#"
            SELECT s.scorer, s.score, s.reason, s.model, s.instructions_sha256, s.judged_at,
                   l.passed AS "passed?", l.labelled_at AS "labelled_at?"
            FROM factory_run_score s
            LEFT JOIN factory_scorer_label l ON l.run_id = s.run_id AND l.scorer = s.scorer
            WHERE s.run_id = $1
            ORDER BY s.scorer
            "#,
            run_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| RunJudgment {
                scorer: row.scorer,
                score: row.score,
                reason: row.reason,
                model: row.model,
                instructions_digest: row.instructions_sha256,
                judged_at: row.judged_at,
                label: row
                    .passed
                    .zip(row.labelled_at)
                    .map(|(passed, labelled_at)| ScorerLabel {
                        passed,
                        labelled_at,
                    }),
            })
            .collect())
    }

    #[tracing::instrument(err, skip(self))]
    async fn label_judgment(
        &self,
        run_id: RunId,
        scorer: &str,
        passed: bool,
    ) -> Result<Option<ScorerLabel>, Self::Err> {
        // Only a judgment that exists is labelled: no row is inserted for a
        // Scorer that did not judge the Run.
        let row = sqlx::query!(
            r#"
            INSERT INTO factory_scorer_label (run_id, scorer, passed)
            SELECT run_id, scorer, $3
            FROM factory_run_score
            WHERE run_id = $1 AND scorer = $2
            ON CONFLICT (run_id, scorer)
                DO UPDATE SET passed = EXCLUDED.passed, labelled_at = now()
            RETURNING passed, labelled_at
            "#,
            run_id.as_uuid(),
            scorer,
            passed,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| ScorerLabel {
            passed: row.passed,
            labelled_at: row.labelled_at,
        }))
    }

    #[tracing::instrument(err, skip_all, fields(judges = judges.len()))]
    async fn calibrations(
        &self,
        user_id: &MacroUserIdStr<'static>,
        judges: &[ScorerJudge],
    ) -> Result<Vec<CalibrationTally>, Self::Err> {
        let scorers: Vec<&str> = judges.iter().map(|judge| judge.scorer.as_str()).collect();
        let models: Vec<&str> = judges.iter().map(|judge| judge.model.as_str()).collect();
        let digests: Vec<&str> = judges
            .iter()
            .map(|judge| judge.instructions_digest.as_str())
            .collect();
        // Only a Run's owner labels it, so a person's labels are those of
        // their Runs. A label counts only on a judgment its Scorer's current
        // judge made: its model and its instructions' fingerprint.
        let rows = sqlx::query!(
            r#"
            SELECT l.scorer,
                   count(*) AS "labelled!",
                   count(*) FILTER (WHERE (s.score >= $2) = l.passed) AS "agreed!"
            FROM factory_scorer_label l
            JOIN factory_run r ON r.id = l.run_id
            JOIN factory_run_score s ON s.run_id = l.run_id AND s.scorer = l.scorer
            JOIN unnest($3::text[], $4::text[], $5::text[]) AS j (scorer, model, digest)
                ON j.scorer = s.scorer AND j.model = s.model AND j.digest = s.instructions_sha256
            WHERE r.user_id = $1
            GROUP BY l.scorer
            ORDER BY l.scorer
            "#,
            user_id.as_ref(),
            PASS_SCORE,
            &scorers as &[&str],
            &models as &[&str],
            &digests as &[&str],
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| CalibrationTally {
                scorer: row.scorer,
                labelled: count(row.labelled),
                agreed: count(row.agreed),
            })
            .collect())
    }
}

impl PgRunRepository {
    /// The person's suites started in the scope's period, newest first, with
    /// their trials.
    async fn read_suites(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        scope: &InsightsScope,
    ) -> Result<Vec<SuiteRecord>, PgRunRepositoryErr> {
        let suites = sqlx::query!(
            r#"
            SELECT id, benchmark, agents AS "agents: Json<serde_json::Value>", created_at
            FROM factory_benchmark_suite s
            WHERE user_id = $1 AND created_at >= $2 AND created_at < $3
              AND EXISTS (SELECT 1 FROM factory_benchmark_trial t WHERE t.suite_id = s.id)
            ORDER BY created_at DESC, id DESC
            LIMIT $4
            "#,
            scope.user_id.as_ref(),
            scope.from,
            scope.to,
            SUITES_READ,
        )
        .fetch_all(&mut **transaction)
        .await?;
        if suites.is_empty() {
            return Ok(Vec::new());
        }
        let suite_ids: Vec<Uuid> = suites.iter().map(|suite| suite.id).collect();

        let mut trials: BTreeMap<Uuid, Vec<TrialRecord>> = BTreeMap::new();
        let rows = sqlx::query!(
            r#"
            SELECT t.suite_id, t.task, t.run_id, r.created_at, r.ended_at, r.outcome, t.score,
                   t.input_tokens, t.output_tokens, t.cache_read_tokens, t.cache_write_tokens
            FROM factory_benchmark_trial t
            JOIN factory_run r ON r.id = t.run_id
            WHERE t.suite_id = ANY($1)
            ORDER BY t.suite_id, t.task
            "#,
            &suite_ids,
        )
        .fetch_all(&mut **transaction)
        .await?;
        for row in rows {
            let result = match (
                row.score,
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
            ) {
                (Some(score), Some(input), Some(output), Some(cache_read), Some(cache_write)) => {
                    Some(TrialResult {
                        score,
                        tokens: TrialTokens {
                            input: tokens(input),
                            output: tokens(output),
                            cache_read: tokens(cache_read),
                            cache_write: tokens(cache_write),
                        },
                    })
                }
                _ => None,
            };
            trials.entry(row.suite_id).or_default().push(TrialRecord {
                task: row.task,
                run_id: RunId::from_uuid(row.run_id),
                started_at: row.created_at,
                ended_at: row.ended_at,
                run_outcome: run_outcome(row.outcome)?,
                result,
                scores: Vec::new(),
            });
        }

        let scores = sqlx::query!(
            r#"
            SELECT suite_id, task, scorer, score
            FROM factory_benchmark_trial_score
            WHERE suite_id = ANY($1)
            ORDER BY suite_id, task, scorer
            "#,
            &suite_ids,
        )
        .fetch_all(&mut **transaction)
        .await?;
        for row in scores {
            let trial = trials
                .get_mut(&row.suite_id)
                .and_then(|trials| trials.iter_mut().find(|trial| trial.task == row.task));
            if let Some(trial) = trial {
                trial.scores.push((row.scorer, row.score));
            }
        }

        suites
            .into_iter()
            .map(|suite| {
                Ok(SuiteRecord {
                    id: suite.id,
                    benchmark: suite.benchmark,
                    agents: agents_of(suite.agents.0)?,
                    created_at: suite.created_at,
                    trials: trials.remove(&suite.id).unwrap_or_default(),
                })
            })
            .collect()
    }
}
