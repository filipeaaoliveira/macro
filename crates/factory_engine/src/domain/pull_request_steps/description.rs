//! What a Run's pull request says of the Run: its report, a marked section
//! of the description between the request and the lines that name the Run
//! and its task. It holds the plan, the Done-when checklist with each
//! clause's acceptance result, the review rounds with how each Blocker
//! ended, the latest round of the repository's checks, and the follow-ups
//! filed.
//!
//! `publish` opens the pull request with it, and `publish` and `ci` write it
//! again on each later visit: only the section is replaced, found by its
//! markers, so whatever else a person wrote in the description stays. A
//! description with no section, as one opened before reports, gets it at
//! its end.
//!
//! A description holds a little less than GitHub's 65,536 characters
//! ([`ROOM_CHARS`]): what does not fit goes into numbered comments on the
//! pull request, each found again by its marker and edited in place. Only
//! comments by the pull request's author, the Run's owner, count: a marker
//! anyone else writes is not the report's. The request is kept whole lines
//! first; the rest of the request, then the rest of the report, go into the
//! comments, never cut mid-line unless one line alone does not fit.
//!
//! What the factory did not write (the request, and what agents wrote: the
//! plan, clauses, findings) is escaped so it mentions no one, links or
//! closes no issue and holds no marker ([`markdown`]).

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::{CheckRoundOutcome, Step};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    DisputeRuling, FindingKind, FixStatus, ReviewFinding, ReviewRound, ReviewRoundOutcome,
    ReviewRoundRepository,
};
use github::domain::models::MacroTaskId;

use super::{MAX_BODY_CHARS, PullRequestStepError, PullRequestSteps};
use crate::domain::acceptance::{Acceptance, AcceptanceCheck, AcceptanceChecks};
use crate::domain::models::{CodeRepository, PullRequest, StepInput};
use crate::domain::ports::{MainWatches, PullRequests};

mod markdown;

/// What opens the report's section in the description.
const REPORT_START: &str = "<!-- macro-factory-report -->";

/// What closes it.
const REPORT_END: &str = "<!-- /macro-factory-report -->";

/// What starts each comment the report overflows into, before its number.
const PART_MARKER: &str = "<!-- macro-factory-report-part ";

/// The most characters the report writes in a description or a comment:
/// GitHub keeps [`MAX_BODY_CHARS`] in either, and this leaves about 1% to
/// spare for how GitHub counts.
const ROOM_CHARS: usize = MAX_BODY_CHARS / 100 * 99;

/// Room a comment keeps for its marker line.
const PART_MARKER_ROOM: usize = 64;

/// What ends the request in the description when the rest of it is in
/// the comments.
const REQUEST_REST: &str = "\n(The rest of the request is in the comments below.)\n";

/// What ends the report in the description when the rest of it is in the
/// comments.
const REPORT_REST: &str = "\n(The rest of this report is in the comments below.)\n";

/// What a comment no longer needed says, so a later write that needs it
/// again finds it.
const PART_UNUSED: &str = "(Nothing more: the whole report fits in the description now.)";

/// The entry whose Step's output holds the plan.
const PLAN_ENTRY: &str = "plan";

/// The entry whose Step verifies the change against the request.
const ACCEPT_ENTRY: &str = "accept";

/// The most characters of the plan the report quotes.
const PLAN_CHARS: usize = 3_000;

/// Where the plan's acceptance checks start in its recorded output.
const PLAN_ACCEPTANCE: &str = "\n\n## Acceptance checks";

/// The most characters of a finding, clause or reason the report quotes.
const LINE_CHARS: usize = 300;

/// `text` on one line, at most [`LINE_CHARS`], quoted.
fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = match line.char_indices().nth(LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    };
    markdown::escape(&line)
}

/// `text` as inline code on one line, at most [`LINE_CHARS`].
fn code(text: &str) -> String {
    let line: String = text
        .chars()
        .map(|c| match c {
            '`' => '\'',
            c if c.is_whitespace() => ' ',
            c => c,
        })
        .collect();
    let line = match line.char_indices().nth(LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    };
    format!("`{}`", markdown::escape(&line))
}

