//! What a reviewer hands back: its text, ending with a fenced `json` block
//! of the Blockers it found.

use factory_records::domain::models::{MAX_OUTPUT_BYTES, Outcome};
use factory_records::domain::review_rounds::{
    Finding, Lines, MAX_FINDING_PATH_BYTES, MAX_FINDING_TEXT_BYTES, MAX_FINDINGS,
    ReviewRoundOutcome,
};
use serde::Deserialize;

#[cfg(test)]
mod test;

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
    #[error("the text has no json block listing the blockers: {0}")]
    NoBlockers(String),
    /// A Blocker has no file, problem or why, one too long, lines that do
    /// not read, or names a Blocker of the round before it did not have.
    #[error(
        "blocker {0} needs a file, a problem and why, each short enough, lines from 1, and a \
         `previous` among the blockers listed"
    )]
    InvalidBlocker(usize),
    /// It lists more Blockers than a round keeps.
    #[error("a reviewer lists at most {MAX_FINDINGS} blockers")]
    TooManyBlockers,
    /// It passes the change and lists Blockers, or asks for changes and
    /// lists none.
    #[error("an answer that passes lists no blocker, and one that asks for changes lists some")]
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
    /// The number of the Blocker of the round before this one is, when it
    /// was found again.
    #[serde(default)]
    previous: Option<u32>,
}

/// The block a reviewer ends its text with.
#[derive(Deserialize)]
struct WrittenBlockers {
    blockers: Vec<WrittenBlocker>,
}

/// The path of a finding about the whole change rather than one file.
pub const WHOLE_CHANGE: &str = ".";

impl ReviewerAnswer {
    /// Read what a reviewer handed back with `outcome`: `text` holds a
    /// fenced `json` block, `{"blockers": [...]}`, which `pass` leaves
    /// empty and `changes` does not; the last such block counts, whatever
    /// the fence's case and whatever text follows it. A Blocker may name
    /// the one of the `carried` Blockers of the round before it is
    /// (`previous`, from 1): it then blocks wherever it is.
    pub fn read(
        outcome: &Outcome,
        text: &str,
        carried: usize,
    ) -> Result<Self, ReviewerAnswerError> {
        let (summary, block) = json_block(text)
            .ok_or_else(|| ReviewerAnswerError::NoBlockers("no fenced json block".to_string()))?;
        let written: WrittenBlockers = serde_json::from_str(block)
            .map_err(|error| ReviewerAnswerError::NoBlockers(error.to_string()))?;
        if written.blockers.len() > MAX_FINDINGS {
            return Err(ReviewerAnswerError::TooManyBlockers);
        }

        let mut findings = Vec::with_capacity(written.blockers.len());
        for (index, blocker) in written.blockers.into_iter().enumerate() {
            let finding =
                finding(blocker, carried).ok_or(ReviewerAnswerError::InvalidBlocker(index + 1))?;
            findings.push(finding);
        }
        let passes = outcome.as_str() == ReviewRoundOutcome::Pass.as_str();
        if passes != findings.is_empty() {
            return Err(ReviewerAnswerError::OutcomeAndBlockers);
        }
        Ok(Self { summary, findings })
    }

    /// The answer of a reviewer whose hand back did not read, even when it
    /// was asked again: one Blocker on the whole change, so the round
    /// cannot pass on an answer nobody could read.
    pub fn unreadable(text: &str, error: &ReviewerAnswerError) -> Self {
        let why = format!("The reviewer's answer did not read, twice: {error}.");
        Self {
            summary: cut(text.trim(), MAX_OUTPUT_BYTES / 2).to_string(),
            findings: vec![Finding {
                path: WHOLE_CHANGE.to_string(),
                lines: None,
                problem: "The reviewer could not hand back a review that reads.".to_string(),
                why: cut(&why, MAX_FINDING_TEXT_BYTES).to_string(),
                blocks_anywhere: true,
            }],
        }
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

/// `text` without its last fenced `json` block (`json` in any case),
/// trimmed, and the block.
fn json_block(text: &str) -> Option<(String, &str)> {
    // ASCII lowercasing keeps every byte where it was, so places in it are
    // places in `text`.
    let lower = text.to_ascii_lowercase();
    let open = lower.rfind("```json")?;
    let body_start = open + "```json".len();
    let close = body_start + text[body_start..].find("```")?;
    let summary = format!("{}{}", &text[..open], &text[close + 3..]);
    Some((summary.trim().to_string(), text[body_start..close].trim()))
}

/// A written Blocker as a finding, if it holds together: a file, a problem
/// and why, each within its bound, lines from 1, the last not before the
/// first, and a `previous` among the `carried` Blockers. A first line alone
/// is that one line.
fn finding(blocker: WrittenBlocker, carried: usize) -> Option<Finding> {
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
    let blocks_anywhere = match blocker.previous {
        None => false,
        Some(previous) => {
            let listed = usize::try_from(previous).is_ok_and(|n| n >= 1 && n <= carried);
            if !listed {
                return None;
            }
            true
        }
    };
    Some(Finding {
        path,
        lines,
        problem,
        why,
        blocks_anywhere,
    })
}

/// Where a finding is: its file, and its lines when it names some.
pub(super) fn place(path: &str, lines: Option<Lines>) -> String {
    if path == WHOLE_CHANGE {
        return "the whole change".to_string();
    }
    match lines {
        Some(Lines { start, end }) if start == end => format!("`{path}` line {start}"),
        Some(Lines { start, end }) => format!("`{path}` lines {start}-{end}"),
        None => format!("`{path}`"),
    }
}

/// One finding, for a person or an agent to read.
fn describe(finding: &Finding) -> String {
    let again = if finding.blocks_anywhere {
        " (still there from the round before)"
    } else {
        ""
    };
    format!(
        "{}: {}{again}\n   Why: {}",
        place(&finding.path, finding.lines),
        finding.problem,
        finding.why
    )
}

/// What ends a Step's output cut to fit.
const CUT_NOTE: &str = "\n\n(The rest is left out: the review round lists every blocker.)\n";

/// `text`, cut to what a Step's output keeps.
fn cut_to_output(text: String) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    format!(
        "{}{CUT_NOTE}",
        cut(&text, MAX_OUTPUT_BYTES - CUT_NOTE.len())
    )
}

/// The longest start of `text` at most `max` bytes long, cut between two
/// characters.
pub(super) fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
