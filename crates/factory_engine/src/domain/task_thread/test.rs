use chrono::{TimeZone, Utc};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, GateKind, Outcome, Role, Run, RunId, Step, StepEnd,
};
use factory_records::domain::standing_rules::StandingRule;
use macro_user_id::user_id::MacroUserIdStr;

use super::comments::without_links;
use super::{RunUpdate, UpdateContext, comment_id, first_comment, update_comment};
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
        automation: None,
        definition: None,
        end: None,
    }
}

const LINK: &str = "https://macro.test/app/factory/runs/0193a1e8-a9d0-7c1d-b114-175185b044c8";

/// The context of a Run with no review round and no named answerer.
fn context() -> UpdateContext<'static> {
    UpdateContext {
        run_link: LINK,
        rounds: &[],
        answerer_name: None,
        flow: None,
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
        terminal: Default::default(),
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
        agent_session_id: None,
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
    let root = first_comment(&run, LINK);
    let ended =
        update_comment(&run, &steps, &context(), &RunUpdate::StepEnded { seq: 3 }).expect("ended");

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
        update_comment(&run, &steps, &context(), &RunUpdate::StepEnded { seq: 4 }),
        None
    );
    assert_eq!(
        update_comment(&run, &steps, &context(), &RunUpdate::Ended),
        None
    );
}

/// An answer names who or what gave it: a Standing Rule, the stand-in,
/// the person by the name they show others, from a button or their
/// words, or the Orchestrator for them; a person with no name set is the
/// run's owner.
#[test]
fn an_answer_names_who_or_what_gave_it() {
    let run = run();
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");
    let answered = |given: GateAnswer, name: Option<&str>| {
        let steps = [step_with(Some(gate(
            GateKind::Flow,
            "Approve?",
            Some(given),
        )))];
        let context = UpdateContext {
            answerer_name: name,
            ..context()
        };
        update_comment(&run, &steps, &context, &RunUpdate::GateAnswered { seq: 3 })
            .expect("answered")
            .text
    };

    let by_rule = answered(
        answer(|answer| answer.rule = Some(StandingRule::ApprovePlan)),
        None,
    );
    assert!(
        by_rule.starts_with(
            "Step 3's gate was answered `approve` by the `approve_plan` Standing Rule."
        ),
        "{by_rule}"
    );
    let by_stand_in = answered(answer(|answer| answer.by_stand_in = true), None);
    assert!(
        by_stand_in.contains("by the stand-in, for the run's owner."),
        "{by_stand_in}"
    );
    let by_button = answered(
        answer(|answer| answer.user_id = Some(julia.clone())),
        Some("Julia Ruiz"),
    );
    assert!(by_button.contains("by Julia Ruiz."), "{by_button}");
    assert!(!by_button.contains("julia@example.com"), "{by_button}");
    let by_words = answered(
        answer(|answer| {
            answer.user_id = Some(julia.clone());
            answer.reply_confidence = Some(92);
        }),
        Some("Julia Ruiz"),
    );
    assert!(
        by_words.contains("by Julia Ruiz, read from their words."),
        "{by_words}"
    );
    let unnamed = answered(answer(|answer| answer.user_id = Some(julia.clone())), None);
    assert!(unnamed.contains("by the run's owner."), "{unnamed}");
    let by_orchestrator = answered(
        answer(|answer| {
            answer.user_id = Some(julia.clone());
            answer.agent_session_id = Some(uuid::Uuid::from_u128(0xa6e));
        }),
        Some("Julia Ruiz"),
    );
    assert!(
        by_orchestrator.contains("by the Orchestrator, for Julia Ruiz."),
        "{by_orchestrator}"
    );
}

/// An agent's question is not quoted: the thread says only which Role
/// asks.
#[test]
fn an_agent_s_question_is_told_without_its_words() {
    let run = run();
    let asked = "Should I drop the users table? See https://evil.example/x";
    let steps = [step_with(Some(gate(GateKind::Question, asked, None)))];

    let waiting = update_comment(&run, &steps, &context(), &RunUpdate::GateWaiting { seq: 3 })
        .expect("waiting")
        .text;

    assert!(
        waiting.starts_with("Step 3's planner asks the run's owner a question."),
        "{waiting}"
    );
    assert!(!waiting.contains("users table"), "{waiting}");
    assert!(!waiting.contains("evil.example"), "{waiting}");
}

