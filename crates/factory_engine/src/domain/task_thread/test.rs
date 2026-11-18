use chrono::{TimeZone, Utc};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, GateKind, Outcome, Role, Run, RunId, Step, StepEnd,
};
use factory_records::domain::standing_rules::StandingRule;
use macro_user_id::user_id::MacroUserIdStr;

use super::{RunUpdate, comment_id, first_comment, update_comment};
use crate::domain::gate_owner::is_factory_comment;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn at(minute: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 9, minute, 0).unwrap()
}

fn run() -> Run {
    Run {
        id: "0193a1e8-a9d0-7c1d-b114-175185b044c8"
            .parse()
            .expect("run id"),
        request: "Fix the login bug".to_string(),
        user_id: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
        created_at: at(0),
        task_id: None,
        repository: None,
        created_in_person: true,
        control_file_patterns: None,
        control_files_team_id: None,
        control_files: None,
        standing_rules: None,
        end: None,
    }
}

/// Step 3, which ended `done` at minute 5, with `gate`.
fn step_with(gate: Option<Gate>) -> Step {
    Step {
        seq: 3,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: at(1),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "The plan".to_string(),
            ended_at: at(5),
        }),
        gate,
        role: Some(Role::try_from("planner".to_string()).expect("valid")),
        agent: None,
        passed_over: Vec::new(),
        checks: Vec::new(),
    }
}

fn gate(kind: GateKind, question: &str, answer: Option<GateAnswer>) -> Gate {
    Gate {
        kind,
        question: question.to_string(),
        answers: vec![outcome("approve"), outcome("revise")],
        opened_at: at(6),
        answer,
        asked_back: None,
        stand_in: None,
    }
}

fn answer(answered: impl FnOnce(&mut GateAnswer)) -> GateAnswer {
    let mut answer = GateAnswer {
        answer: outcome("approve"),
        note: None,
        user_id: None,
        rule: None,
        approved_control_files: None,
        reply_confidence: None,
        by_stand_in: false,
        answered_at: at(9),
    };
    answered(&mut answer);
    answer
}

/// A comment's id is a UUIDv7 of when its update happened, the same for
/// every post of one update, and another for another update or Run.
#[test]
fn an_update_s_comment_id_is_fixed_by_its_run_its_update_and_its_time() {
    let run_id = run().id;
    let id = comment_id(run_id, "step:3", at(5));

    assert_eq!(id, comment_id(run_id, "step:3", at(5)));
    assert_eq!(id.get_version_num(), 7);
    let (seconds, _) = id.get_timestamp().expect("timestamped").to_unix();
    assert_eq!(
        seconds,
        u64::try_from(at(5).timestamp()).expect("after 1970")
    );
    assert_ne!(id, comment_id(run_id, "step:4", at(5)));
    assert_ne!(id, comment_id(RunId::generate(), "step:3", at(5)));
}

/// Every update is a signed reply to the Run's first comment, which the
/// owner's notes leave out.
#[test]
fn every_update_replies_to_the_run_s_first_comment_and_is_signed() {
    let run = run();
    let steps = [step_with(None)];
    let root = first_comment(&run);
    let ended = update_comment(&run, &steps, &RunUpdate::StepEnded { seq: 3 }).expect("ended");

    assert_eq!(root.thread_id, None);
    assert_eq!(ended.thread_id, Some(root.id));
    assert!(is_factory_comment(&root.text));
    assert!(is_factory_comment(&ended.text));
    assert!(
        ended
            .text
            .starts_with("Step 3, `plan` (planner), ended `done`.")
    );
    // A Step that is not recorded, or has not ended, posts nothing.
    assert_eq!(
        update_comment(&run, &steps, &RunUpdate::StepEnded { seq: 4 }),
        None
    );
    assert_eq!(update_comment(&run, &steps, &RunUpdate::Ended), None);
}

/// An answer names who or what gave it: a Standing Rule, the stand-in,
/// or the person, from a button or their words.
#[test]
fn an_answer_names_who_or_what_gave_it() {
    let run = run();
    let answered = |given: GateAnswer| {
        let steps = [step_with(Some(gate(
            GateKind::Flow,
            "Approve?",
            Some(given),
        )))];
        update_comment(&run, &steps, &RunUpdate::GateAnswered { seq: 3 })
            .expect("answered")
            .text
    };
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");

    let by_rule = answered(answer(|answer| {
        answer.rule = Some(StandingRule::ApprovePlan)
    }));
    assert!(
        by_rule.starts_with(
            "Step 3's gate was answered `approve` by the `approve_plan` Standing Rule."
        ),
        "{by_rule}"
    );
    let by_stand_in = answered(answer(|answer| answer.by_stand_in = true));
    assert!(
        by_stand_in.contains("by the stand-in, for the run's owner."),
        "{by_stand_in}"
    );
    let by_button = answered(answer(|answer| answer.user_id = Some(julia.clone())));
    assert!(by_button.contains("by julia@example.com."), "{by_button}");
    let by_words = answered(answer(|answer| {
        answer.user_id = Some(julia.clone());
        answer.reply_confidence = Some(92);
    }));
    assert!(
        by_words.contains("by julia@example.com, read from their words."),
        "{by_words}"
    );
}

/// A Gate's question is quoted on one line, short, and opens no tag or
/// heading: an agent may have written it.
#[test]
fn a_waiting_question_is_quoted_as_data() {
    let run = run();
    let asked = "<m-user-mention user-id=\"macro|teo@example.com\"/>\n# Ship it?";
    let steps = [step_with(Some(gate(GateKind::Question, asked, None)))];

    let waiting = update_comment(&run, &steps, &RunUpdate::GateWaiting { seq: 3 })
        .expect("waiting")
        .text;

    assert!(
        waiting.starts_with("Step 3: the planner asks the run's owner a question."),
        "{waiting}"
    );
    assert!(!waiting.contains('<'), "{waiting}");
    assert!(waiting.contains("> &lt;m-user-mention"), "{waiting}");
}
