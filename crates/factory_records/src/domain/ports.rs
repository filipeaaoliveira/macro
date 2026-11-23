//! The port the factory's records are kept behind.

use std::collections::BTreeMap;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::breaks::{NewBreakRecord, RunsBreakRecords};
use crate::domain::models::{
    BudgetStop, CheckResult, CheckRound, CheckRoundOutcome, CheckRoundStart, ClaimedSession,
    ControlFileChanges, DefinitionRead, EntryId, GateAnswer, HandBack, HandedBack, NewRun,
    NewSteer, Outcome, PassedOver, QueueHold, Role, Run, RunFlow, RunId, RunInsert, RunOutcome,
    SessionStep, SteerRefusal, Steered, Step, StepKey, StepSession, StepSteering, StoppingSession,
    TaskLink, TeamMembership,
};
use crate::domain::standing_rules::StandingRule;

/// Where Runs and their Steps are kept.
///
/// A Run's workflow repeats its writes when Temporal retries them, so every
/// write that a Step or a Run's end makes is safe to repeat. Steps are found
/// by their place in the Run ([`StepKey`]), which a reset never reuses.
pub trait RunRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep `run`, a new Run that has not started yet, working on its task
    /// document and in its repository if they are given, in one write.
    fn insert_run(
        &self,
        run: NewRun<'_>,
    ) -> impl Future<Output = Result<RunInsert, Self::Err>> + Send;

    /// Keep `run` as [`Self::insert_run`] does, held to its owner's Queue: a
    /// start waits behind a lock on that owner's Queue, so starts at the
    /// same time are taken one after the other, and under `hold`'s limit it
    /// is kept only while the owner holds fewer places than the limit
    /// (`RunInsert::QueueFull` otherwise, with the places counted).
    fn insert_run_in_queue(
        &self,
        run: NewRun<'_>,
        hold: QueueHold<'_>,
    ) -> impl Future<Output = Result<RunInsert, Self::Err>> + Send;

    /// Link the Run to the task document `task_id`, replacing any task it
    /// had. Linking it again changes nothing.
    fn link_task(
        &self,
        id: RunId,
        task_id: Uuid,
    ) -> impl Future<Output = Result<TaskLink, Self::Err>> + Send;

    /// Remove a Run whose workflow never started, with its Steps.
    fn delete_run(&self, id: RunId) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Run, if it exists.
    fn get_run(&self, id: RunId) -> impl Future<Output = Result<Option<Run>, Self::Err>> + Send;

    /// The Run's Steps, in order.
    fn list_steps(&self, id: RunId) -> impl Future<Output = Result<Vec<Step>, Self::Err>> + Send;

    /// The newest `limit` Runs of `user_id` that work on the task document
    /// `task_id`, newest first.
    fn list_task_runs(
        &self,
        task_id: Uuid,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<Run>, Self::Err>> + Send;

    /// The newest `limit` Runs of `user_id` that have not ended and wait on
    /// a Gate no one answered yet, newest first.
    fn list_waiting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<Run>, Self::Err>> + Send;

    /// `user_id`'s Runs that hold a place in their Queue before asking
    /// anything: those of `asking_flows` that have not ended or reached a
    /// Gate yet, newest first, up to `limit`.
    fn list_starting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        asking_flows: &[&str],
        limit: u16,
    ) -> impl Future<Output = Result<Vec<Run>, Self::Err>> + Send;

    /// The Steps of the Runs `ids`, each Run's in order. A Run with no Steps
    /// has no entry.
    fn list_runs_steps(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<BTreeMap<RunId, Vec<Step>>, Self::Err>> + Send;

    /// Record that the Step `key`, the `visit` of `entry`, started.
    /// Recording it again changes nothing.
    fn start_step(
        &self,
        key: StepKey,
        entry: &EntryId,
        visit: u16,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the Step `key` ended and what it produced. A Step that
    /// already ended keeps its first end.
    fn end_step(
        &self,
        key: StepKey,
        outcome: &Outcome,
        output: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the Step `key` waits for a person to answer `question`
    /// with one of `answers`. Recording it again changes nothing.
    fn open_gate(
        &self,
        key: StepKey,
        question: &str,
        answers: &[Outcome],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record `user_id`'s answer to the Gate of the Step `key`, with the
    /// Tampered files it approved, if any, and the agent session that gave
    /// it for them (`agent_session`), when the Orchestrator did. A Gate
    /// already answered keeps its first answer.
    fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
        approved_control_files: Option<&[String]>,
        agent_session: Option<Uuid>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record the Standing Rule `rule`'s answer to the Gate of the Step
    /// `key`: a Rule Answer, with no author or note, approving no Control
    /// Files. A Gate already answered keeps its first answer. Returns the
    /// answer the Gate keeps once written, read back; none when the Step
    /// has no Gate.
    fn answer_gate_by_rule(
        &self,
        key: StepKey,
        answer: &Outcome,
        rule: StandingRule,
    ) -> impl Future<Output = Result<Option<GateAnswer>, Self::Err>> + Send;

    /// Record that the Step `key` works in `session` for `role`, offering its
    /// agent `offered`, unless an earlier attempt of the Step already
    /// recorded a session: then that one is returned and kept.
    fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        role: &Role,
        offered: &[Outcome],
    ) -> impl Future<Output = Result<ClaimedSession, Self::Err>> + Send;

    /// Record that `passed`'s Agent, whose session `from` the Step `key` of
    /// `role` works in, could not start it, and hand the Step to `to`, a
    /// session of the next Agent of its Role, if there is one; with none,
    /// the Step keeps `from`. Only while the Step works in `from`, still
    /// holds it (see [`Self::holds_session`]) and took no later prompt (see
    /// [`Self::begin_turn`]): whether it was recorded.
    fn pass_session(
        &self,
        key: StepKey,
        from: Uuid,
        role: &Role,
        passed: &PassedOver,
        to: Option<&StepSession>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// The session the Step `key` works in, if it claimed one.
    fn claimed_session(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<StepSession>, Self::Err>> + Send;

    /// Record that an attempt of the Step `key` is about to ask for its
    /// session `session_id` to be opened, at the database's time. None once
    /// the Step let go of the session, works in another one, or it or its
    /// Run ended: no prepare may be sent then. The time recorded, for
    /// [`Self::forget_prepare`].
    fn record_prepare(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> impl Future<Output = Result<Option<chrono::DateTime<chrono::Utc>>, Self::Err>> + Send;

    /// Record that the prepare of the Step `key`'s session recorded at
    /// `prepared_at` is not in flight: it was refused, or never showed the
    /// session open. A later attempt's prepare, and a session an attempt saw
    /// open, keep their record.
    fn forget_prepare(
        &self,
        key: StepKey,
        prepared_at: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that an attempt of the Step `key` saw its session `session_id`
    /// open, while the Step works in it.
    fn record_open(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// How many of its repository's setup commands passed, in order, in
    /// the Step `key`'s session `session_id`: none for a session that ran
    /// none, as one the Step went on to.
    fn setup_passed(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> impl Future<Output = Result<u16, Self::Err>> + Send;

    /// Record that the first `passed` of its repository's setup commands
    /// passed in the Step `key`'s session `session_id`, while the Step
    /// holds it. False when it no longer does.
    fn record_setup_passed(
        &self,
        key: StepKey,
        session_id: Uuid,
        passed: u16,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// The session the Step `key` claimed, as a stop sees it: whether it
    /// opened, and whether a prepare sent less than `lands_within` ago may
    /// still open it.
    fn stopping_session(
        &self,
        key: StepKey,
        lands_within: std::time::Duration,
    ) -> impl Future<Output = Result<Option<StoppingSession>, Self::Err>> + Send;

    /// Record that the Step `key` let go of its session: stopped it for
    /// good, or is done with it. Recording it again changes nothing.
    fn release_session(&self, key: StepKey) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Whether the Step `key` still holds the session `session_id`: it works
    /// in it, neither the Step nor its Run ended, and the session was not
    /// let go. An attempt of the Step opens or prompts a session only while
    /// the Step holds it, so one handed on to another Agent is never opened
    /// or prompted again.
    fn holds_session(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// The Step that works in the agent session `session_id`, if any: the
    /// one that has not ended, else the latest. A Step that goes on after
    /// its agent's question works in the asking Step's session, so a
    /// session can have several Steps, at most one of them running.
    fn session_step(
        &self,
        session_id: Uuid,
    ) -> impl Future<Output = Result<Option<SessionStep>, Self::Err>> + Send;

    /// Record what the agent in `session_id` handed back, replacing an
    /// earlier hand back, while its Step has not ended. Whether it was kept:
    /// false once the Step ended.
    fn hand_back(
        &self,
        session_id: Uuid,
        hand_back: &HandBack,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// What the agent handed back for the prompt `action_id` of the Step
    /// `key`'s session, the first or a later one: only the latest prompt's
    /// hand back is kept, so a prompt the Step went on from reads as
    /// [`HandedBack::Superseded`], never with a later prompt's answer.
    fn handed_back(
        &self,
        key: StepKey,
        action_id: Uuid,
    ) -> impl Future<Output = Result<HandedBack, Self::Err>> + Send;

    /// Record that the Run's owner steers the running Step `key` with
    /// `steer`, and return its session, or why it takes no steering now.
    /// Only a Step that has not ended and holds its own session takes it,
    /// while its work waits on its agent's turn (see
    /// [`Self::close_steering`]); a stop asked refuses everything after
    /// it. A note goes to a Step that is not paused, a pause to one that is
    /// not, and a resume to one that is: a resume ends the pause. A note or a
    /// resume becomes the prompt the Step's work follows.
    fn steer(
        &self,
        key: StepKey,
        steer: &NewSteer,
    ) -> impl Future<Output = Result<Result<Steered, SteerRefusal>, Self::Err>> + Send;

    /// Record that the session refused the prompt `prompt_id` a note or a
    /// resume of the Step `key` sent: the steer is marked undelivered, the
    /// Step follows `previous` again, and a refused resume pauses the Step
    /// again. Changes nothing once the Step follows another prompt.
    fn unsteer(
        &self,
        key: StepKey,
        prompt_id: Uuid,
        previous: Option<Uuid>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The owner's steering of the Step `key`, as its work reads it while
    /// it waits on its agent.
    fn steering(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<StepSteering, Self::Err>> + Send;

    /// Record that the work of the paused Step `key` ended it because it
    /// stayed paused past its time. Recording it again changes nothing.
    fn pause_ran_out(&self, key: StepKey) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Stop the Step `key` taking its owner's steering, once its agent's
    /// turn on the factory's prompt `turn` ended and its work follows
    /// `followed`, the prompt of the owner's steering it read last (none
    /// when it followed none), and the Step is neither paused nor asked to
    /// stop. Whether the wait on `turn` ends: false when the owner steered
    /// it since, so the work reads the steering again; true, closing
    /// nothing, once the Step works on a later prompt than `turn`, whose
    /// wait the steering is now (an attempt retried late never closes it).
    /// A later prompt of the factory's ([`Self::begin_turn`]) lets the
    /// owner steer it again.
    fn close_steering(
        &self,
        key: StepKey,
        turn: Uuid,
        followed: Option<Uuid>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Record that the agent of the Step `key` works on a later prompt of
    /// its session `session_id`, `action_id`, and may hand back one of
    /// `offered`: the hand back of the prompt before is cleared, and a stop
    /// reads this prompt's progress; the owner may steer the Step again, and
    /// it follows this prompt rather than their latest. Recording the same
    /// prompt again changes nothing, so its hand back stays. `place` orders the Step's later
    /// prompts: a Step that ended, works in no session or another one, or
    /// already works on a prompt at `place` or later is left alone. Whether
    /// the Step works on the prompt now: false when it was left alone.
    fn begin_turn(
        &self,
        key: StepKey,
        session_id: Uuid,
        action_id: Uuid,
        place: u16,
        offered: &[Outcome],
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// The rounds of checks the Step `key` ran, in order.
    fn check_rounds(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Vec<CheckRound>, Self::Err>> + Send;

    /// Record that the Step `key` starts the round of checks `start`.
    /// Recording a round again changes nothing: the first record stays.
    fn start_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the round `round` of the Step `key` ended. A round that
    /// already ended keeps its first end.
    fn end_check_round(
        &self,
        key: StepKey,
        round: u8,
        outcome: CheckRoundOutcome,
        report: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how one check of the open round `round` of the Step `key`
    /// ran, which the engine ran itself on `commit`: the round is verified
    /// from then on, and keeps the commit of its first run. A check whose
    /// run is recorded keeps it, and a round that ended changes no more.
    fn record_check_result(
        &self,
        key: StepKey,
        round: u8,
        commit: &str,
        result: &CheckResult,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the agent's quota ran out on the round `round` of the
    /// Step `key`, during its checks or the fix of their failure, so no fix
    /// or round follows it: an open round ends failed with `report`, and
    /// one that already failed keeps its end. A round that passed is left
    /// alone. Recording it again changes nothing.
    fn end_check_round_on_quota(
        &self,
        key: StepKey,
        round: u8,
        report: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the failure of the round `round` of the Step `key` goes
    /// back to its agent in the prompt `fix_action_id`, unless it already
    /// went back in another. The prompt it goes back in; `None` when the
    /// round did not fail.
    fn send_back_check_round(
        &self,
        key: StepKey,
        round: u8,
        fix_action_id: Uuid,
    ) -> impl Future<Output = Result<Option<Uuid>, Self::Err>> + Send;

    /// Record how the break of the test `record.test` that the Step `key`'s
    /// agent reported went. A break whose record is kept keeps it, so a
    /// retried round records each break once, unless it was
    /// [`BreakStatus::Interrupted`](crate::domain::breaks::BreakStatus::Interrupted):
    /// a later run's record replaces that one.
    fn record_break(
        &self,
        key: StepKey,
        record: &NewBreakRecord,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The break records of the Runs `ids`, by Run: each Run's by Step, then
    /// in the order they were recorded. A Run with none is left out.
    fn runs_break_records(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<RunsBreakRecords, Self::Err>> + Send;

    /// Keep `sha` as the commit whose checks passed, which the Run asks a
    /// person to merge, replacing the one kept before: only that commit
    /// merges once they agree. Keeping it again changes nothing.
    fn set_merge_head(
        &self,
        id: RunId,
        sha: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep `url` as the page of the pull request the Run's branch proposes
    /// its change in, replacing the one kept before. Keeping it again
    /// changes nothing.
    fn set_pull_request_url(
        &self,
        id: RunId,
        url: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep how a wait of a `ci` Step on its pull request's checks ended,
    /// with the jobs it saw finish. A Step keeps its last wait: a retry
    /// that read another head or verdict, as green after a red on the same
    /// head once a failed job ran again, replaces the wait and its jobs, so
    /// the kept wait is the one the Step routed. Keeping the same head and
    /// verdict again changes nothing.
    fn record_ci_wait(
        &self,
        wait: &crate::domain::quality::CiWait,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The head the Run's last kept `ci` wait before the Step at `before`
    /// read, with the jobs it kept there, if one was kept.
    fn last_ci_wait(
        &self,
        run_id: RunId,
        before: u16,
    ) -> impl Future<Output = Result<Option<crate::domain::quality::LastCiWait>, Self::Err>> + Send;

    /// Keep `names`, the CI jobs the Run's Factory Definition names as
    /// known flakes, on the Run before its workflow starts. Keeping them
    /// again changes nothing.
    fn keep_known_ci_flakes(
        &self,
        id: RunId,
        names: &[String],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// What tells whether a red of the Run `id`'s pull request
    /// `pull_request` is flaky: the known flakes the Run keeps, the
    /// failures quarantined in its repository `repository` (its URL in
    /// lower case) whose reruns passed since `since`, and the failures
    /// other pull requests of the repository proved flaky since `since`: a
    /// rerun of the failed job passed, or the same head went green after
    /// it. Neither the Run nor its pull request counts.
    fn ci_flakes(
        &self,
        id: RunId,
        repository: &str,
        pull_request: Option<&str>,
        since: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<crate::domain::quality::CiFlakes, Self::Err>> + Send;

    /// The reruns the `ci` Step `key` asked for, by job name.
    fn ci_retries(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Vec<crate::domain::quality::CiRetry>, Self::Err>> + Send;

    /// Keep that the `ci` Step `key` asks GitHub to rerun `retries` on the
    /// commit `head_sha`, under the `rerun_flaky_ci` rule. A job the Step
    /// asked for before is kept as it was: a Step reruns a job once. The
    /// names this call kept, which only it may ask GitHub to rerun.
    fn request_ci_retries(
        &self,
        key: StepKey,
        head_sha: &str,
        retries: &[crate::domain::quality::CiRetryRequest],
    ) -> impl Future<Output = Result<Vec<String>, Self::Err>> + Send;

    /// Keep how the reruns of the `ci` Step `key` named in `settled` ended.
    /// A rerun that ended before keeps how it ended.
    fn settle_ci_retries(
        &self,
        key: StepKey,
        settled: &[(String, crate::domain::quality::CiRetryResult)],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Quarantine each failure of `jobs` in the repository `repository`
    /// (its URL in lower case) whose reruns there passed `threshold` or
    /// more times since `since`, for the Run `id` to file its task. A
    /// failure quarantined already keeps its task, and its passed retries
    /// and last pass are brought up to date. The names of the jobs newly
    /// quarantined.
    fn quarantine_ci_jobs(
        &self,
        id: RunId,
        repository: &str,
        jobs: &[crate::domain::quality::NewCiQuarantine],
        since: chrono::DateTime<chrono::Utc>,
        threshold: u32,
    ) -> impl Future<Output = Result<Vec<String>, Self::Err>> + Send;

    /// The quarantines the Run `id` made whose task is not filed yet.
    fn unfiled_ci_quarantines(
        &self,
        id: RunId,
    ) -> impl Future<Output = Result<Vec<crate::domain::quality::CiQuarantine>, Self::Err>> + Send;

    /// Keep that the task `task_id` of a quarantine is filed.
    fn mark_ci_quarantine_filed(
        &self,
        task_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Lift the quarantine whose task is `task_id`: its failure is no
    /// known flake from then on, until its reruns quarantine it again.
    /// Whether there was one.
    fn lift_ci_quarantine(
        &self,
        task_id: Uuid,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Keep the Control Files the Run's branch changed, as last read,
    /// replacing what was kept before: none clears it. Keeping it again
    /// changes nothing.
    fn set_control_file_changes(
        &self,
        id: RunId,
        changes: Option<&ControlFileChanges>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The name `user_id` shows others, from the first and last name they
    /// set; none when they set neither, or have no account.
    fn display_name(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Whether `user_id`'s account is an agent's (`"User"."isAgent"`): it
    /// signs in as any person does, yet is never one in person.
    fn is_agent_account(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// The team `user_id` is on, and whether they administer it (an agent's
    /// account never does); none when they are on no team.
    fn team_of(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Option<TeamMembership>, Self::Err>> + Send;

    /// The Control File patterns the team `team_id` set for its members'
    /// Runs; none when it set none, so the factory's default applies.
    fn team_control_files(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Option<Vec<String>>, Self::Err>> + Send;

    /// Set the Control File patterns of the team `team_id`'s Runs; none goes
    /// back to the factory's default.
    fn set_team_control_files(
        &self,
        team_id: Uuid,
        patterns: Option<&[String]>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The repository the team `team_id` names as its Factory Definition,
    /// by its `https://github.com/owner/name` URL; none when it names none.
    fn team_definition(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Name `repo_url` as the team `team_id`'s Factory Definition, kept as
    /// named by `named_by`, whose self-improvement Runs are theirs; none
    /// names none, so its Runs go back to the team's settings. In the same
    /// write, the team's open self-improvement Run on another Definition
    /// (repository names compared in any case) is settled as closed, its
    /// failures back to pending: it would hold them until the team named
    /// that Definition again. Its Run, if any, goes on for its owner.
    fn set_team_definition(
        &self,
        team_id: Uuid,
        repo_url: Option<&str>,
        named_by: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The commit of the Factory Definition `repo_url` (compared ignoring
    /// case) that the newest Run of the team `team_id`'s members started
    /// under it read; none when none of them started one under it.
    fn newest_definition_read(
        &self,
        team_id: Uuid,
        repo_url: &str,
    ) -> impl Future<Output = Result<Option<DefinitionRead>, Self::Err>> + Send;

    /// The Agents of the Factory Definition the Run `id` started under, as
    /// it kept them; none when it started under none.
    fn run_definition_agents(
        &self,
        id: RunId,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, Self::Err>> + Send;

    /// The Flow the Run `id` walks, as it keeps it; none when there is no
    /// such Run.
    fn run_flow(
        &self,
        id: RunId,
    ) -> impl Future<Output = Result<Option<RunFlow>, Self::Err>> + Send;

    /// The Flows the Runs `ids` walk, as each keeps it, by Run; a Run that
    /// is not there is left out.
    fn runs_flows(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<BTreeMap<RunId, RunFlow>, Self::Err>> + Send;

    /// The commit the Run last asked a person to merge, if any.
    fn merge_head(
        &self,
        id: RunId,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Record how the Run ended. A Run that already ended keeps its end. A
    /// cancelled Run's Steps that were still open end `cancelled`.
    fn end_run(
        &self,
        id: RunId,
        outcome: RunOutcome,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The agent sessions the Run `id`'s Steps worked in, each once, as
    /// Insights reads them: a Step that went on from its agent's question
    /// shares the asking Step's session, and a Step that went on in a new
    /// session (its first was lost, or its Agent could not start) counts
    /// the one it left too.
    fn step_sessions(&self, id: RunId)
    -> impl Future<Output = Result<Vec<Uuid>, Self::Err>> + Send;

    /// Keep why the Run `id`'s token budget ends it, before its end is
    /// recorded. A Run that already has a budget stop keeps the first, so a
    /// retry changes nothing; a Run with no budget keeps none.
    fn record_budget_stop(
        &self,
        id: RunId,
        stop: BudgetStop,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep that the cancel about to be sent to the Run `id` comes from its
    /// owner in person, while the Run has not ended. Once kept it stays: a
    /// later cancel by an agent does not undo the person's rejection.
    fn record_cancel_sender(
        &self,
        id: RunId,
        in_person: bool,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
