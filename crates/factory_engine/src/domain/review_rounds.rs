//! Review rounds: a review panel's reviewers review the Run's change at
//! once, each in a Step of its own, and hand back the Blockers they found,
//! with their file, lines, what is wrong and why. A round reviews one diff:
//! the Run's whole change in its first round, and in a later one the fix
//! diff, what the fix changed since the round before. Once its reviewers'
//! Steps end, a finding in that diff is a Blocker, which the fix gets; one
//! outside it becomes a follow-up task, in the project of the Run's task,
//! and holds nothing.

#[cfg(test)]
mod test;

use factory_records::domain::models::{MAX_OUTPUT_BYTES, Outcome, RunId};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    Finding, FindingClass, FindingKind, Lines, MAX_FINDING_PATH_BYTES, MAX_FINDING_TEXT_BYTES,
    MAX_FINDINGS, ReviewFinding, ReviewRound, ReviewRoundOutcome, ReviewRoundRepository,
    ReviewRoundStart,
};
use serde::{Deserialize, Serialize};

use crate::domain::models::{CodeRepository, StepError, run_branch};
use crate::domain::ports::{FollowUpTask, PullRequests, Retryable, ReviewService, TaskDirectory};

/// A review round for a Run's workflow to open, before its reviewers'
/// Steps start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct ReviewRoundClosing {
    /// The Run.
    pub run_id: RunId,
    /// The place of the round's first Step.
    pub seq: u16,
    /// Whether a reviewer's work failed for good.
    pub failed: bool,
}

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

/// What a reviewer handed back: what it says of the change, and the
/// Blockers it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerAnswer {
    /// Its text, without the Blockers' block.
    pub summary: String,
    /// Its Blockers, in the order it gave them.
    pub findings: Vec<Finding>,
}

/// Why a reviewer's hand back does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReviewerAnswerError {
    /// It has no `json` block, or the block is not a list of Blockers.
    #[error("the reviewer's text has no json block listing its blockers: {0}")]
    NoBlockers(String),
    /// A Blocker has no file, problem or why, one too long, or lines that
    /// do not read.
    #[error("blocker {0} needs a file, a problem and why, each short enough, and lines from 1")]
    InvalidBlocker(usize),
    /// It lists more Blockers than a round keeps.
    #[error("a reviewer lists at most {MAX_FINDINGS} blockers")]
    TooManyBlockers,
    /// It passes the change and lists Blockers, or asks for changes and
    /// lists none.
    #[error("a reviewer that passes lists no blocker, and one that asks for changes lists some")]
    OutcomeAndBlockers,
}

/// A Blocker as the reviewer writes it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WrittenBlocker {
    file: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
    problem: String,
    why: String,
}

/// The block a reviewer ends its text with.
#[derive(Deserialize)]
struct WrittenBlockers {
    blockers: Vec<WrittenBlocker>,
}

impl ReviewerAnswer {
    /// Read what a reviewer handed back with `outcome`: `text` ends with a
    /// fenced `json` block, `{"blockers": [...]}`, which `pass` leaves
    /// empty and `changes` does not.
    pub fn read(outcome: &Outcome, text: &str) -> Result<Self, ReviewerAnswerError> {
        let (summary, block) = json_block(text)
            .ok_or_else(|| ReviewerAnswerError::NoBlockers("no fenced json block".to_string()))?;
        let written: WrittenBlockers = serde_json::from_str(block)
            .map_err(|error| ReviewerAnswerError::NoBlockers(error.to_string()))?;
        if written.blockers.len() > MAX_FINDINGS {
            return Err(ReviewerAnswerError::TooManyBlockers);
        }

        let mut findings = Vec::with_capacity(written.blockers.len());
        for (index, blocker) in written.blockers.into_iter().enumerate() {
            findings.push(finding(blocker).ok_or(ReviewerAnswerError::InvalidBlocker(index + 1))?);
        }
        let passes = outcome.as_str() == ReviewRoundOutcome::Pass.as_str();
        if passes != findings.is_empty() {
            return Err(ReviewerAnswerError::OutcomeAndBlockers);
        }
        Ok(Self { summary, findings })
    }

    /// The answer for a person and the Run's later Steps to read: its
    /// summary, then its Blockers, cut to what a Step's output keeps.
    pub fn render(&self) -> String {
        let mut text = self.summary.clone();
        if !self.findings.is_empty() {
            text.push_str("\n\n### Blockers\n");
            for (index, finding) in self.findings.iter().enumerate() {
                text.push_str(&format!("\n{}. {}", index + 1, describe(finding)));
            }
            text.push('\n');
        }
        cut_to_output(text)
    }
}

/// `text` without its last fenced `json` block, trimmed, and the block.
fn json_block(text: &str) -> Option<(String, &str)> {
    let open = text.rfind("```json")?;
    let body_start = open + "```json".len();
    let close = body_start + text[body_start..].find("```")?;
    let summary = format!("{}{}", &text[..open], &text[close + 3..]);
    Some((summary.trim().to_string(), text[body_start..close].trim()))
}

/// A written Blocker as a finding, if it holds together: a file, a problem
/// and why, each within its bound, and lines from 1, the last not before
/// the first. A first line alone is that one line.
fn finding(blocker: WrittenBlocker) -> Option<Finding> {
    let path = blocker.file.trim();
    let path = path
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    let problem = blocker.problem.trim().to_string();
    let why = blocker.why.trim().to_string();
    let fits = |text: &str, max: usize| !text.is_empty() && text.len() <= max;
    if !fits(&path, MAX_FINDING_PATH_BYTES)
        || !fits(&problem, MAX_FINDING_TEXT_BYTES)
        || !fits(&why, MAX_FINDING_TEXT_BYTES)
    {
        return None;
    }

    let lines = match (blocker.start_line, blocker.end_line) {
        (None, None) => None,
        (Some(start), end) => {
            let end = end.unwrap_or(start);
            let readable = start >= 1 && end >= start && i32::try_from(end).is_ok();
            if !readable {
                return None;
            }
            Some(Lines { start, end })
        }
        (None, Some(_)) => return None,
    };
    Some(Finding {
        path,
        lines,
        problem,
        why,
    })
}