/// The latest Step of `entry` among `earlier` that ended.
fn latest<'a>(earlier: &'a [Step], entry: &str) -> Option<&'a Step> {
    earlier
        .iter()
        .rev()
        .find(|step| step.entry.as_str() == entry && step.end.is_some())
}

/// The plan of the latest `plan` Step, without its acceptance checks, quoted
/// and cut at a line to [`PLAN_CHARS`]; a first line longer than that is
/// cut itself, so some of the plan shows.
fn plan(earlier: &[Step]) -> Option<String> {
    let output = &latest(earlier, PLAN_ENTRY)?.end.as_ref()?.output;
    let plan = output
        .split(PLAN_ACCEPTANCE)
        .next()
        .unwrap_or_default()
        .trim();
    if plan.is_empty() {
        return None;
    }

    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0;
    let mut cut = false;
    for line in plan.lines() {
        chars += line.chars().count() + 1;
        if chars > PLAN_CHARS {
            cut = true;
            if kept.is_empty() {
                let end = line
                    .char_indices()
                    .nth(PLAN_CHARS)
                    .map_or(line.len(), |(end, _)| end);
                kept.push(&line[..end]);
            }
            break;
        }
        kept.push(line);
    }

    let mut text = String::new();
    for line in kept {
        text.push_str(&format!("> {}\n", markdown::escape(line)));
    }
    if cut {
        text.push_str("> …\n\n(The rest of the plan is on the run's plan step.)\n");
    }
    Some(text)
}

/// How the clause `number`, `acceptance`, went in the `accept` Step
/// `accept`: whether it holds, and how that is known.
fn clause_result(number: usize, acceptance: &Acceptance, accept: Option<&Step>) -> (bool, String) {
    let Some(accept) = accept else {
        return (false, "not checked yet".to_string());
    };
    let verdict = accept
        .end
        .as_ref()
        .map(|end| end.outcome.as_str())
        .unwrap_or_default();
    if verdict == "off" {
        return (
            false,
            "not checked: the repository turns acceptance off".to_string(),
        );
    }
    let verifier_passed = verdict == "pass";

    match &acceptance.check {
        AcceptanceCheck::Run { run, .. } => {
            let round = accept.checks.last();
            let name = format!("acceptance-{number}");
            let result =
                round.and_then(|round| round.results.iter().find(|result| result.name == name));
            let passed = match (result, round.and_then(|round| round.end.as_ref())) {
                (Some(result), _) => Some(result.exit_code == 0),
                (None, Some(end)) => Some(end.outcome == CheckRoundOutcome::Passed),
                (None, None) => None,
            };
            match passed {
                Some(true) => (verifier_passed, format!("{} passed", code(run))),
                Some(false) => (false, format!("{} failed", code(run))),
                None => (false, format!("{} has not run", code(run))),
            }
        }
        AcceptanceCheck::None(_) if verifier_passed => {
            (true, "checked by hand by the verifier".to_string())
        }
        AcceptanceCheck::None(_) => (
            false,
            "checked by hand: the verifier did not pass the change".to_string(),
        ),
    }
}

/// The Done-when checklist: each clause of the plan's acceptance checks,
/// with its result in the latest `accept` Step.
fn done_when(earlier: &[Step]) -> Option<String> {
    let accept = latest(earlier, ACCEPT_ENTRY);
    let checks = match AcceptanceChecks::of_run(earlier) {
        Ok(Some(checks)) => checks,
        Ok(None) => return None,
        Err(error) => {
            return Some(format!(
                "The plan's acceptance checks do not read: {error}.\n"
            ));
        }
    };

    let mut text = String::new();
    for (index, acceptance) in checks.clauses.iter().enumerate() {
        let (holds, how) = clause_result(index + 1, acceptance, accept);
        let mark = if holds { "x" } else { " " };
        text.push_str(&format!(
            "- [{mark}] {}: {how}\n",
            one_line(&acceptance.clause)
        ));
    }
    Some(text)
}

