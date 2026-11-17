//! Review rounds: a review panel's reviewers review the Run's change at
//! once, each in a Step of its own, and hand back the Blockers they found,
//! with their file, lines, what is wrong and why. A round reviews one diff:
//! the Run's whole change in its first round, and in a later one the fix
//! diff, what the fix changed since the round before. Once its reviewers'
//! Steps end, a finding in that diff is a Blocker, which the fix gets; one
//! outside it becomes a follow-up task, in the project of the Run's task,
//! and holds nothing.

mod answer;
mod brief;
#[cfg(test)]
mod test;

use answer::place;
pub use answer::{ReviewerAnswer, ReviewerAnswerError, WHOLE_CHANGE};
pub use brief::{BRIEF_BUDGET, blockers_brief, carried_blockers, findings_block};

use factory_records::domain::models::StepKey;
use factory_records::domain::models::{Outcome, RunId};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    FindingClass, FindingKind, Lines, ReviewFinding, ReviewRound, ReviewRoundFailure,
    ReviewRoundOutcome, ReviewRoundRepository, ReviewRoundStart,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::{CodeRepository, StepError, run_branch};
use crate::domain::ports::{
    FollowUpMade, FollowUpTask, PullRequests, Retryable, ReviewService, TaskDirectory,
};

/// A review round for a Run's workflow to open, before its reviewers'
/// Steps start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRoundOpening {
    /// The Run.
    pub run_id: RunId,
    /// The place of the round's first Step.
    pub seq: u16,
    /// Which round of its entry, from 1.
    pub round: u16,
    /// How many reviewers it has.
    pub reviewers: u16,
}

/// A review round for a Run's workflow to close, once its reviewers'
/// Steps ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRoundClosing {
    /// The Run.
    pub run_id: RunId,
    /// The place of the round's first Step.
    pub seq: u16,
    /// Whether a reviewer's work failed for good.
    pub failed: bool,
    /// Whether it is the last round its entry runs: Blockers it still
    /// finds end the Run for a person rather than go to a fix no round
    /// would review.
    #[serde(default)]
    pub last: bool,
}

/// The outcome of a last round that still found Blockers.
pub const UNRESOLVED: &str = "unresolved";

/// A file a diff changed, and the lines of it the diff's hunks cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Its path after the change.
    pub path: String,
    /// Its path before, when the change renamed it.
    pub previous_path: Option<String>,
    /// The lines its hunks cover after the change, context included; none
    /// when the diff shows no hunks, as for a binary file, and the whole
    /// file counts as changed.
    pub hunks: Option<Vec<Lines>>,
}

impl ChangedFile {
    /// The file `path`, renamed from `previous_path`, with the unified
    /// diff `patch` GitHub gives for it.
    pub fn from_patch(path: String, previous_path: Option<String>, patch: Option<&str>) -> Self {
        Self {
            path,
            previous_path,
            hunks: patch.map(hunk_lines),
        }
    }
}

/// The lines each hunk of `patch` covers after the change, from its
/// `@@ -a,b +c,d @@` headers: `c` to `c + d - 1`, or line `c` alone for a
/// hunk that only removes lines.
fn hunk_lines(patch: &str) -> Vec<Lines> {
    patch.lines().filter_map(hunk_header).collect()
}

/// The lines after the change of the hunk whose header is `line`, if it
/// is one.
fn hunk_header(line: &str) -> Option<Lines> {
    let rest = line.strip_prefix("@@ -")?;
    let (_, after) = rest.split_once(" +")?;
    let (range, _) = after.split_once(" @@")?;
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start.parse::<u32>().ok()?, count.parse::<u32>().ok()?),
        None => (range.parse::<u32>().ok()?, 1),
    };
    let start = start.max(1);
    Some(Lines {
        start,
        end: start.saturating_add(count.saturating_sub(1)),
    })
}

/// Whether the finding at `path` and `lines` is in the diff that changed
/// `files`: its file changed, and its lines, when it names some, meet a
/// hunk of it.
pub fn in_diff(path: &str, lines: Option<Lines>, files: &[ChangedFile]) -> bool {
    let path = path.trim_start_matches("./").trim_start_matches('/');
    let Some(file) = files
        .iter()
        .find(|file| file.path == path || file.previous_path.as_deref() == Some(path))
    else {
        return false;
    };
    match (lines, &file.hunks) {
        (Some(lines), Some(hunks)) => hunks
            .iter()
            .any(|hunk| hunk.start <= lines.end && lines.start <= hunk.end),
        _ => true,
    }
}

