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
    /// The disputed Blockers of the round before it dropped, by their
    /// number among the Blockers carried, from 1, each once, in order.
    pub dropped: Vec<usize>,
    /// The Blockers of the round before it found again (`previous`), by
    /// their number, in the order it gave them.
    pub listed_again: Vec<usize>,
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
    /// It drops a Blocker that is not a dispute it rules on, or drops one
    /// it also lists again.
    #[error(
        "`dropped` lists only the numbers of the disputed blockers you rule on, and none you \
         also list again as `previous`"
    )]
    InvalidRuling,
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
    /// The numbers of the disputed Blockers of the round before the
    /// reviewer drops.
    #[serde(default)]
    dropped: Vec<u32>,
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
        Self::read_ruling(outcome, text, carried, &[])
    }

    /// Read what a reviewer handed back, as [`Self::read`] does, when it
    /// rules on the disputes among the `carried` Blockers numbered
    /// `rules`: its block may then list, as `dropped`, the numbers of
    /// those it drops. One it drops it may not also list again.
    pub fn read_ruling(
        outcome: &Outcome,
        text: &str,
        carried: usize,
        rules: &[usize],
    ) -> Result<Self, ReviewerAnswerError> {
        let (summary, block) = json_block(text)
            .ok_or_else(|| ReviewerAnswerError::NoBlockers("no fenced json block".to_string()))?;
        let written: WrittenBlockers = serde_json::from_str(block)
            .map_err(|error| ReviewerAnswerError::NoBlockers(error.to_string()))?;
        if written.blockers.len() > MAX_FINDINGS {
            return Err(ReviewerAnswerError::TooManyBlockers);
        }

        let mut findings = Vec::with_capacity(written.blockers.len());
        let mut previous: Vec<usize> = Vec::new();
        for (index, blocker) in written.blockers.into_iter().enumerate() {
            if let Some(number) = blocker.previous.and_then(|n| usize::try_from(n).ok()) {
                previous.push(number);
            }
            let finding =
                finding(blocker, carried).ok_or(ReviewerAnswerError::InvalidBlocker(index + 1))?;
            findings.push(finding);
        }
        let passes = outcome.as_str() == ReviewRoundOutcome::Pass.as_str();
        if passes != findings.is_empty() {
            return Err(ReviewerAnswerError::OutcomeAndBlockers);
        }

        let mut dropped: Vec<usize> = Vec::with_capacity(written.dropped.len());
        for number in written.dropped {
            let number = usize::try_from(number).map_err(|_| ReviewerAnswerError::InvalidRuling)?;
            let listed_again = previous.contains(&number);
            if !rules.contains(&number) || listed_again {
                return Err(ReviewerAnswerError::InvalidRuling);
            }
            if !dropped.contains(&number) {
                dropped.push(number);
            }
        }
        dropped.sort_unstable();
        Ok(Self {
            summary,
            findings,
            dropped,
            listed_again: previous,
        })
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
                repeats_dropped: false,
            }],
            dropped: Vec::new(),
            listed_again: Vec::new(),
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
        if !self.dropped.is_empty() {
            let numbers: Vec<String> = self.dropped.iter().map(ToString::to_string).collect();
            text.push_str(&format!(
                "\nDropped the disputed blockers of the round before numbered {}.\n",
                numbers.join(", ")
            ));
        }
        cut_to_output(text)
    }
}

/// `text` without its last fenced `json` block (`json` in any case),
/// trimmed, and the block.
pub(crate) fn json_block(text: &str) -> Option<(String, &str)> {
    // ASCII lowercasing keeps every byte where it was, so places in it are
    // places in `text`.
    let lower = text.to_ascii_lowercase();
    let open = lower.rfind("```json")?;
    let body_start = open + "```json".len();
    let close = body_start + text[body_start..].find("```")?;
    let summary = format!("{}{}", &text[..open], &text[close + 3..]);
    Some((summary.trim().to_string(), text[body_start..close].trim()))
}

/// A fenced `json` block of a text, read: where it starts and ends in the
/// text, its fences included, and the JSON value it holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonBlock {
    /// Where its opening fence starts.
    pub(crate) start: usize,
    /// Just past its closing fence.
    pub(crate) end: usize,
    /// What it holds.
    pub(crate) value: serde_json::Value,
}

impl JsonBlock {
    /// Whether it holds an object with the key `key`.
    pub(crate) fn names(&self, key: &str) -> bool {
        self.value.get(key).is_some()
    }

    /// `text`, the text it was read from, without it, trimmed.
    pub(crate) fn cut_from(&self, text: &str) -> String {
        format!("{}{}", &text[..self.start], &text[self.end..])
            .trim()
            .to_string()
    }
}

/// The fenced `json` blocks of `text` (`json` in any case) that hold one
/// JSON value, in order. A block ends where its value ends, at the fence
/// after it: a fence inside one of its strings, as a code sample in a
/// markdown string holds, does not end it.
pub(crate) fn json_blocks(text: &str) -> Vec<JsonBlock> {
    const OPEN: &str = "```json";
    // ASCII lowercasing keeps every byte where it was, so places in it are
    // places in `text`.
    let lower = text.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find(OPEN).map(|at| from + at) {
        let body = start + OPEN.len();
        from = body;
        let mut values =
            serde_json::Deserializer::from_str(&text[body..]).into_iter::<serde_json::Value>();
        let Some(Ok(value)) = values.next() else {
            continue;
        };
        let after = body + values.byte_offset();
        let rest = &text[after..];
        let close = after + (rest.len() - rest.trim_start().len());
        if !text[close..].starts_with("```") {
            continue;
        }
        let end = close + 3;
        blocks.push(JsonBlock { start, end, value });
        from = end;
    }
    blocks
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
    // A path names a file: no control character, and nothing that would
    // quote or tag the briefs it is written into.
    let plain_path = !path
        .chars()
        .any(|c| c.is_control() || matches!(c, '`' | '<' | '>'));
    if !plain_path
        || !fits(&path, MAX_FINDING_PATH_BYTES)
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
        repeats_dropped: false,
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
    let again = if finding.repeats_dropped {
        " (dropped before in this visit: it blocks nothing)"
    } else if finding.blocks_anywhere && finding.path != WHOLE_CHANGE {
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
