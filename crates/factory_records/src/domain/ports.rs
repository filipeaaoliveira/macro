//! The port the factory's records are kept behind.

use std::collections::BTreeMap;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{
    CheckResult, CheckRound, CheckRoundOutcome, CheckRoundStart, ClaimedSession,
    ControlFileChanges, EntryId, GateAnswer, HandBack, HandedBack, NewRun, Outcome, PassedOver,
    Role, Run, RunId, RunInsert, RunOutcome, SessionStep, Step, StepKey, StepSession,
    StoppingSession, TaskLink, TeamMembership,
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
    /// Tampered files it approved, if any. A Gate already answered keeps its
    /// first answer.
    fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
        approved_control_files: Option<&[String]>,
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

    /// The Step that works in the agent session `session_id`, if any.
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

    /// Record that the agent of the Step `key` works on a later prompt of
    /// its session `session_id`, `action_id`, and may hand back one of
    /// `offered`: the hand back of the prompt before is cleared, and a stop
    /// reads this prompt's progress. Recording the same prompt again changes
    /// nothing, so its hand back stays. `place` orders the Step's later
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

    /// Keep `sha` as the commit whose checks passed, which the Run asks a
    /// person to merge, replacing the one kept before: only that commit
    /// merges once they agree. Keeping it again changes nothing.
    fn set_merge_head(
        &self,
        id: RunId,
        sha: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep the Control Files the Run's branch changed, as last read,
    /// replacing what was kept before: none clears it. Keeping it again
    /// changes nothing.
    fn set_control_file_changes(
        &self,
        id: RunId,
        changes: Option<&ControlFileChanges>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

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
}
