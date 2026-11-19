//! The task thread at the worker: a Run's updates posted to its task once
//! each is recorded, signed as the factory's, once each however often the
//! activity that posts it is tried; a review round told as one comment;
//! nothing posted with the section off; a thread found again, or started
//! anew, when its first comment is not there; and a thread that cannot be
//! posted to never failing the Run. And a notify-only Gate's notice.

use chrono::{TimeDelta, Utc};
use factory_records::domain::models::{
    EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{Finding, ReviewRoundRepository};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, RulesSource, RunRules,
};
use uuid::Uuid;

use super::{FakeExecutor, julia, keep_notify_only_reply, reply_step};
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, FakeTaskNotes, FakeTaskThread};
use crate::domain::flow::StepName;
use crate::domain::gate::{AnswerGate, GateAnswered, GateRuling, GateTelling};
use crate::domain::gate_owner::{FACTORY_SIGNATURE, told_id};
use crate::domain::models::{RunFinish, StepEnding, StepReport, StepRequest};
use crate::domain::ports::{ReviewService, StepService, TaskThread};
use crate::domain::review_rounds::{
    ChangedFile, ReviewRoundClosing, ReviewRoundOpening, ReviewRoundsImpl,
};
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
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
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

/// The Step `seq` of the Run `run_id`, of the entry `entry`, which runs
/// the Step of the same name.
fn step_of(run_id: RunId, seq: u16, entry: &str) -> StepRequest {
    StepRequest {
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        step: StepName::try_from(entry.to_string()).expect("valid"),
        ..reply_step(run_id, seq)
    }
}

/// The link the fake thread gives the Run `run_id`.
fn link(run_id: RunId) -> String {
    FakeTaskThread::default().run_link(run_id)
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
    repo.display_names.lock().expect("names").insert(
        "macro|julia@example.com".to_string(),
        "Julia Ruiz".to_string(),
    );

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
            agent_session: None,
        },
    };
    steps.answer_gate(answered.clone()).await.expect("answer");
    steps.answer_gate(answered).await.expect("answer again");
    // Only a pull request Step names its pull request, whatever another
    // Step's outcome is called.
    let opened =
        "Pull request #12: https://github.com/acme/app/pull/12\n\nIt changes no Control Files.";
    steps
        .start_step(step_of(run_id, 2, "publish"))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 2, "published", opened))
        .await
        .expect("end");
    steps
        .start_step(reply_step(run_id, 3))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 3, "published", opened))
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
                "**A factory run started.** [Open the run]({}); its updates follow in this thread.\n\n{FACTORY_SIGNATURE}",
                link(run_id)
            ),
            format!("Step 1, `reply`, ended `done`.\n\n{FACTORY_SIGNATURE}"),
            format!(
                "Step 1 waits for the run's owner to answer `approve` or `revise`.\n\n> Send the reply?\n\n{FACTORY_SIGNATURE}"
            ),
            format!("Step 1's gate was answered `approve` by Julia Ruiz.\n\n{FACTORY_SIGNATURE}"),
            format!(
                "Step 2, `publish`, ended `published`.\n\nPull request #12: https://github.com/acme/app/pull/12\n\n{FACTORY_SIGNATURE}"
            ),
            format!("Step 3, `reply`, ended `published`.\n\n{FACTORY_SIGNATURE}"),
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
/// could not be posted, looks for the Run's thread, finds none, and starts
/// it anew; the next update finds that one.
#[tokio::test]
async fn an_update_without_its_thread_starts_the_thread_first() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    // The new thread's first comment is stamped now, the one updates reply
    // to at the Run's creation: within one millisecond the two ids are the
    // same and the second update finds its thread without looking.
    repo.runs
        .lock()
        .expect("runs")
        .get_mut(&run_id)
        .expect("run")
        .created_at = Utc::now() - TimeDelta::minutes(1);
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);
    for seq in [1, 2] {
        repo.start_step(
            StepKey { run_id, seq },
            &EntryId::try_from("reply".to_string()).expect("valid"),
            1,
        )
        .await
        .expect("start");
        steps
            .end_step(ending(run_id, seq, "done", "Replied"))
            .await
            .expect("end");
    }

    let posted = thread.posted.lock().expect("posted").clone();
    assert_eq!(posted.len(), 3, "{posted:?}");
    assert!(posted[0].1.text.starts_with("**A factory run started.**"));
    assert_eq!(posted[0].1.thread_id, None);
    assert!(
        posted[1]
            .1
            .text
            .starts_with("Step 1, `reply`, ended `done`.")
    );
    assert!(
        posted[2]
            .1
            .text
            .starts_with("Step 2, `reply`, ended `done`.")
    );
    assert!(
        posted[1..]
            .iter()
            .all(|(_, reply)| reply.thread_id == Some(posted[0].1.id))
    );
    assert_eq!(*thread.looked_for.lock().expect("looked for"), 2);
}

