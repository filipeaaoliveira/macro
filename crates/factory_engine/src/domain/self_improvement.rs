//! Self-improvement: once a team's Runs under its Factory Definition have
//! failed often enough, or long enough ago, the factory starts one Run
//! against the Definition itself, whose pull request a person reviews.
//!
//! A failure is a live Scorer's judgment below [`PASS_SCORE`], or a Run its
//! owner marked flawed, of a Run (not a self-improvement Run) that started
//! under the Definition the team names now ([`collect`]). Once a Run of the
//! team ends and is scored, [`SelfImprovementServiceImpl`] settles the
//! team's open self-improvement Run, then starts one when the pending
//! failures reach the Definition's `selfImprovement` thresholds
//! ([`SelfImprovementDefinition`]): one Run of the built-in `improve` Flow
//! on the Definition's repository, owned by the admin who named the
//! Definition, whose request (and so its pull request's description) lists
//! the failures it groups under "Regressions addressed". Nothing changes
//! the workflow: the check runs in the scoring activity a Run's end already
//! calls.
//!
//! A person approves its plan and merges it, whatever it changes: the
//! `improve` Flow keeps those Gates for a person (`person_only`), which no
//! Standing Rule, stand-in or Orchestrator answers. Its failures are
//! addressed when its pull request merged, dismissed when a person
//! rejected it (closed its pull request unmerged, or cancelled its Run), so
//! they never start another, and go back to pending when its Run ended
//! without a pull request or could not start; then the team's next waits
//! ([`paused_until`]). New failures, of the same Scorer too, still count. The thresholds are the team's
//! Definition's as it is now, read as its namer, who must still be an admin
//! of the team ([`namer_standing`]).

#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use factory_records::domain::evaluation::PASS_SCORE;
use factory_records::domain::models::{RunId, RunOutcome};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::self_improvement::{
    DefinitionNaming, Failure, FailureSource, IMPROVE_FLOW, Improvement, ImprovementBegin,
    ImprovementState, NewFailure, RunFailureFacts, SelfImprovementRepository, Settlement,
};
use uuid::Uuid;

use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::definition::DefinitionError;
use crate::domain::evaluation::SelfImprovementDefinition;
use crate::domain::models::{
    CodeRepository, FactoryError, PullRequestState, StepError, run_branch,
};
use crate::domain::ports::{FactoryService, PullRequests, ScoringService};

/// The most failures one self-improvement Run groups: the oldest.
pub const MAX_GROUPED_FAILURES: u32 = 50;

/// How long after a self-improvement Run settled without merging (closed
/// or cancelled by a person, ended without a pull request, refused) the
/// team's next one waits; each such Run in a row since the last merged one
/// doubles it, up to [`MAX_UNMERGED_PAUSE`]. So a rejected or failing
/// improvement never starts another at once, and none loops unattended.
pub const UNMERGED_PAUSE: chrono::TimeDelta = chrono::TimeDelta::hours(24);

/// The longest pause after self-improvement Runs that did not merge.
pub const MAX_UNMERGED_PAUSE: chrono::TimeDelta = chrono::TimeDelta::days(7);

/// How many of a team's latest self-improvement Runs the pause reads: enough
/// for the doubling to reach [`MAX_UNMERGED_PAUSE`].
const RECENT_READ: u32 = 8;

/// How long a self-improvement Run may stay starting before it is taken
/// for one whose start was lost, and refused.
pub const STARTING_STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// How long an open self-improvement Run's Run may keep no Step before it
/// is taken for one whose workflow never started, and closed: as long as a
/// Step's work may take in all, so a started workflow has kept its first.
pub const NO_STEP_STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::hours(2);

/// How long after its Run ended a self-improvement Run whose pull request
/// cannot be read (its owner's GitHub access revoked, say) stays open
/// before it is closed.
pub const UNREAD_STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::days(7);

/// How long the thresholds read from a team's current Definition are kept
/// before it is read from GitHub again: a Run's end of the team reads
/// them at most this often.
pub const THRESHOLDS_KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The most characters of a failure's reason the request quotes.
const REASON_CHARS: usize = 400;

