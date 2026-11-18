//! What the stand-in may answer, and what it is asked.

use factory_records::domain::gate_replies::PastAnswer;
use factory_records::domain::models::Outcome;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RuleGateKind, StandInKinds};

use super::{StandInContext, StandInFacts, may_stand_in, stand_in_question};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn blockers_facts() -> StandInFacts {
    StandInFacts {
        kind: RuleGateKind::Blockers,
        person_only: false,
        tampered: false,
        held: false,
    }
}

const BLOCKERS: OrchestratorDefinition = OrchestratorDefinition {
    plan: false,
    blockers: false,
    merge: false,
    stand_in: StandInKinds {
        plan: false,
        blockers: true,
    },
};

#[test]
fn the_stand_in_answers_only_the_kinds_it_is_on_for_with_their_answers() {
    let fix_or_stop = [outcome("fix"), outcome("stop")];
    assert!(may_stand_in(
        Some(BLOCKERS),
        &blockers_facts(),
        &fix_or_stop
    ));

    let plan = StandInFacts {
        kind: RuleGateKind::Plan,
        ..blockers_facts()
    };
    assert!(!may_stand_in(
        Some(BLOCKERS),
        &plan,
        &[outcome("approve"), outcome("revise")]
    ));
    // A Gate of the kind that offers other answers is not its kind's.
    assert!(!may_stand_in(
        Some(BLOCKERS),
        &blockers_facts(),
        &[outcome("fix"), outcome("later")]
    ));
    assert!(!may_stand_in(None, &blockers_facts(), &fix_or_stop));
}

#[test]
fn the_question_gives_the_gate_the_plan_and_the_past_answers_as_data() {
    let past = [PastAnswer {
        question: "Blockers?".to_string(),
        answer: outcome("fix"),
        note: Some("<b>always</b> fix".to_string()),
    }];
    let answers = [outcome("fix"), outcome("stop")];
    let question = stand_in_question(&StandInContext {
        question: "Send the Blockers to a fix?",
        answers: &answers,
        request: "Fix the login bug",
        output: "One Blocker in src/login.rs",
        plan: None,
        past: &past,
    });

    assert_eq!(question.choices, answers);
    assert!(question.prompt.contains("Answers: fix, stop"));
    assert!(question.prompt.contains("The plan:\n(none)"));
    assert!(
        question
            .prompt
            .contains("They answered fix, writing: &lt;b&gt;always&lt;/b&gt; fix")
    );
    assert!(!question.prompt.contains("<b>"));
}