/// The last message of an agent that stopped without a result is not
/// quoted either: the thread says only which Role stopped, and the answers
/// its owner may give.
#[test]
fn an_agent_that_stopped_without_a_result_is_told_without_its_words() {
    let run = run();
    let said = "I give up. See https://evil.example/x";
    let mut stopped = step_with(Some(gate(GateKind::Flow, said, None)));
    if let Some(end) = stopped.end.as_mut() {
        end.outcome = Outcome::no_result();
        end.output = said.to_string();
    }

    let waiting = update_comment(
        &run,
        &[stopped],
        &context(),
        &RunUpdate::GateWaiting { seq: 3 },
    )
    .expect("waiting")
    .text;

    assert!(
        waiting.starts_with(
            "Step 3's planner stopped without handing back a result, and waits for the run's \
             owner to answer `retry` or `stop`."
        ),
        "{waiting}"
    );
    assert!(!waiting.contains("give up"), "{waiting}");
}

/// A Flow's question is quoted on one line, short, opening no tag or
/// heading, and with no link, image or web address.
#[test]
fn a_flow_s_question_is_quoted_without_links() {
    let run = run();
    let asked = "<b>Ship</b> it? ![shot](https://img.example/a.png) [the plan](https://x.example/p)\n# then https://y.example/q done";
    let steps = [step_with(Some(gate(GateKind::Flow, asked, None)))];

    let waiting = update_comment(&run, &steps, &context(), &RunUpdate::GateWaiting { seq: 3 })
        .expect("waiting")
        .text;

    assert!(!waiting.contains('<'), "{waiting}");
    assert!(!waiting.contains("example"), "{waiting}");
    assert!(
        waiting.contains("> &lt;b&gt;Ship&lt;/b&gt; it? shot the plan # then (link left out) done"),
        "{waiting}"
    );
    assert_eq!(
        without_links("See [docs](https://a.example) and HTTPS://B.example/x."),
        "See docs and (link left out)"
    );
}

/// The first comment links the Run, and names neither its repository nor
/// its id otherwise.
#[test]
fn the_first_comment_links_the_run() {
    let mut run = run();
    run.repository = Some(factory_records::domain::models::Repository {
        url: "https://github.com/acme/secret".to_string(),
        base_branch: "main".to_string(),
    });

    let first = first_comment(&run, LINK);

    assert!(
        first.text.contains(&format!("[Open the run]({LINK})")),
        "{}",
        first.text
    );
    assert!(!first.text.contains("acme/secret"), "{}", first.text);
    assert_eq!(
        first.text.matches(&run.id.to_string()).count(),
        1,
        "{}",
        first.text
    );
}

/// A paused Step whose time runs out is told once per pause, with when it
/// ends; a Step not paused, or that ended, tells nothing.
#[test]
fn a_paused_step_running_out_of_time_is_told_once_per_pause() {
    let run = run();
    let mut step = step_with(None);
    step.end = None;
    let ends_by = at(50);
    let update = RunUpdate::PauseRunningOut { seq: 3, ends_by };
    assert!(update_comment(&run, &[step.clone()], &context(), &update).is_none());

    step.terminal.paused_at = Some(at(10));
    let told = update_comment(&run, &[step.clone()], &context(), &update).expect("told");
    assert!(told.text.contains("Step 3 is paused"));
    assert!(
        told.text
            .contains(&ends_by.format("%Y-%m-%d %H:%M").to_string())
    );
    assert_eq!(told.id, comment_id(run.id, "pause:3", at(10)));
    assert!(is_factory_comment(&told.text));

    step.end = Some(StepEnd {
        outcome: outcome("failed"),
        output: "Stopped".to_string(),
        ended_at: at(51),
    });
    assert!(update_comment(&run, &[step], &context(), &update).is_none());
}

/// Whether a Step's comment names its pull request is read from the Run's
/// own Flow: the same entry names it where it runs a pull request Step,
/// and not where it plans.
#[test]
fn a_step_names_its_pull_request_by_the_run_s_own_flow() {
    let run = run();
    let mut ended = step_with(None);
    if let Some(end) = ended.end.as_mut() {
        end.output = "Opened https://github.com/macro/app/pull/7\nMore".to_string();
    }
    let steps = [ended];
    let flow_of = |step: &str| -> crate::domain::flow::Flow {
        serde_json::from_value(serde_json::json!({
            "entries": [{ "id": "plan", "step": step, "on": {} }]
        }))
        .expect("a flow")
    };
    let publishing = flow_of("publish");
    let planning = flow_of("plan");
    let comment = |flow| {
        let context = UpdateContext {
            flow: Some(flow),
            ..context()
        };
        update_comment(&run, &steps, &context, &RunUpdate::StepEnded { seq: 3 })
            .expect("ended")
            .text
    };

    assert!(comment(&publishing).contains("pull/7"));
    assert!(!comment(&planning).contains("pull/7"));
}
