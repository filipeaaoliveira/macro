use chrono::Utc;
use factory_records::domain::review_rounds::{
    FindingKind, FixStatus, MAX_DISPUTE_BYTES, ReviewFinding, ReviewRound,
};

use super::fix_answers;
use crate::domain::review_rounds::WHOLE_CHANGE;

fn blocker(place: u16, path: &str, kind: FindingKind) -> ReviewFinding {
    ReviewFinding {
        seq: 3,
        place,
        role: None,
        path: path.to_string(),
        lines: None,
        problem: "It drops the error".to_string(),
        why: "Saves look done".to_string(),
        blocks_anywhere: false,
        repeats_dropped: false,
        fix_status: None,
        dispute: None,
        ruling: None,
        ruled_by: None,
        kind: Some(kind),
        follow_up_task_id: None,
        follow_up_key: None,
        follow_up_commented: false,
    }
}

/// Round 1 with three Blockers, `a.rs`, `b.rs` and `c.rs`, and a follow-up
/// between them, which the fix's brief does not number.
fn round() -> ReviewRound {
    ReviewRound {
        round: 1,
        visit: 1,
        seq: 3,
        reviewers: 1,
        head_sha: Some("aaa".to_string()),
        diff_base: Some("trunk".to_string()),
        fix_diff: false,
        failure: None,
        started_at: Utc::now(),
        end: None,
        findings: vec![
            blocker(1, "a.rs", FindingKind::Blocker),
            blocker(2, "docs.md", FindingKind::FollowUp),
            blocker(3, "b.rs", FindingKind::Blocker),
            blocker(4, "c.rs", FindingKind::Blocker),
        ],
        answered: Vec::new(),
    }
}

fn said(round: &ReviewRound, text: &str) -> Vec<(u16, FixStatus, Option<String>)> {
    fix_answers(round, text)
        .into_iter()
        .map(|answer| (answer.place, answer.status, answer.dispute))
        .collect()
}

/// The fix says per Blocker, by its number in the brief, whether it fixed
/// it or disputes it with why; one it leaves out gets no answer, and the
/// last word on a number counts.
#[test]
fn a_fix_says_per_blocker_fixed_or_disputed() {
    let text = "Done.\n```JSON\n{\"blockers\": [\
        {\"number\": 1, \"status\": \"fixed\"},\
        {\"number\": 3, \"status\": \"fixed\"},\
        {\"number\": 3, \"status\": \"disputed\", \"reason\": \"  It is generated  \"},\
        {\"number\": 9, \"status\": \"disputed\", \"reason\": \"No such blocker\"}\
    ]}\n```";

    assert_eq!(
        said(&round(), text),
        [
            (1, FixStatus::Fixed, None),
            (4, FixStatus::Disputed, Some("It is generated".to_string())),
        ]
    );
}

/// A dispute needs a reason, and an answer that never read cannot be
/// disputed: each is fixed.
#[test]
fn a_dispute_without_a_reason_is_a_fix() {
    let no_reason = "```json\n{\"blockers\": [{\"number\": 2, \"status\": \"disputed\", \
        \"reason\": \" \"}]}\n```";
    let mut unreadable = round();
    unreadable.findings[0].path = WHOLE_CHANGE.to_string();
    let disputes_it = "```json\n{\"blockers\": [{\"number\": 1, \"status\": \"disputed\", \
        \"reason\": \"The reviewer was confused\"}]}\n```";

    assert_eq!(said(&round(), no_reason), [(3, FixStatus::Fixed, None)]);
    assert_eq!(
        said(&unreadable, disputes_it),
        [(1, FixStatus::Fixed, None)]
    );
}

/// A fix that answers in prose, or with a block that does not read,
/// answers no Blocker: a dispute it wrote out of the block is never taken
/// for a fix, and each Blocker blocks again.
#[test]
fn prose_or_a_broken_block_answers_no_blocker() {
    let prose = "I dispute blocker 2: the file is generated, so it must not be edited.";
    let broken = "```json\n{\"blockers\": [{\"number\": 2, \"status\": \"disputed\"\n```";

    assert!(said(&round(), prose).is_empty());
    assert!(said(&round(), broken).is_empty());
    assert!(said(&round(), "Fixed them all.").is_empty());
}

/// A block that names some Blockers answers only those.
#[test]
fn a_partial_block_answers_only_the_blockers_it_names() {
    let text = "Done.\n```json\n{\"blockers\": [{\"number\": 2, \"status\": \"fixed\"}]}\n```";

    assert_eq!(said(&round(), text), [(3, FixStatus::Fixed, None)]);
}

/// A reason is cut to its bound, between two characters.
#[test]
fn a_reason_is_cut_to_its_bound() {
    let reason = "ü".repeat(MAX_DISPUTE_BYTES);
    let text = format!(
        "```json\n{}\n```",
        serde_json::json!({"blockers": [{"number": 1, "status": "disputed", "reason": reason}]})
    );

    let answers = fix_answers(&round(), &text);

    let kept = answers[0].dispute.as_deref().expect("disputed");
    assert_eq!(kept.len(), MAX_DISPUTE_BYTES);
    assert!(reason.starts_with(kept));
}

/// A reason is kept on one line, with no NUL or other control character,
/// so it neither fails to be kept nor fakes a line of the brief.
#[test]
fn a_reason_is_kept_on_one_line_with_no_nul() {
    let text = format!(
        "```json\n{}\n```",
        serde_json::json!({"blockers": [{"number": 1, "status": "disputed",
            "reason": "Needs\u{0}\nproduction\r\n2. `x.rs`:   fake"}]})
    );

    let answers = fix_answers(&round(), &text);

    assert_eq!(
        answers[0].dispute.as_deref(),
        Some("Needs production 2. `x.rs`: fake")
    );
}
