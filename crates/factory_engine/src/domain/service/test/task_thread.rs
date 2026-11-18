//! The task thread at the worker: a Run's updates posted to its task once
//! each is recorded, signed as the factory's, once each however often the
//! activity that posts it is tried; nothing posted with the section off;
//! and a thread that cannot be posted to never failing the Run.

use factory_records::domain::models::{
    EntryId, NewRun, Outcome, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, RulesSource, RunRules,
};
use uuid::Uuid;

use super::{FakeExecutor, julia, reply_step};
use crate::domain::fakes::{FakeNotifier, FakeRepo, FakeTaskNotes, FakeTaskThread};
use crate::domain::gate::{AnswerGate, GateAnswered, GateRuling};
use crate::domain::gate_owner::FACTORY_SIGNATURE;
use crate::domain::models::{RunFinish, StepEnding, StepReport};
use crate::domain::ports::StepService;
use crate::domain::service::StepServiceImpl;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

/// Rules with the task thread section `task_thread` and the owner section
/// `owner_gates`, which fix Blockers.
fn rules(task_thread: bool, owner_gates: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            task_thread,
            owner_gates,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// A Run of Julia's on a task, with `rules`.
async fn run_on_task(repo: &FakeRepo, rules: RunRules) -> (RunId, Uuid) {
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: Some(task),
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
        })
        .await
        .expect("insert")
    else {
        panic!("the task exists");
    };
    (run.id, task)
}

fn service(
    repo: &FakeRepo,
    notifier: &FakeNotifier,
    thread: &FakeTaskThread,
) -> StepServiceImpl<FakeRepo, FakeExecutor, FakeNotifier, FakeTaskNotes, FakeTaskThread> {
    StepServiceImpl::new(repo.clone(), FakeExecutor::default())
        .with_owner_gates(notifier.clone(), FakeTaskNotes::default())
        .with_task_thread(thread.clone())
}

fn ending(run_id: RunId, seq: u16, outcome_name: &str, output: &str) -> StepEnding {
    StepEnding {
        key: StepKey { run_id, seq },
        report: StepReport {
            outcome: outcome(outcome_name),
            output: output.to_string(),
        },
    }
}

/// A Run with the section on posts its start as the thread's first
/// comment, then each Step's end, its Gate waiting, the answer and who
/// gave it, the pull request and the Run's end as replies to it, each
/// signed, on its task; trying each activity again posts nothing more.
#[tokio::test]
async fn a_run_s_updates_are_posted_once_each_to_one_thread_on_its_task() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(true, false)).await;
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);

    for _ in 0..2 {
        steps
            .start_step(reply_step(run_id, 1))
            .await
            .expect("start");
        steps
            .end_step(ending(run_id, 1, "done", "Replied to the request"))
            .await
            .expect("end");
    }
    let key = StepKey { run_id, seq: 1 };
    repo.open_gate(
        key,
        "Send the reply?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    steps.notify_gate(key).await.expect("notify");
    let answered = GateAnswered {
        key,
        answer: AnswerGate {
            seq: 1,
            answer: outcome("approve"),
            note: None,
            user_id: julia(),
            approved_control_files: None,
        },
    };
    steps.answer_gate(answered.clone()).await.expect("answer");
    steps.answer_gate(answered).await.expect("answer again");
    steps
        .start_step(reply_step(run_id, 2))
        .await
        .expect("start");
    steps
        .end_step(ending(
            run_id,
            2,
            "published",
            "Pull request #12: https://github.com/acme/app/pull/12\n\nIt changes no Control Files.",
        ))
        .await
        .expect("end");
    repo.end_run(run_id, RunOutcome::Success)
        .await
        .expect("end run");
    for _ in 0..2 {
        steps
            .end_run(RunFinish {
                run_id,
                outcome: RunOutcome::Success,
            })
            .await
            .expect("end run");
    }

    let posted = thread.posted.lock().expect("posted").clone();
    let texts: Vec<&str> = posted
        .iter()
        .map(|(_, comment)| comment.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            format!(
                "**A factory run started**. Its updates follow in this thread.\n\nRun `{run_id}`.\n\n{FACTORY_SIGNATURE}"
            ),
            format!("Step 1, `reply`, ended `done`.\n\n{FACTORY_SIGNATURE}"),
            format!(
                "Step 1 waits for the run's owner to answer `approve` or `revise`.\n\n> Send the reply?\n\n{FACTORY_SIGNATURE}"
            ),
            format!(
                "Step 1's gate was answered `approve` by julia@example.com.\n\n{FACTORY_SIGNATURE}"
            ),
            format!(
                "Step 2, `reply`, ended `published`.\n\nPull request #12: https://github.com/acme/app/pull/12\n\n{FACTORY_SIGNATURE}"
            ),
            format!("**The factory run ended** `success`.\n\n{FACTORY_SIGNATURE}"),
        ]
    );
    let root = posted[0].1.id;
    assert_eq!(posted[0].1.thread_id, None);
    assert!(
        posted[1..]
            .iter()
            .all(|(_, comment)| comment.thread_id == Some(root))
    );
    assert!(posted.iter().all(|(on, _)| *on == task));
    // Macro takes a comment's own id only as a UUIDv7.
    assert!(
        posted
            .iter()
            .all(|(_, comment)| comment.id.get_version_num() == 7)
    );
}