/// What the request says before the failures.
const REQUEST_BRIEF: &str = include_str!("self_improvement/request.md");

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> StepError {
    StepError::Internal(rootcause::Report::new(error).into_dynamic())
}

/// The failures `facts` holds, for a Run under its team's Definition: each
/// live Scorer's judgment below the pass score, and its flaw.
fn failures_of(facts: &RunFailureFacts) -> Vec<NewFailure> {
    let mut failures: Vec<NewFailure> = facts
        .judgments
        .iter()
        .filter(|(_, score, _)| *score < PASS_SCORE)
        .map(|(scorer, score, reason)| NewFailure {
            source: FailureSource::Scorer {
                name: scorer.clone(),
                score: *score,
            },
            reason: reason.clone(),
        })
        .collect();
    if facts.outcome == Some(RunOutcome::Flawed)
        && let Some(reason) = &facts.flaw_reason
    {
        failures.push(NewFailure {
            source: FailureSource::Flawed,
            reason: reason.clone(),
        });
    }
    failures
}

/// Whether `facts` is a Run whose failures count against `naming`: one of
/// the team's Definition, not a self-improvement Run.
fn counts_against(facts: &RunFailureFacts, naming: &DefinitionNaming) -> bool {
    facts.flow != IMPROVE_FLOW
        && facts
            .definition_repo_url
            .as_deref()
            .is_some_and(|repo_url| repo_url.eq_ignore_ascii_case(&naming.repo_url))
}

/// Until when the team's next self-improvement Run waits, given its
/// latest ones (`recent`, newest first, none open): after each that
/// settled without merging, in a row since the last that merged,
/// [`UNMERGED_PAUSE`] doubled for each but the first, up to
/// [`MAX_UNMERGED_PAUSE`], from the latest one's settlement. None after a
/// merged one, or none at all.
pub fn paused_until(recent: &[Improvement]) -> Option<DateTime<Utc>> {
    let unmerged = recent
        .iter()
        .take_while(|improvement| {
            matches!(
                improvement.state,
                ImprovementState::Closed | ImprovementState::Refused
            )
        })
        .count();
    let settled_at = recent.first()?.settled_at?;
    if unmerged == 0 {
        return None;
    }
    let doublings = u32::try_from(unmerged - 1).unwrap_or(u32::MAX).min(16);
    let pause = UNMERGED_PAUSE
        .checked_mul(1 << doublings)
        .unwrap_or(MAX_UNMERGED_PAUSE)
        .min(MAX_UNMERGED_PAUSE);
    Some(settled_at + pause)
}

/// Whether a self-improvement Run may be started as the Definition's
/// namer, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamerStanding {
    /// The namer is an admin of the team: their Runs start.
    Admin,
    /// No one is kept as naming the Definition (named before this was
    /// kept, or their account is gone).
    None,
    /// The namer left the team.
    LeftTeam,
    /// The namer is on the team but no longer an admin of it.
    NotAdmin,
}

/// Where the namer of `naming`, the team `team_id`'s Definition, stands,
/// with their id when they may start its self-improvement Run.
pub async fn namer_standing<R>(
    repo: &R,
    team_id: Uuid,
    naming: &DefinitionNaming,
) -> Result<(NamerStanding, Option<MacroUserIdStr<'static>>), StepError>
where
    R: RunRepository,
{
    let Some(namer) = naming.named_by.clone() else {
        return Ok((NamerStanding::None, None));
    };
    let membership = repo.team_of(&namer).await.map_err(internal)?;
    Ok(match membership {
        Some(team) if team.team_id == team_id && team.admin => (NamerStanding::Admin, Some(namer)),
        Some(team) if team.team_id == team_id => (NamerStanding::NotAdmin, None),
        _ => (NamerStanding::LeftTeam, None),
    })
}

/// What the team is shown of why a self-improvement Run could not start:
/// a short reason, never the error's own words.
fn refusal_reason(error: &FactoryError) -> &'static str {
    match error {
        FactoryError::InvalidDefinition(DefinitionError::ImprovementAgents(_)) => {
            "the definition's agents do not fill the improve flow's roles"
        }
        FactoryError::InvalidDefinition(_) | FactoryError::UnreachableDefinition => {
            "the definition does not read"
        }
        FactoryError::QueueFull { .. } => "the namer's queue is full",
        FactoryError::NoPushAccess
        | FactoryError::UnreachableRepository
        | FactoryError::InvalidRepository => "the namer cannot push to the definition's repository",
        _ => "the run could not start",
    }
}