/// Macro takes a comment's own id only within a day of its time: a Run
/// older than a day whose first comment was never posted starts its thread
/// with a first comment of the current time, which every later update
/// finds, and never a second.
#[tokio::test]
async fn a_thread_lost_for_over_a_day_starts_anew_once() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    repo.runs
        .lock()
        .expect("runs")
        .get_mut(&run_id)
        .expect("run")
        .created_at = Utc::now() - TimeDelta::days(2);
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);

    // The Run's start, under an id of its creation, is refused.
    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    assert!(thread.texts().is_empty());
    for seq in [1, 2] {
        if seq == 2 {
            steps
                .start_step(reply_step(run_id, seq))
                .await
                .expect("start");
        }
        steps
            .end_step(ending(run_id, seq, "done", "Replied"))
            .await
            .expect("end");
    }

    let posted = thread.posted.lock().expect("posted").clone();
    let roots: Vec<_> = posted
        .iter()
        .filter(|(_, comment)| comment.thread_id.is_none())
        .collect();
    assert_eq!(roots.len(), 1, "{posted:?}");
    let root = &roots[0].1;
    assert!(root.text.contains(&link(run_id)), "{}", root.text);
    let (seconds, _) = root.id.get_timestamp().expect("v7").to_unix();
    let stamped = i64::try_from(seconds).expect("seconds");
    assert!((Utc::now().timestamp() - stamped).abs() < 60, "a fresh id");
    assert_eq!(posted.len(), 3, "{posted:?}");
    assert!(
        posted[1..]
            .iter()
            .all(|(_, reply)| reply.thread_id == Some(root.id))
    );
}