/// Where a finding is: its file and lines.
fn place(finding: &ReviewFinding) -> String {
    match finding.lines {
        Some(lines) if lines.start == lines.end => {
            code(&format!("{}:{}", finding.path, lines.start))
        }
        Some(lines) => code(&format!("{}:{}-{}", finding.path, lines.start, lines.end)),
        None => code(&finding.path),
    }
}

/// How a Blocker ended: what the fix said of it, and the ruling on a
/// dispute.
fn fate(blocker: &ReviewFinding) -> &'static str {
    match (blocker.fix_status, blocker.ruling) {
        (Some(FixStatus::Fixed), _) => "fixed",
        (Some(FixStatus::Disputed), Some(DisputeRuling::Dropped)) => {
            "disputed; the reviewer dropped it"
        }
        (Some(FixStatus::Disputed), Some(DisputeRuling::Upheld)) => {
            "disputed; the reviewer upheld it"
        }
        (Some(FixStatus::Disputed), None) => "disputed; not ruled on yet",
        (None, _) => "not fixed yet",
    }
}

/// The review rounds: how each ended, and each Blocker with how it ended.
fn review_rounds(rounds: &[ReviewRound]) -> Option<String> {
    if rounds.is_empty() {
        return None;
    }

    let mut text = String::new();
    for round in rounds {
        let reviewed = if round.fix_diff {
            "the fixes"
        } else {
            "the whole change"
        };
        let blockers: Vec<&ReviewFinding> = round.blockers().collect();
        let ended = match (&round.failure, round.end.as_ref().map(|end| end.outcome)) {
            (Some(failure), _) => format!("could not open: {}", one_line(failure)),
            (None, None) => "reviewing".to_string(),
            (None, Some(ReviewRoundOutcome::Pass)) => "passed".to_string(),
            (None, Some(ReviewRoundOutcome::Failed)) => "a reviewer's work failed".to_string(),
            (None, Some(ReviewRoundOutcome::Changes)) => match blockers.len() {
                1 => "1 Blocker".to_string(),
                count => format!("{count} Blockers"),
            },
        };
        text.push_str(&format!("- Round {}, {reviewed}: {ended}\n", round.round));
        for blocker in blockers {
            text.push_str(&format!(
                "  - {}: {} ({})\n",
                place(blocker),
                one_line(&blocker.problem),
                fate(blocker)
            ));
        }
        let dismissed = round
            .findings
            .iter()
            .filter(|finding| finding.kind == Some(FindingKind::Dismissed))
            .count();
        if dismissed > 0 {
            text.push_str(&format!(
                "  - {dismissed} dismissed: raised again after the reviewer dropped them\n"
            ));
        }
    }
    Some(text)
}

/// The latest round of the repository's checks a Step ran, outside the
/// `accept` Step's acceptance checks: how it went, and which checks ran.
fn checks(earlier: &[Step]) -> Option<String> {
    let (step, round) = earlier
        .iter()
        .rev()
        .filter(|step| step.entry.as_str() != ACCEPT_ENTRY)
        .find_map(|step| {
            let round = step.checks.iter().rev().find(|round| round.end.is_some())?;
            Some((step, round))
        })?;
    let outcome = round.end.as_ref()?.outcome;
    let names: Vec<String> = round.checks.iter().map(|name| code(name)).collect();
    Some(format!(
        "The repository's checks {} in round {} of the {} step: {}.\n",
        outcome.as_str(),
        round.round,
        code(step.entry.as_str()),
        names.join(", ")
    ))
}

/// The follow-ups the review rounds found outside the diff under review,
/// each with whether its task was filed.
fn follow_ups(rounds: &[ReviewRound]) -> Option<String> {
    let mut text = String::new();
    for finding in rounds
        .iter()
        .flat_map(|round| &round.findings)
        .filter(|finding| finding.kind == Some(FindingKind::FollowUp))
    {
        let filed = if finding.follow_up_task_id.is_some() {
            "filed as a task"
        } else {
            "not filed yet"
        };
        text.push_str(&format!(
            "- {}: {} ({filed})\n",
            place(finding),
            one_line(&finding.problem)
        ));
    }
    (!text.is_empty()).then_some(text)
}