/// The follow-up task a finding outside the diff under review becomes.
fn follow_up(run_id: RunId, round: &ReviewRound, finding: &ReviewFinding) -> FollowUpTask {
    let first_line = finding.problem.lines().next().unwrap_or_default();
    let name: String = first_line.chars().take(120).collect();
    let reviewer = finding
        .role
        .as_ref()
        .map(|role| format!("the {role}"))
        .unwrap_or_else(|| "a reviewer".to_string());
    FollowUpTask {
        name: format!("Follow-up: {name}"),
        markdown: format!(
            "{reviewer} found this in review round {} of factory run {run_id}, outside the \
             changes it reviewed, so it did not hold the pull request.\n\n\
             **Where:** {}\n\n**What is wrong:** {}\n\n**Why it matters:** {}\n",
            round.round,
            place(&finding.path, finding.lines),
            finding.problem,
            finding.why
        ),
    }
}

/// Whether the findings of `round` were all classified: none waits for
/// the round to end.
fn classified(round: &ReviewRound) -> bool {
    round.findings.iter().all(|finding| finding.kind.is_some())
}

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

/// A failure of GitHub as a Step's: tried again unless it would repeat.
fn github_failure(error: impl std::error::Error + Retryable + Send + Sync + 'static) -> StepError {
    if error.retryable() {
        StepError::Internal(internal(error))
    } else {
        StepError::Final(internal(error))
    }
}

/// Opens and closes a Run's review rounds, over the Run records, GitHub,
/// which says what the Run's branch holds, and the tasks follow-ups become.
#[derive(Debug, Clone)]
pub struct ReviewRoundsImpl<R, G, T> {
    repo: R,
    github: G,
    tasks: T,
}

impl<R, G, T> ReviewRoundsImpl<R, G, T> {
    /// The review rounds, over the Run records `repo`, GitHub and the
    /// tasks follow-ups become.
    pub fn new(repo: R, github: G, tasks: T) -> Self {
        Self {
            repo,
            github,
            tasks,
        }
    }
}

impl<R, G, T> ReviewRoundsImpl<R, G, T>
where
    R: RunRepository + ReviewRoundRepository,
    G: PullRequests,
    T: TaskDirectory,
{
    /// The Run's round that starts at `seq`.
    async fn round(&self, run_id: RunId, seq: u16) -> Result<ReviewRound, StepError> {
        self.repo
            .review_rounds(run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .find(|round| round.seq == seq)
            .ok_or_else(|| {
                StepError::Final(rootcause::report!("no review round starts at step {seq}"))
            })
    }

    /// Tell each finding of `round` not yet told whether it is in the diff
    /// the round reviewed. When GitHub cannot list that diff in full, each
    /// is a Blocker: a finding is never let go for lack of a diff.
    async fn classify(
        &self,
        run_id: RunId,
        round: &ReviewRound,
        owner: &macro_user_id::user_id::MacroUserIdStr<'static>,
        repository: &CodeRepository,
    ) -> Result<(), StepError> {
        let files = match self
            .github
            .changed_files(
                owner,
                repository,
                round.diff_base.as_deref().unwrap_or_default(),
                round.head_sha.as_deref().unwrap_or_default(),
            )
            .await
        {
            Ok(files) => files,
            Err(error) if error.retryable() => return Err(github_failure(error)),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "the reviewed diff could not be read: every finding blocks"
                );
                None
            }
        };

        let classes: Vec<FindingClass> = round
            .findings
            .iter()
            .filter(|finding| finding.kind.is_none())
            .map(|finding| {
                // A Blocker found again, or an answer that never read,
                // blocks wherever it is.
                let inside = finding.blocks_anywhere
                    || files
                        .as_deref()
                        .is_none_or(|files| in_diff(&finding.path, finding.lines, files));
                FindingClass {
                    seq: finding.seq,
                    place: finding.place,
                    kind: if inside {
                        FindingKind::Blocker
                    } else {
                        FindingKind::FollowUp
                    },
                }
            })
            .collect();
        self.repo
            .classify_findings(run_id, round.seq, &classes)
            .await
            .map_err(internal)?;
        Ok(())
    }
}

