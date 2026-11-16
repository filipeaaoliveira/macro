use factory_records::domain::models::Outcome;
use macro_user_id::user_id::MacroUserIdStr;

use super::{AnswerGate, GateRefusal, OpenGate, check_answer};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn plan_gate() -> OpenGate {
    OpenGate {
        seq: 2,
        answers: vec![outcome("approve"), outcome("revise")],
    }
}

fn answer(seq: u16, answer: &str) -> AnswerGate {
    AnswerGate {
        seq,
        answer: outcome(answer),
        note: None,
        user_id: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
    }
}

#[test]
fn an_offered_answer_to_the_open_gate_is_taken() {
    let taken = check_answer(Some(&plan_gate()), false, &answer(2, "revise"));

    assert_eq!(taken, Ok(()));
}

#[test]
fn an_answer_with_no_gate_open_is_refused() {
    let refused = check_answer(None, false, &answer(2, "approve"));

    assert!(matches!(refused, Err(GateRefusal::NotWaiting { .. })));
}

#[test]
fn an_answer_to_an_earlier_step_is_refused() {
    let refused = check_answer(Some(&plan_gate()), false, &answer(1, "approve"));

    assert!(matches!(refused, Err(GateRefusal::NotWaiting { .. })));
}

#[test]
fn a_second_answer_is_refused() {
    let refused = check_answer(Some(&plan_gate()), true, &answer(2, "approve"));

    assert_eq!(refused, Err(GateRefusal::AlreadyAnswered));
}

#[test]
fn an_answer_the_gate_does_not_offer_is_refused() {
    let refused = check_answer(Some(&plan_gate()), false, &answer(2, "merge"));

    assert_eq!(refused, Err(GateRefusal::UnknownAnswer(outcome("merge"))));
}
