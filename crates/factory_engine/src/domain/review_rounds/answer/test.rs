use factory_records::domain::models::Outcome;
use factory_records::domain::review_rounds::Lines;

use super::{ReviewerAnswer, ReviewerAnswerError, WHOLE_CHANGE};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

#[test]
fn a_reviewer_hands_back_its_blockers_in_a_json_block() {
    let text = "Checked the save path.\n\n```json\n{\"blockers\": [\
        {\"file\": \"./src/save.rs\", \"startLine\": 12, \"problem\": \"Drops the error\", \"why\": \"Saves look done\"},\
        {\"file\": \"README.md\", \"problem\": \"Out of date\", \"why\": \"It names the old flag\"}\
    ]}\n```\n";

    let answer = ReviewerAnswer::read(&outcome("changes"), text, 0).expect("reads");

    assert_eq!(answer.summary, "Checked the save path.");
    assert_eq!(answer.findings[0].path, "src/save.rs");
    assert_eq!(answer.findings[0].lines, Some(Lines { start: 12, end: 12 }));
    assert_eq!(answer.findings[1].lines, None);
    let rendered = answer.render();
    assert!(rendered.starts_with("Checked the save path.\n\n### Blockers\n"));
    assert!(
        rendered.contains("1. `src/save.rs` line 12: Drops the error\n   Why: Saves look done")
    );
}

#[test]
fn a_hand_back_whose_blockers_do_not_read_or_fit_its_outcome_is_refused() {
    let none = "```json\n{\"blockers\": []}\n```";
    let one =
        "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"problem\": \"p\", \"why\": \"w\"}]}\n```";
    let backwards = "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"startLine\": 9, \"endLine\": 3, \"problem\": \"p\", \"why\": \"w\"}]}\n```";
    let no_why =
        "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"problem\": \"p\", \"why\": \" \"}]}\n```";

    assert!(ReviewerAnswer::read(&outcome("pass"), none, 0).is_ok());
    assert_eq!(
        ReviewerAnswer::read(&outcome("pass"), one, 0),
        Err(ReviewerAnswerError::OutcomeAndBlockers)
    );
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), none, 0),
        Err(ReviewerAnswerError::OutcomeAndBlockers)
    );
    assert!(matches!(
        ReviewerAnswer::read(&outcome("changes"), "Looks wrong to me.", 0),
        Err(ReviewerAnswerError::NoBlockers(_))
    ));
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), backwards, 0),
        Err(ReviewerAnswerError::InvalidBlocker(1))
    );
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), no_why, 0),
        Err(ReviewerAnswerError::InvalidBlocker(1))
    );
}

/// A fence in another case, a block followed by more text, and an earlier
/// block all read: the last block counts.
#[test]
fn a_hand_back_is_read_whatever_its_fence_s_case_and_what_follows() {
    let text = "First try:\n```json\n{\"blockers\": []}\n```\nThen:\n```JSON\n{\"blockers\": [\
        {\"file\": \"a.rs\", \"problem\": \"p\", \"why\": \"w\"}]}\n```\nThat is all.";

    let answer = ReviewerAnswer::read(&outcome("changes"), text, 0).expect("reads");

    assert_eq!(answer.findings.len(), 1);
    assert!(answer.summary.ends_with("That is all."));
}

/// A Blocker that names one of the round before blocks wherever it is; a
/// number the round before did not have does not read.
#[test]
fn a_blocker_found_again_names_its_number_in_the_round_before() {
    let again = "```json\n{\"blockers\": [{\"file\": \"b.rs\", \"problem\": \"p\", \
        \"why\": \"w\", \"previous\": 2}]}\n```";

    let answer = ReviewerAnswer::read(&outcome("changes"), again, 2).expect("reads");
    let unknown = ReviewerAnswer::read(&outcome("changes"), again, 1);

    assert!(answer.findings[0].blocks_anywhere);
    assert_eq!(unknown, Err(ReviewerAnswerError::InvalidBlocker(1)));
}

/// An answer that never read is one Blocker on the whole change, which
/// blocks wherever it is.
#[test]
fn an_unreadable_answer_is_one_blocker_on_the_whole_change() {
    let answer = ReviewerAnswer::unreadable(
        "Looks fine to me.",
        &ReviewerAnswerError::NoBlockers("no fenced json block".to_string()),
    );

    assert_eq!(answer.findings.len(), 1);
    assert_eq!(answer.findings[0].path, WHOLE_CHANGE);
    assert!(answer.findings[0].blocks_anywhere);
    assert!(answer.render().contains("the whole change"));
}