/// Where a finding is: its file, and its lines when it names some.
fn place(path: &str, lines: Option<Lines>) -> String {
    match lines {
        Some(Lines { start, end }) if start == end => format!("`{path}` line {start}"),
        Some(Lines { start, end }) => format!("`{path}` lines {start}-{end}"),
        None => format!("`{path}`"),
    }
}

/// One finding, for a person or an agent to read.
fn describe(finding: &Finding) -> String {
    format!(
        "{}: {}\n   Why: {}",
        place(&finding.path, finding.lines),
        finding.problem,
        finding.why
    )
}

/// What ends a Step's output cut to fit.
const CUT_NOTE: &str = "\n\n(The rest is left out: the review round lists every blocker.)\n";

/// `text`, cut to what a Step's output keeps.
fn cut_to_output(mut text: String) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    let mut end = MAX_OUTPUT_BYTES - CUT_NOTE.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(CUT_NOTE);
    text
}

/// The Blockers of `round`, for the fix to read: each with where it is,
/// what is wrong and why, and the Role of the reviewer that found it.
pub fn blockers_brief(round: &ReviewRound) -> String {
    let mut brief = String::new();
    for (index, blocker) in round.blockers().enumerate() {
        let role = blocker
            .role
            .as_ref()
            .map(|role| format!(" (found by the {role})"))
            .unwrap_or_default();
        brief.push_str(&format!(
            "\n{}. {}: {}{role}\n   Why: {}",
            index + 1,
            place(&blocker.path, blocker.lines),
            blocker.problem,
            blocker.why
        ));
    }
    brief
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
            .changed_files(owner, repository, &round.diff_base, &round.head_sha)
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
                let inside = files
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
    async fn open_round(&self, opening: ReviewRoundOpening) -> Result<(), StepError> {
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
        // A retried opening keeps the commit the first one recorded.
        if rounds.iter().any(|round| round.seq == opening.seq) {
            return Ok(());
        }

        let repository = run.repository.as_ref().ok_or_else(|| {
            StepError::Final(rootcause::report!(
                "a review round needs the run's repository"
            ))
        })?;
        let code = CodeRepository::parse(&repository.url).ok_or_else(|| {
            StepError::Final(rootcause::report!("the run's repository URL does not read"))
        })?;
        let head = self
            .github
            .branch_head(&run.user_id, &code, &run_branch(run_id))
            .await
            .map_err(github_failure)?
            .ok_or_else(|| {
                StepError::Final(rootcause::report!("the run's branch is not on GitHub"))
            })?;

        // A later round reviews the fix diff: what changed since the round
        // before it. A fix that changed nothing has the whole change
        // reviewed again, so the Blockers it left are found again.
        let before = rounds.iter().rev().find(|round| round.seq < opening.seq);
        let (diff_base, fix_diff) = match before {
            Some(before) if opening.round > 1 && before.head_sha != head => {
                (before.head_sha.clone(), true)
            }
            _ => (repository.base_branch.clone(), false),
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
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %closing.run_id, seq = closing.seq))]
    async fn close_round(&self, closing: ReviewRoundClosing) -> Result<Outcome, StepError> {
        let run_id = closing.run_id;
        let round = self.round(run_id, closing.seq).await?;
        if let Some(end) = &round.end {
            return outcome(end.outcome);
        }
        if closing.failed {
            self.end(run_id, round.seq, ReviewRoundOutcome::Failed)
                .await?;
            return outcome(ReviewRoundOutcome::Failed);
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

        // A follow-up that could not become a task stays on the round as
        // one: it never holds the pull request.
        let waiting = round
            .findings
            .iter()
            .filter(|finding| finding.kind == Some(FindingKind::FollowUp))
            .filter(|finding| finding.follow_up_task_id.is_none());
        for finding in waiting {
            let task = follow_up(run_id, &round, finding);
            let made = self
                .tasks
                .create_follow_up(&run.user_id, run.task_id, &task)
                .await;
            match made {
                Ok(task_id) => {
                    let key = factory_records::domain::models::StepKey {
                        run_id,
                        seq: finding.seq,
                    };
                    self.repo
                        .link_follow_up(key, finding.place, task_id)
                        .await
                        .map_err(internal)?;
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        seq = finding.seq,
                        place = finding.place,
                        "a follow-up task was not made"
                    );
                }
            }
        }

        let ended = if round.blockers().next().is_some() {
            ReviewRoundOutcome::Changes
        } else {
            ReviewRoundOutcome::Pass
        };
        self.end(run_id, round.seq, ended).await
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
    ) -> Result<Outcome, StepError> {
        let round = self
            .repo
            .end_review_round(run_id, seq, ended)
            .await
            .map_err(internal)?;
        let recorded = round
            .and_then(|round| round.end)
            .map_or(ended, |end| end.outcome);
        outcome(recorded)
    }
}

/// A round's outcome as the Flow routes it.
fn outcome(ended: ReviewRoundOutcome) -> Result<Outcome, StepError> {
    Outcome::try_from(ended.as_str().to_string()).map_err(|error| StepError::Final(internal(error)))
}
