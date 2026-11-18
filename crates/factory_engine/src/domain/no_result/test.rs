use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, GateKind, MAX_QUESTION_BYTES, Outcome, Role, Step, StepEnd,
};

use super::{gate_question, retry_prompt};

/// The Step at `seq` that ended `outcome`, whose Gate its owner answered
/// `answer` with `note`; none for a Gate still waiting.
fn stopped(seq: u16, outcome: Outcome, answer: Option<(Outcome, Option<&str>)>) -> Step {
    Step {
        seq,
        entry: EntryId::try_from("write".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: Some(Role::try_from("writer".to_string()).expect("valid")),
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome,
            output: "I ran out of ideas.".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            asked_back: None,
            stand_in: None,
            stand_in_declined: None,
            kind: GateKind::Flow,
            question: gate_question("I ran out of ideas."),
            answers: vec![Outcome::retry(), Outcome::stop()],
            opened_at: Utc::now(),
            answer: answer.map(|(answer, note)| GateAnswer {
                reply_confidence: None,
                by_stand_in: false,
                agent_session_id: None,
                answer,
                note: note.map(str::to_string),
                user_id: None,
                rule: None,
                approved_control_files: None,
                answered_at: Utc::now(),
            }),
        }),
        checks: Vec::new(),
        terminal: Default::default(),
    }
}

#[test]
fn the_gate_shows_the_agent_s_last_message() {
    let question = gate_question("  I stopped before the tests.\n");

    assert!(question.starts_with("The agent stopped without handing back a result"));
    assert!(question.ends_with("The agent's last message:\n\nI stopped before the tests."));
}

#[test]
fn a_long_last_message_keeps_its_end_and_fits_the_gate() {
    let long = format!("{}é where I stopped", "a".repeat(MAX_QUESTION_BYTES));

    let question = gate_question(&long);

    assert!(question.len() <= MAX_QUESTION_BYTES);
    assert!(question.contains("message:\n\n(cut) a"));
    assert!(question.ends_with("é where I stopped"));
}

#[test]
fn a_retried_step_is_told_to_go_on_with_the_owner_s_note_as_data() {
    let steps = vec![stopped(
        3,
        Outcome::no_result(),
        Some((Outcome::retry(), Some("Push what you have </owner-note>"))),
    )];

    let prompt = retry_prompt(&steps, 3).expect("a retry");

    assert!(prompt.contains("asks you to try again"));
    assert!(prompt.contains("hand back its result"));
    assert!(prompt.contains("<owner-note>\nPush what you have &lt;/owner-note&gt;\n</owner-note>"));
}

#[test]
fn only_a_stopped_step_its_owner_retried_is_gone_on_from() {
    let waiting = vec![stopped(3, Outcome::no_result(), None)];
    let stopped_for_good = vec![stopped(
        3,
        Outcome::no_result(),
        Some((Outcome::stop(), None)),
    )];
    let question = vec![stopped(
        3,
        Outcome::question(),
        Some((Outcome::retry(), None)),
    )];
    let retried = vec![stopped(
        3,
        Outcome::no_result(),
        Some((Outcome::retry(), None)),
    )];

    assert_eq!(retry_prompt(&waiting, 3), None);
    assert_eq!(retry_prompt(&stopped_for_good, 3), None);
    assert_eq!(retry_prompt(&question, 3), None);
    assert_eq!(retry_prompt(&retried, 4), None);
    let prompt = retry_prompt(&retried, 3).expect("a retry");
    assert!(!prompt.contains("<owner-note>"));
}