impl<R, G, T> ReviewService for ReviewRoundsImpl<R, G, T>
where
    R: RunRepository + ReviewRoundRepository,
    G: PullRequests,
    T: TaskDirectory,
{
    #[tracing::instrument(err, skip_all, fields(run_id = %opening.run_id, seq = opening.seq, round = opening.round))]
    async fn open_round(&self, opening: ReviewRoundOpening) -> Result<bool, StepError> {
        let run_id = opening.run_id;
        let run = self
            .repo
            .get_run(run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        if run.end.is_some() {
            return Err(StepError::RunEnded);
        }
        let rounds = self.repo.review_rounds(run_id).await.map_err(internal)?;
        // A retried opening keeps what the first one recorded.
        if let Some(recorded) = rounds.iter().find(|round| round.seq == opening.seq) {
            return Ok(recorded.failure.is_none());
        }

        let head = match self.head(&run, run_id).await? {
            Ok(head) => head,
            // A round that cannot open says why, on the Run, and fails.
            Err(why) => {
                let failure = ReviewRoundFailure {
                    seq: opening.seq,
                    round: opening.round,
                    reviewers: opening.reviewers,
                    failure: why.to_string(),
                };
                self.repo
                    .fail_review_round(run_id, &failure)
                    .await
                    .map_err(internal)?;
                return Ok(false);
            }
        };

        // A later round reviews the fix diff: what changed since the round
        // before it. A fix that changed nothing has the whole change
        // reviewed again.
        let before = rounds
            .iter()
            .rev()
            .find(|round| round.seq < opening.seq)
            .and_then(|round| round.head_sha.as_deref());
        let base_branch = run
            .repository
            .as_ref()
            .map(|repository| repository.base_branch.clone())
            .unwrap_or_default();
        let (diff_base, fix_diff) = match before {
            Some(before) if opening.round > 1 && before != head => (before.to_string(), true),
            _ => (base_branch, false),
        };
        let start = ReviewRoundStart {
            seq: opening.seq,
            round: opening.round,
            reviewers: opening.reviewers,
            head_sha: head,
            diff_base,
            fix_diff,
        };
        self.repo
            .start_review_round(run_id, &start)
            .await
            .map_err(internal)?;
        Ok(true)
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %closing.run_id, seq = closing.seq))]
    async fn close_round(&self, closing: ReviewRoundClosing) -> Result<Outcome, StepError> {
        let ended = self.closed(&closing).await?;
        // The last round's Blockers end the Run for a person: no round
        // would review a fix of them.
        if closing.last && ended == ReviewRoundOutcome::Changes {
            return Outcome::try_from(UNRESOLVED.to_string())
                .map_err(|error| StepError::Final(internal(error)));
        }
        outcome(ended)
    }
}