/// Keep the failures of the Run `run_id` for its team, when it ran under
/// the Definition the team names now: its live Scorers' judgments below
/// the pass score, and its flaw. Safe to repeat. The facts it read, and the
/// team's Definition, for the caller to go on with.
pub async fn collect<R>(
    repo: &R,
    run_id: RunId,
) -> Result<Option<(RunFailureFacts, Uuid, DefinitionNaming)>, StepError>
where
    R: RunRepository + SelfImprovementRepository,
{
    let Some(facts) = repo.failure_facts(run_id).await.map_err(internal)? else {
        return Ok(None);
    };
    let Some(team) = repo.team_of(&facts.user_id).await.map_err(internal)? else {
        return Ok(None);
    };
    // A team that names no Definition collects nothing.
    let Some(naming) = repo
        .definition_naming(team.team_id)
        .await
        .map_err(internal)?
    else {
        return Ok(None);
    };
    if counts_against(&facts, &naming) {
        let failures = failures_of(&facts);
        if !failures.is_empty() {
            repo.record_failures(team.team_id, &naming.repo_url, run_id, &failures)
                .await
                .map_err(internal)?;
        }
    }
    Ok(Some((facts, team.team_id, naming)))
}

/// The first `chars` characters of `text` on one line, so a reason cannot
/// pass for a section of the request.
fn one_line(text: &str, chars: usize) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

/// A word joiner: invisible, and breaks what GitHub would read as a
/// mention or an issue reference.
const WORD_JOINER: char = '\u{2060}';

/// A failure's reason as the request quotes it: on one line
/// ([`one_line`]), with a word joiner after each `@` and `#`, so a reason
/// that reaches the pull request's description mentions no one and links
/// no issue.
fn quoted_reason(reason: &str) -> String {
    let mut quoted = String::new();
    for c in one_line(reason, REASON_CHARS).chars() {
        quoted.push(c);
        if c == '@' || c == '#' {
            quoted.push(WORD_JOINER);
        }
    }
    quoted
}

/// The request of a self-improvement Run grouping `failures`, with links
/// to each failing Run (`run_link`) and to the Scorers' judgments
/// (`judgments_link`). Its first line is its pull request's title; its
/// "Regressions addressed" section goes into the pull request's
/// description with it.
pub fn improvement_request(
    failures: &[Failure],
    run_link: impl Fn(RunId) -> String,
    judgments_link: &str,
) -> String {
    let mut scorers: Vec<(&str, Vec<&Failure>)> = Vec::new();
    let mut flawed: Vec<&Failure> = Vec::new();
    for failure in failures {
        match &failure.source {
            FailureSource::Scorer { name, .. } => {
                match scorers.iter_mut().find(|(scorer, _)| *scorer == name) {
                    Some((_, grouped)) => grouped.push(failure),
                    None => scorers.push((name, vec![failure])),
                }
            }
            FailureSource::Flawed => flawed.push(failure),
        }
    }

    let mut request = format!(
        "Improve the factory definition to address {} failed runs\n\n{}",
        failures.len(),
        REQUEST_BRIEF.trim_end()
    );
    request.push_str("\n\n## Regressions addressed\n");
    for (scorer, grouped) in &scorers {
        request.push_str(&format!(
            "\n### Scorer `{scorer}` ({}, [judgments]({judgments_link}))\n\n",
            grouped.len()
        ));
        for failure in grouped {
            let score = match failure.source {
                FailureSource::Scorer { score, .. } => format!("{score:.2}"),
                FailureSource::Flawed => String::new(),
            };
            request.push_str(&format!(
                "- [Run {}]({}) ({}), scored {score}: > {}\n",
                failure.run_id,
                run_link(failure.run_id),
                failure.flow,
                quoted_reason(&failure.reason)
            ));
        }
    }
    if !flawed.is_empty() {
        request.push_str(&format!("\n### Runs marked flawed ({})\n\n", flawed.len()));
        for failure in flawed {
            request.push_str(&format!(
                "- [Run {}]({}) ({}): > {}\n",
                failure.run_id,
                run_link(failure.run_id),
                failure.flow,
                quoted_reason(&failure.reason)
            ));
        }
    }
    request
}