/// The report of the Run whose earlier Steps are `earlier` and whose review
/// rounds are `rounds`.
pub(super) fn report(earlier: &[Step], rounds: &[ReviewRound]) -> String {
    let sections = [
        ("Plan", plan(earlier)),
        ("Done when", done_when(earlier)),
        ("Review rounds", review_rounds(rounds)),
        ("Checks", checks(earlier)),
        ("Follow-up tasks", follow_ups(rounds)),
    ];
    let mut text = "## Factory run\n".to_string();
    for (heading, section) in sections {
        if let Some(section) = section {
            text.push_str(&format!("\n### {heading}\n\n{section}"));
        }
    }
    text
}

/// The lines that end the description: the Run, and the task it works on,
/// written so GitHub's webhook links the pull request to the task.
pub(super) fn footer(input: &StepInput<'_>) -> String {
    let mut footer = format!("\n\nOpened by a Macro factory run ({}).", input.key.run_id);
    if let Some(task_id) = input.task_id {
        footer.push_str(&format!(
            "\n\nTask: {}",
            MacroTaskId::from_uuid(&task_id).to_task_id_string()
        ));
    }
    footer
}

/// The whole lines of `text` that fit in `room` characters with `note`
/// after them, then `note`, and the lines after them; all of `text`, and
/// nothing after, when it fits. When not even its first line fits, it is
/// cut where the room ends instead, so a request on one line still shows.
fn fit(text: &str, room: usize, note: &str) -> (String, Option<String>) {
    if text.chars().count() <= room {
        return (text.to_string(), None);
    }

    let note_chars = note.chars().count();
    let mut kept_chars = 0;
    let mut kept_bytes = 0;
    for line in text.split_inclusive('\n') {
        let chars = line.chars().count();
        if kept_chars + chars + note_chars > room {
            break;
        }
        kept_chars += chars;
        kept_bytes += line.len();
    }
    if kept_chars + note_chars > room {
        return (String::new(), Some(text.to_string()));
    }
    if kept_bytes == 0 {
        kept_bytes = text
            .char_indices()
            .nth(room - note_chars)
            .map_or(text.len(), |(end, _)| end);
    }
    let mut kept = text[..kept_bytes].to_string();
    kept.push_str(note);
    (kept, Some(text[kept_bytes..].to_string()))
}

/// How many characters the report's section takes besides the report.
fn frame_chars() -> usize {
    REPORT_START.chars().count() + REPORT_END.chars().count() + 2
}

/// The request as the description holds it, escaped, whole lines, and the
/// rest of it, if any: the request goes first, before the report.
fn request_split(request: &str, footer: &str) -> (String, Option<String>) {
    let room = ROOM_CHARS - footer.chars().count() - frame_chars() - 2;
    fit(&markdown::escape(request), room, REQUEST_REST)
}

/// A description with the Run's report, and the comments it overflows
/// into, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Written {
    /// The description; none when what a person wrote around the section
    /// leaves no room for it.
    pub(super) body: Option<String>,
    /// The comments, each with its marker.
    pub(super) comments: Vec<String>,
}

/// `before`, the report's section holding what fits of `report`, and
/// `after`; what is left of the request, `request_rest`, and of the
/// report goes into the comments.
fn with_report(before: &str, after: &str, report: &str, request_rest: Option<String>) -> Written {
    let room =
        ROOM_CHARS.checked_sub(before.chars().count() + after.chars().count() + frame_chars());
    let Some(room) = room else {
        let rest = [request_rest, Some(report.to_string())];
        return Written {
            body: None,
            comments: parts(rest),
        };
    };
    let (kept, report_rest) = fit(report, room, REPORT_REST);
    Written {
        body: Some(format!(
            "{before}{REPORT_START}\n{kept}\n{REPORT_END}{after}"
        )),
        comments: parts([request_rest, report_rest]),
    }
}