/// With the section off, as by default, a Run posts nothing.
#[tokio::test]
async fn with_the_section_off_nothing_is_posted() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(false, true)).await;
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);

    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 1, "done", "Replied"))
        .await
        .expect("end");
    steps
        .end_run(RunFinish {
            run_id,
            outcome: RunOutcome::Success,
        })
        .await
        .expect("end run");

    assert!(thread.texts().is_empty());
}

/// A thread that cannot be posted to fails nothing: every record goes on.
#[tokio::test]
async fn a_thread_that_cannot_be_posted_to_fails_nothing() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    let thread = FakeTaskThread {
        down: true,
        ..FakeTaskThread::default()
    };
    let steps = service(&repo, &FakeNotifier::default(), &thread);

    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 1, "done", "Replied"))
        .await
        .expect("end");
    steps
        .end_run(RunFinish {
            run_id,
            outcome: RunOutcome::Success,
        })
        .await
        .expect("end run");

    let recorded = repo.list_steps(run_id).await.expect("steps");
    assert!(recorded[0].end.is_some());
    assert!(thread.texts().is_empty());
}

/// An update whose thread was never started, as when the Run's start
/// could not be posted, starts the thread first.
#[tokio::test]
async fn an_update_without_its_thread_starts_the_thread_first() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);
    repo.start_step(
        StepKey { run_id, seq: 1 },
        &EntryId::try_from("reply".to_string()).expect("valid"),
        1,
    )
    .await
    .expect("start");

    steps
        .end_step(ending(run_id, 1, "done", "Replied"))
        .await
        .expect("end");

    let texts = thread.texts();
    assert_eq!(texts.len(), 2);
    assert!(
        texts[0].starts_with("**A factory run started**"),
        "{texts:?}"
    );
    assert!(
        texts[1].starts_with("Step 1, `reply`, ended `done`."),
        "{texts:?}"
    );
}

/// A Rule Answer is posted with the rule that gave it, once, even when
/// the ruling is asked again.
#[tokio::test]
async fn a_rule_answer_is_posted_with_its_rule() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);
    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 1, "changes", "Two Blockers"))
        .await
        .expect("end");
    let key = StepKey { run_id, seq: 1 };
    let answers = vec![outcome("fix"), outcome("stop")];
    repo.open_gate(key, "Fix the Blockers?", &answers)
        .await
        .expect("open");
    let ruling = GateRuling {
        key,
        kind: RuleGateKind::Blockers,
        fix_rounds: 0,
        fix_limit: 2,
        answers,
    };

    for _ in 0..2 {
        let ruled = steps.rule_gate(ruling.clone()).await.expect("ruled");
        assert!(ruled.is_some());
    }

    let texts = thread.texts();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(
        texts[2],
        format!(
            "Step 1's gate was answered `fix` by the `fix_blockers` Standing Rule.\n\n{FACTORY_SIGNATURE}"
        )
    );
}