/// Whether a team's pending failures, `count` of them the oldest kept at
/// `oldest`, call for a self-improvement Run under `thresholds` at `now`.
pub fn is_due(
    thresholds: SelfImprovementDefinition,
    count: u32,
    oldest: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    if !thresholds.enabled || count == 0 {
        return false;
    }
    let old_enough = oldest.is_some_and(|oldest| {
        now - oldest >= chrono::TimeDelta::days(i64::from(thresholds.oldest_days))
    });
    count >= u32::from(thresholds.failures) || old_enough
}

/// What a Run's end does for its team's self-improvement.
pub trait SelfImprovement: Send + Sync + 'static {
    /// Keep the failures of the Run `run_id`, which ended, settle its
    /// team's open self-improvement Run, and start one when the team's
    /// pending failures call for it.
    fn after_run_end(&self, run_id: RunId) -> impl Future<Output = Result<(), StepError>> + Send;
}

/// The thresholds of a team's Definition, by team and Definition
/// repository: none for a team the Run API found naming none. Each with
/// when it was read.
type KeptThresholds = HashMap<(Uuid, String), (Instant, Option<SelfImprovementDefinition>)>;

/// Self-improvement over the records, the Run API (which starts the Run as
/// any Run, as the Definition's namer), and GitHub, where an ended Run's
/// pull request is read.
pub struct SelfImprovementServiceImpl<R, F, G> {
    repo: R,
    runs: Arc<F>,
    github: G,
    app_url: String,
    /// The thresholds read lately, kept [`THRESHOLDS_KEPT_FOR`].
    thresholds: Mutex<KeptThresholds>,
}

impl<R, F, G> SelfImprovementServiceImpl<R, F, G> {
    /// The service, linking Runs in the Macro app at `app_url`.
    pub fn new(repo: R, runs: Arc<F>, github: G, app_url: &str) -> Self {
        Self {
            repo,
            runs,
            github,
            app_url: app_url.trim_end_matches('/').to_string(),
            thresholds: Mutex::new(HashMap::new()),
        }
    }
}