/// A review round is told as one comment once it closes, with its outcome,
/// its Blockers and each reviewer's result; its reviewers' Steps are not
/// told one by one.
#[tokio::test]
async fn a_review_round_is_one_comment() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let run_id = RunId::generate();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia(),
            task_id: Some(task),
            project_id: None,
            repository: Some(&repository),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules(true, false),
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("the task exists");
    };
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), "aaa".to_string()),
        vec![ChangedFile::from_patch(
            "src/save.rs".to_string(),
            None,
            Some("@@ -1,3 +1,8 @@"),
        )],
    );
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);
    let rounds = ReviewRoundsImpl::new(repo.clone(), github.clone(), repo.clone())
        .with_task_thread(thread.clone());
    steps
        .start_step(step_of(run_id, 1, "write"))
        .await
        .expect("start");
    rounds
        .open_round(ReviewRoundOpening {
            run_id,
            seq: 2,
            round: 1,
            reviewers: 2,
        })
        .await
        .expect("open");
    let entry = EntryId::try_from("review".to_string()).expect("entry");
    let reviewers = [
        (2, "correctness-reviewer", "changes"),
        (3, "scope-reviewer", "pass"),
    ];
    for (seq, role, _) in reviewers {
        let key = StepKey { run_id, seq };
        repo.start_step(key, &entry, 1).await.expect("start");
        repo.with_step_role(key, Role::try_from(role.to_string()).expect("role"));
    }
    let blocker = Finding {
        path: "src/save.rs".to_string(),
        lines: None,
        problem: "It drops the error".to_string(),
        why: "A failed save looks saved".to_string(),
        blocks_anywhere: false,
    };
    repo.record_findings(StepKey { run_id, seq: 2 }, 2, &[blocker])
        .await
        .expect("record");
    repo.record_findings(StepKey { run_id, seq: 3 }, 2, &[])
        .await
        .expect("record");
    for (seq, _, ended) in reviewers {
        steps
            .end_step(ending(run_id, seq, ended, "Reviewed"))
            .await
            .expect("end");
    }

    let closing = ReviewRoundClosing {
        run_id,
        seq: 2,
        failed: false,
        last: false,
    };
    rounds.close_round(closing.clone()).await.expect("close");
    rounds.close_round(closing).await.expect("close again");

    let texts = thread.texts();
    assert_eq!(texts.len(), 2, "{texts:#?}");
    assert_eq!(
        texts[1],
        format!(
            "Review round 1 ended `changes` with 1 blocker: correctness-reviewer `changes`, scope-reviewer `pass`.\n\n{FACTORY_SIGNATURE}"
        )
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

/// An answer the Orchestrator gave for the owner, from their agent session
/// in Macro chat, is posted as the Orchestrator's, not as the owner's own.
#[tokio::test]
async fn an_orchestrator_answer_is_posted_as_the_orchestrator_s() {
    let repo = FakeRepo::default();
    let (run_id, _) = run_on_task(&repo, rules(true, false)).await;
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &FakeNotifier::default(), &thread);
    repo.display_names.lock().expect("names").insert(
        "macro|julia@example.com".to_string(),
        "Julia Ruiz".to_string(),
    );
    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    steps
        .end_step(ending(run_id, 1, "done", "Planned"))
        .await
        .expect("end");
    let key = StepKey { run_id, seq: 1 };
    repo.open_gate(
        key,
        "Approve the plan?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    steps
        .answer_gate(GateAnswered {
            key,
            answer: AnswerGate {
                seq: 1,
                answer: outcome("revise"),
                note: None,
                user_id: julia(),
                approved_control_files: None,
                agent_session: Some(Uuid::from_u128(0xa6e)),
            },
        })
        .await
        .expect("answer");

    let texts = thread.texts();
    assert_eq!(
        texts.last().map(String::as_str),
        Some(
            format!(
                "Step 1's gate was answered `revise` by the Orchestrator, for Julia Ruiz.\n\n{FACTORY_SIGNATURE}"
            )
            .as_str()
        ),
        "{texts:?}"
    );
}

/// A notify-only Gate is posted, and told to the owner with the waiting
/// Gate's kind of notification, not pushed and saying the Run goes on,
/// whatever the owner section says: one notification per entry of the Run,
/// however often its Gate tells, marked done once the Run ends.
#[tokio::test]
async fn a_notify_only_gate_is_told_once_per_entry_until_the_run_ends() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(true, false)).await;
    keep_notify_only_reply(&repo, run_id);
    let notifier = FakeNotifier::default();
    let thread = FakeTaskThread::default();
    let steps = service(&repo, &notifier, &thread);
    for seq in [1, 2] {
        steps
            .start_step(reply_step(run_id, seq))
            .await
            .expect("start");
        steps
            .end_step(ending(run_id, seq, "done", "Planned"))
            .await
            .expect("end");
        steps
            .tell_gate(GateTelling {
                key: StepKey { run_id, seq },
                question: "The plan is ready.".to_string(),
            })
            .await
            .expect("told");
    }

    let sent = notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 2);
    let entry = EntryId::try_from("reply".to_string()).expect("valid");
    assert!(
        sent.iter()
            .all(|notice| notice.notification_id == told_id(run_id, &entry))
    );
    assert!(!sent[0].push);
    assert_eq!(sent[0].task_id, Some(task));
    assert_eq!(sent[0].metadata.question, "The plan is ready.");
    assert!(sent[0].metadata.notify_only);
    let texts = thread.texts();
    assert_eq!(
        texts[2],
        format!(
            "Step 1 tells the run's owner, and the run goes on.\n\n> The plan is ready.\n\n{FACTORY_SIGNATURE}"
        )
    );

    steps
        .end_run(RunFinish {
            run_id,
            outcome: RunOutcome::Success,
        })
        .await
        .expect("end run");
    let done = notifier.done.lock().expect("done").clone();
    assert!(done.contains(&told_id(run_id, &entry)), "{done:?}");

    // A Run that ended tells no one any more.
    steps
        .tell_gate(GateTelling {
            key: StepKey { run_id, seq: 2 },
            question: "The plan is ready.".to_string(),
        })
        .await
        .expect("told");
    assert_eq!(notifier.sent.lock().expect("sent").len(), 2);
}