impl<R, G, T> ReviewRoundsImpl<R, G, T>
where
    R: RunRepository + ReviewRoundRepository,
    G: PullRequests,
    T: TaskDirectory,
{
    /// The commit of `run`'s branch on GitHub, or why a round cannot be
    /// opened on it.
    async fn head(
        &self,
        run: &factory_records::domain::models::Run,
        run_id: RunId,
    ) -> Result<Result<String, &'static str>, StepError> {
        let Some(repository) = run.repository.as_ref() else {
            return Ok(Err("The run works on no repository."));
        };
        let Some(code) = CodeRepository::parse(&repository.url) else {
            return Ok(Err("The run's repository URL does not read."));
        };
        let head = self
            .github
            .branch_head(&run.user_id, &code, &run_branch(run_id))
            .await;
        match head {
            Ok(Some(head)) => Ok(Ok(head)),
            Ok(None) => Ok(Err("The run's branch is not on GitHub.")),
            Err(error) if error.retryable() => Err(github_failure(error)),
            Err(_) => Ok(Err(
                "GitHub refused to show the run's branch to the run's owner.",
            )),
        }
    }

    /// End the round `closing` names, and return how it ended: the first
    /// end, when a retry finds one.
    async fn closed(&self, closing: &ReviewRoundClosing) -> Result<ReviewRoundOutcome, StepError> {
        let run_id = closing.run_id;
        let round = self.round(run_id, closing.seq).await?;
        if let Some(end) = &round.end {
            return Ok(end.outcome);
        }

        // A reviewer whose Step ended with no answer read, as one a worker
        // without review panels ran, reviewed nothing: the round fails.
        let steps = self.repo.list_steps(run_id).await.map_err(internal)?;
        let unanswered = steps.iter().any(|step| {
            round.step_seqs().contains(&step.seq)
                && step
                    .end
                    .as_ref()
                    .is_some_and(|end| end.outcome != Outcome::failed())
                && !round.answered.contains(&step.seq)
        });
        if closing.failed || unanswered {
            return self
                .end(run_id, round.seq, ReviewRoundOutcome::Failed)
                .await;
        }

        let run = self
            .repo
            .get_run(run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let code = run
            .repository
            .as_ref()
            .and_then(|repository| CodeRepository::parse(&repository.url))
            .ok_or_else(|| {
                StepError::Final(rootcause::report!(
                    "a review round needs the run's repository"
                ))
            })?;
        let mut round = round;
        if !classified(&round) {
            self.classify(run_id, &round, &run.user_id, &code).await?;
            round = self.round(run_id, closing.seq).await?;
        }
        self.make_follow_ups(&run, &round).await?;

        let ended = if round.blockers().next().is_some() {
            ReviewRoundOutcome::Changes
        } else {
            ReviewRoundOutcome::Pass
        };
        self.end(run_id, round.seq, ended).await
    }

    /// Make a task of each follow-up of `round` that has none. Each task is
    /// made with an id kept before it is asked for, so a retry finds the
    /// task rather than making a second. A follow-up document storage
    /// refuses stays on the round as one, with no task; one it could not
    /// answer fails the close, which is tried again. Either way it never
    /// holds the pull request.
    async fn make_follow_ups(
        &self,
        run: &factory_records::domain::models::Run,
        round: &ReviewRound,
    ) -> Result<(), StepError> {
        let waiting = round
            .findings
            .iter()
            .filter(|finding| finding.kind == Some(FindingKind::FollowUp))
            .filter(|finding| finding.follow_up_task_id.is_none());
        for finding in waiting {
            let key = StepKey {
                run_id: run.id,
                seq: finding.seq,
            };
            let Some(task_id) = self
                .repo
                .follow_up_key(key, finding.place, Uuid::now_v7())
                .await
                .map_err(internal)?
            else {
                continue;
            };
            let task = follow_up(run.id, round, finding);
            let made = self
                .tasks
                .create_follow_up(&run.user_id, run.task_id, &task, task_id)
                .await
                .map_err(|error| StepError::Internal(internal(error)))?;
            match made {
                FollowUpMade::Made => {
                    self.repo
                        .link_follow_up(key, finding.place, task_id)
                        .await
                        .map_err(internal)?;
                }
                FollowUpMade::Refused => {
                    tracing::warn!(
                        seq = finding.seq,
                        place = finding.place,
                        "document storage refused a follow-up task"
                    );
                }
            }
        }
        Ok(())
    }
}

impl<R, G, T> ReviewRoundsImpl<R, G, T>
where
    R: RunRepository + ReviewRoundRepository,
{
    /// End the round at `seq` with `ended`, and return the outcome it was
    /// recorded with: the first end, when a retry finds one.
    async fn end(
        &self,
        run_id: RunId,
        seq: u16,
        ended: ReviewRoundOutcome,
    ) -> Result<ReviewRoundOutcome, StepError> {
        let round = self
            .repo
            .end_review_round(run_id, seq, ended)
            .await
            .map_err(internal)?;
        Ok(round
            .and_then(|round| round.end)
            .map_or(ended, |end| end.outcome))
    }
}

/// A round's outcome as the Flow routes it.
fn outcome(ended: ReviewRoundOutcome) -> Result<Outcome, StepError> {
    Outcome::try_from(ended.as_str().to_string()).map_err(|error| StepError::Final(internal(error)))
}