impl<R, F, G> SelfImprovementServiceImpl<R, F, G>
where
    R: RunRepository + SelfImprovementRepository,
    F: FactoryService,
    G: PullRequests,
{
    /// How the open self-improvement Run `open` is settled now, if it is:
    /// merged once its Run succeeded or its pull request merged; closed
    /// once its pull request closed, its Run ended without one, its Run
    /// kept no Step for [`NO_STEP_STALE_AFTER`], or its pull request stayed
    /// unread [`UNREAD_STALE_AFTER`] after its Run ended; refused when it
    /// stayed starting too long. None while its Run goes on, or its pull
    /// request is open.
    async fn settlement(&self, open: &Improvement) -> Result<Option<Settlement>, StepError> {
        let Some(run_id) = open.run_id else {
            return Ok(match open.state {
                ImprovementState::Starting
                    if Utc::now() - open.created_at > STARTING_STALE_AFTER =>
                {
                    Some(Settlement::Refused("its run never started".to_string()))
                }
                ImprovementState::Starting => None,
                // Its Run is gone.
                _ => Some(Settlement::Closed),
            });
        };
        let Some(facts) = self.repo.failure_facts(run_id).await.map_err(internal)? else {
            return Ok(Some(Settlement::Closed));
        };
        // How it settles with no pull request: rejected when its owner
        // cancelled its Run in person. A cancel by an agent, a bot under a
        // Standing Role, an internal caller or a program only means it did
        // not finish: its failures are pending again.
        let without_pull_request =
            if facts.outcome == Some(RunOutcome::Cancelled) && facts.cancelled_in_person {
                Settlement::Rejected
            } else {
                Settlement::Closed
            };
        match facts.outcome {
            // A Run kept, but whose workflow never started (its start
            // failed, and whether it was kept could not be read), would
            // hold its failures for good.
            None if Utc::now() - open.created_at > NO_STEP_STALE_AFTER => {
                let steps = self.repo.list_steps(run_id).await.map_err(internal)?;
                return Ok(steps.is_empty().then_some(Settlement::Closed));
            }
            None => return Ok(None),
            Some(RunOutcome::Success | RunOutcome::Flawed) => return Ok(Some(Settlement::Merged)),
            Some(_) => {}
        }
        // A Run that ended for a person may leave its pull request open,
        // which they may still merge or close.
        let Some(repository) = facts.repo_url.as_deref().and_then(CodeRepository::parse) else {
            return Ok(Some(without_pull_request));
        };
        let found = self
            .github
            .find(&facts.user_id, &repository, &run_branch(run_id))
            .await;
        Ok(match found {
            Ok(Some(pull_request)) => match pull_request.state {
                PullRequestState::Merged => Some(Settlement::Merged),
                PullRequestState::Open => None,
                // A person closed it unmerged.
                PullRequestState::Closed => Some(Settlement::Rejected),
            },
            Ok(None) => Some(without_pull_request),
            Err(error) => {
                tracing::warn!(?error, %run_id, "a self-improvement run's pull request was not read");
                let ended_at = self
                    .repo
                    .get_run(run_id)
                    .await
                    .map_err(internal)?
                    .and_then(|run| run.end)
                    .map(|end| end.ended_at);
                ended_at
                    .is_none_or(|ended_at| Utc::now() - ended_at > UNREAD_STALE_AFTER)
                    .then_some(Settlement::Closed)
            }
        })
    }

    /// The `selfImprovement` thresholds of the team's Definition as it is
    /// now, read as `owner`, its namer, as Insights reads them, never a
    /// copy an ended Run kept; kept [`THRESHOLDS_KEPT_FOR`]. None when the
    /// team names none, or it did not read.
    async fn current_thresholds(
        &self,
        team_id: Uuid,
        naming: &DefinitionNaming,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<SelfImprovementDefinition>, StepError> {
        let key = (team_id, naming.repo_url.to_ascii_lowercase());
        let now = Instant::now();
        if let Ok(mut kept) = self.thresholds.lock() {
            kept.retain(|_, (read_at, _)| now.duration_since(*read_at) < THRESHOLDS_KEPT_FOR);
            if let Some((_, thresholds)) = kept.get(&key) {
                return Ok(*thresholds);
            }
        }
        let thresholds = match self.runs.team_evaluation(owner).await {
            Ok(team) => team.map(|team| team.evaluation.self_improvement()),
            Err(error @ FactoryError::Internal(_)) => return Err(internal(error)),
            Err(error) => {
                tracing::warn!(%error, %team_id, "the team's definition did not read: no self-improvement run");
                return Ok(None);
            }
        };
        if let Ok(mut kept) = self.thresholds.lock() {
            kept.insert(key, (now, thresholds));
        }
        Ok(thresholds)
    }

    /// Start the team's self-improvement Run of the failures it claims, as
    /// `owner`, the Definition's namer; refused, and its failures back to
    /// pending, when the Run does not start.
    async fn start(
        &self,
        team_id: Uuid,
        naming: &DefinitionNaming,
        owner: MacroUserIdStr<'static>,
    ) -> Result<(), StepError> {
        let id = Uuid::now_v7();
        let claimed = self
            .repo
            .begin_improvement(id, team_id, &naming.repo_url, MAX_GROUPED_FAILURES)
            .await
            .map_err(internal)?;
        let ImprovementBegin::Claimed(failures) = claimed else {
            return Ok(());
        };
        let request = improvement_request(
            &failures,
            |run_id| format!("{}/app/factory/runs/{run_id}", self.app_url),
            &format!("{}/app/factory/insights", self.app_url),
        );
        let started = self
            .runs
            .create_improvement_run(owner, request, naming.repo_url.clone(), id)
            .await;
        match started {
            Ok(detail) => {
                tracing::info!(%team_id, run_id = %detail.run.id, failures = failures.len(), "a self-improvement run started");
            }
            Err(error) => {
                // A start that failed after the Run was kept and linked may
                // still have started its workflow: an improvement linked to
                // a Run that exists stays open, holding its failures, and
                // is settled by that Run.
                let linked = self
                    .repo
                    .latest_improvement(team_id)
                    .await
                    .map_err(internal)?
                    .filter(|latest| latest.id == id)
                    .and_then(|latest| latest.run_id);
                if let Some(run_id) = linked
                    && self.repo.get_run(run_id).await.map_err(internal)?.is_some()
                {
                    tracing::warn!(%error, %team_id, %run_id, "a self-improvement run's start failed, but its run is kept");
                    return Ok(());
                }
                tracing::warn!(%error, %team_id, "a self-improvement run was refused");
                let why = refusal_reason(&error).to_string();
                self.repo
                    .settle_improvement(id, &Settlement::Refused(why))
                    .await
                    .map_err(internal)?;
            }
        }
        Ok(())
    }
}

impl<R, F, G> SelfImprovement for SelfImprovementServiceImpl<R, F, G>
where
    R: RunRepository + SelfImprovementRepository,
    F: FactoryService,
    G: PullRequests,
{
    #[tracing::instrument(err, skip(self))]
    async fn after_run_end(&self, run_id: RunId) -> Result<(), StepError> {
        let Some((_, team_id, naming)) = collect(&self.repo, run_id).await? else {
            return Ok(());
        };

        // The team's open Run is settled first: only once it is may
        // another start.
        let latest = self
            .repo
            .latest_improvement(team_id)
            .await
            .map_err(internal)?;
        if let Some(open) = latest.as_ref().filter(|latest| latest.state.is_open()) {
            match self.settlement(open).await? {
                Some(settlement) => self
                    .repo
                    .settle_improvement(open.id, &settlement)
                    .await
                    .map_err(internal)?,
                None => return Ok(()),
            }
        }
        // Read after settling, so one settled just now pauses the next.
        let recent = self
            .repo
            .recent_improvements(team_id, RECENT_READ)
            .await
            .map_err(internal)?;
        if paused_until(&recent).is_some_and(|until| Utc::now() < until) {
            return Ok(());
        }

        let pending = self
            .repo
            .pending_failures(team_id, &naming.repo_url)
            .await
            .map_err(internal)?;
        if pending.count == 0 {
            return Ok(());
        }
        // The namer starts it only while an admin of the team, whose
        // Definition their Run reads.
        let (standing, owner) = namer_standing(&self.repo, team_id, &naming).await?;
        let Some(owner) = owner else {
            tracing::info!(%team_id, ?standing, "the definition's namer may not start a self-improvement run");
            return Ok(());
        };
        let Some(thresholds) = self.current_thresholds(team_id, &naming, &owner).await? else {
            return Ok(());
        };
        if !is_due(thresholds, pending.count, pending.oldest, Utc::now()) {
            return Ok(());
        }
        self.start(team_id, &naming, owner).await
    }
}

/// Scoring, then the Run's team's self-improvement: what the scoring
/// activity a Run's end calls does. A self-improvement step that fails is
/// logged and tried again on the activity's retry with the scoring.
pub struct ImprovingScoring<S, I> {
    scoring: S,
    improvement: I,
}

impl<S, I> ImprovingScoring<S, I> {
    /// `scoring`, followed by `improvement`.
    pub fn new(scoring: S, improvement: I) -> Self {
        Self {
            scoring,
            improvement,
        }
    }
}

impl<S, I> ScoringService for ImprovingScoring<S, I>
where
    S: ScoringService,
    I: SelfImprovement,
{
    async fn score_run(&self, run_id: RunId, last_attempt: bool) -> Result<(), StepError> {
        let scored = self.scoring.score_run(run_id, last_attempt).await;
        let improved = self.improvement.after_run_end(run_id).await;
        scored?;
        improved
    }
}