/// The description a pull request is opened with: the request, the
/// report, then the footer. The comments it overflows into are written once
/// the pull request is open ([`PullRequestSteps::refresh_report`]).
pub(super) fn opening(request: &str, footer: &str, report: &str) -> String {
    let (request, request_rest) = request_split(request, footer);
    // The request was cut to leave room for the section and the footer, so
    // the description always has room for them.
    with_report(&format!("{request}\n\n"), footer, report, request_rest)
        .body
        .unwrap_or_default()
}

/// Where the section that the start marker at `start` of `text` opens
/// ends: after the first end marker past it, when no other start marker
/// comes first. None for a start marker left alone.
fn section_end(text: &str, start: usize) -> Option<usize> {
    let after = start + REPORT_START.len();
    let end = text[after..].find(REPORT_END)?;
    let next_start = text[after..].find(REPORT_START);
    if next_start.is_some_and(|next_start| next_start < end) {
        return None;
    }
    Some(after + end + REPORT_END.len())
}

/// `text` without any section of the report and without any marker left
/// alone.
fn without_sections(text: &str) -> String {
    let mut kept = text.to_string();
    while let Some(start) = kept.find(REPORT_START) {
        let end = section_end(&kept, start).unwrap_or(start + REPORT_START.len());
        kept.replace_range(start..end, "");
    }
    kept.replace(REPORT_END, "")
}

/// The description `current` with its report's section replaced by
/// `report`: the first start marker an end marker closes. Any other
/// section, and any marker left alone, as a person's copy and paste leaves,
/// is removed, so the description holds one. One with no section gets it
/// at its end.
pub(super) fn rewritten(current: &str, request: &str, footer: &str, report: &str) -> Written {
    let (_, request_rest) = request_split(request, footer);
    let section = current
        .match_indices(REPORT_START)
        .find_map(|(start, _)| Some((start, section_end(current, start)?)));
    match section {
        Some((start, end)) => with_report(
            &without_sections(&current[..start]),
            &without_sections(&current[end..]),
            report,
            request_rest,
        ),
        None => {
            let current = without_sections(current);
            let kept = current.trim_end();
            let before = if kept.is_empty() {
                String::new()
            } else {
                format!("{kept}\n\n")
            };
            with_report(&before, "", report, request_rest)
        }
    }
}

/// The comments for the rest of the request and of the report: their lines
/// in order, as few comments as hold them, each starting with its marker.
fn parts(rests: [Option<String>; 2]) -> Vec<String> {
    let [request_rest, report_rest] = rests;
    let mut text = String::new();
    if let Some(rest) = request_rest {
        text.push_str(&format!("The rest of the request:\n\n{rest}\n\n"));
    }
    if let Some(rest) = report_rest {
        text.push_str(&format!("The rest of the factory run's report:\n\n{rest}"));
    }
    if text.is_empty() {
        return Vec::new();
    }

    let room = ROOM_CHARS - PART_MARKER_ROOM;
    let mut chunks: Vec<String> = vec![String::new()];
    let mut chunk_chars = 0;
    for line in text.split_inclusive('\n') {
        // A line goes whole into the comment it fits in, else into the
        // next one; only a line longer than a whole comment is cut, filling
        // each comment it spans.
        let mut rest = line;
        while !rest.is_empty() {
            let rest_chars = rest.chars().count();
            let space = room - chunk_chars;
            if rest_chars <= space {
                if let Some(chunk) = chunks.last_mut() {
                    chunk.push_str(rest);
                }
                chunk_chars += rest_chars;
                break;
            }
            if rest_chars <= room || space == 0 {
                chunks.push(String::new());
                chunk_chars = 0;
                continue;
            }
            let end = rest
                .char_indices()
                .nth(space)
                .map_or(rest.len(), |(end, _)| end);
            if let Some(chunk) = chunks.last_mut() {
                chunk.push_str(&rest[..end]);
            }
            rest = &rest[end..];
            chunks.push(String::new());
            chunk_chars = 0;
        }
    }
    if chunks.last().is_some_and(String::is_empty) {
        chunks.pop();
    }
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| format!("{}\n{chunk}", part_marker(index + 1)))
        .collect()
}

/// The marker line of the comment `number`, from 1.
fn part_marker(number: usize) -> String {
    format!("{PART_MARKER}{number} -->")
}

