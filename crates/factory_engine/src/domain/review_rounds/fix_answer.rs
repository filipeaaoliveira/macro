//! What a fix hands back of the Blockers it was given: its text ends with
//! a fenced `json` block that says, per Blocker by its number in the fix's
//! brief, whether it fixed it or disputes it, with why.

#[cfg(test)]
mod test;

use factory_records::domain::review_rounds::{
    FixAnswer, FixStatus, MAX_DISPUTE_BYTES, ReviewFinding, ReviewRound,
};

use super::answer::{WHOLE_CHANGE, cut, json_block};
use super::brief::one_line;

/// What the fix handed back, `text`, says of each Blocker of `round` its
/// block names, numbered from 1 as its brief numbers them. A Blocker it
/// disputes with a reason is disputed, the reason on one line (no NUL or
/// other control character, white space collapsed) and cut to
/// [`MAX_DISPUTE_BYTES`]; any other it names is fixed, as one it disputes
/// with no reason. A Blocker the block leaves out, and every one when the
/// block does not read, gets no answer: it stays unanswered and blocks
/// again. An answer that never read names nothing to rule on, so it is
/// never disputed. When the block names a Blocker more than once, the
/// last counts.
pub fn fix_answers(round: &ReviewRound, text: &str) -> Vec<FixAnswer> {
    let said = said_of(text);
    round
        .blockers()
        .enumerate()
        .filter_map(|(index, blocker)| {
            let (_, reason) = said.iter().rev().find(|(number, _)| *number == index + 1)?;
            let reason = reason.as_deref().filter(|_| disputable(blocker));
            Some(match reason {
                Some(reason) => FixAnswer {
                    seq: blocker.seq,
                    place: blocker.place,
                    status: FixStatus::Disputed,
                    dispute: Some(cut(reason, MAX_DISPUTE_BYTES).to_string()),
                },
                None => FixAnswer {
                    seq: blocker.seq,
                    place: blocker.place,
                    status: FixStatus::Fixed,
                    dispute: None,
                },
            })
        })
        .collect()
}

/// Whether the fix may dispute `blocker`: one on a place in the change,
/// not an answer that never read.
fn disputable(blocker: &ReviewFinding) -> bool {
    blocker.path != WHOLE_CHANGE
}

/// The Blockers `text` names, by number, each with its reason when it is
/// disputed with one: none when its block does not read. An entry that
/// does not read is left out.
fn said_of(text: &str) -> Vec<(usize, Option<String>)> {
    let Some((_, block)) = json_block(text) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(block) else {
        return Vec::new();
    };
    let Some(entries) = value.get("blockers").and_then(|list| list.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let number = entry
                .get("number")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())?;
            let disputed =
                entry.get("status").and_then(serde_json::Value::as_str) == Some("disputed");
            let reason = entry
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(one_line)
                .filter(|reason| disputed && !reason.is_empty());
            Some((number, reason))
        })
        .collect()
}