/// The number of the report's comment `body` is, if it is one.
fn part_number(body: &str) -> Option<usize> {
    let (number, _) = body.strip_prefix(PART_MARKER)?.split_once(" -->")?;
    number.parse().ok()
}

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
{
    /// The Run's report, from its earlier Steps and its review rounds.
    pub(super) async fn report(
        &self,
        input: &StepInput<'_>,
    ) -> Result<String, PullRequestStepError> {
        let rounds = self
            .repo
            .review_rounds(input.key.run_id)
            .await
            .map_err(PullRequestStepError::records)?;
        Ok(report(input.earlier, &rounds))
    }

    /// Write the Run's report into its open pull request: its section of
    /// the description, and the comments it overflows into. Reading first,
    /// it writes only what changed, so a retry writes nothing twice. A
    /// failure is logged and dropped: the report is for people, and the
    /// next visit writes it again.
    pub(super) async fn refresh_report(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        pull_request: &PullRequest,
    ) {
        if let Err(error) = self.write_report(input, repository, pull_request).await {
            tracing::warn!(?error, "the pull request's report was not written");
        }
    }

    async fn write_report(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        pull_request: &PullRequest,
    ) -> Result<(), PullRequestStepError> {
        let report = self.report(input).await?;
        let text = self
            .github
            .pull_request_text(input.owner, repository, pull_request.number)
            .await
            .map_err(PullRequestStepError::github)?;
        let current = text.body;
        let written = rewritten(&current, input.request, &footer(input), &report);
        // GitHub replaces a description whole and takes no condition on
        // what it was: a person's edit saved between the read above and
        // this write is lost. The window is one request long, and the
        // section is rewritten on every visit anyway.
        if let Some(body) = written.body.filter(|body| *body != current) {
            self.github
                .edit_pull_request_body(input.owner, repository, pull_request.number, &body)
                .await
                .map_err(PullRequestStepError::github)?;
        }

        // Each comment is found by its marker, among those the pull
        // request's author wrote, as the factory writes them as the Run's
        // owner: a marker anyone else writes is not the report's, and is
        // never edited. One that says something else now is edited, one
        // missing is added, and one no longer needed, or a second of the
        // same number, says it is not needed.
        let existing = self
            .github
            .pull_request_comments(input.owner, repository, pull_request.number)
            .await
            .map_err(PullRequestStepError::github)?;
        let mut found: Vec<(usize, u64, String)> = existing
            .into_iter()
            .filter(|comment| text.author_id.is_some() && comment.author_id == text.author_id)
            .filter_map(|comment| Some((part_number(&comment.body)?, comment.id, comment.body)))
            .collect();
        // Stable: of comments of one number, the oldest is kept.
        found.sort_by_key(|(number, _, _)| *number);
        let mut duplicates = Vec::new();
        let mut index = 1;
        while index < found.len() {
            if found[index].0 == found[index - 1].0 {
                duplicates.push(found.remove(index));
            } else {
                index += 1;
            }
        }

        for (index, part) in written.comments.iter().enumerate() {
            let number = index + 1;
            match found.iter().find(|(found, _, _)| *found == number) {
                Some((_, _, body)) if body == part => {}
                Some((_, id, _)) => {
                    self.github
                        .edit_pull_request_comment(input.owner, repository, *id, part)
                        .await
                        .map_err(PullRequestStepError::github)?;
                }
                None => {
                    self.github
                        .comment_on_pull_request(input.owner, repository, pull_request.number, part)
                        .await
                        .map_err(PullRequestStepError::github)?;
                }
            }
        }
        let unneeded = found
            .iter()
            .filter(|(number, _, _)| *number > written.comments.len());
        for (number, id, body) in unneeded.chain(&duplicates) {
            let unused = format!("{}\n{PART_UNUSED}", part_marker(*number));
            if *body != unused {
                self.github
                    .edit_pull_request_comment(input.owner, repository, *id, &unused)
                    .await
                    .map_err(PullRequestStepError::github)?;
            }
        }
        Ok(())
    }
}
