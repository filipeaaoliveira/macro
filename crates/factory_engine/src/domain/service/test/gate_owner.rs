//! The owner section at the worker: a waiting Gate told to the Run's owner,
//! repeated Blockers left to them, and their comments on the Run's task
//! given to the next Step, apart from their words on a Gate; and none of it
//! with the section off.

use chrono::{TimeDelta, Utc};
use factory_records::domain::models::{
    EntryId, NewRun, Outcome, Role, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{FindingKind, Lines, ReviewFinding, ReviewRound};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, RulesSource, RunRules, StandInKinds, StandingRule,
};
use uuid::Uuid;

use super::{FakeExecutor, julia, reply_step};
use crate::domain::fakes::{FakeGateModel, FakeNotifier, FakeRepo, FakeTaskNotes};
use crate::domain::gate::{
    AnswerGate, GateAnswered, GateRuling, ReplyRead, ReplyReading, StandInAsking,
};
use crate::domain::gate_owner::{OwnerNote, factory_signed, notice_id};
use crate::domain::gate_replies::GateReplySettings;
use crate::domain::gate_replies::service::GateRepliesImpl;
use crate::domain::models::{RunFinish, StepEnding, StepError, StepReport};
use crate::domain::ports::{GateReplyService, StepService};
use crate::domain::service::StepServiceImpl;
use crate::domain::standing_rules::RuleAnswer;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

/// The rules that fix Blockers, with the owner section `owner_gates`.
fn rules(owner_gates: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
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

/// Start the Step `key` of `entry`, end it `outcome`, and open its Gate.
async fn gate_at(repo: &FakeRepo, key: StepKey, entry: &str, ended: &str, answers: &[&str]) {
    let entry = EntryId::try_from(entry.to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &outcome(ended), "Worked on it")
        .await
        .expect("end");
    let answers: Vec<Outcome> = answers.iter().map(|answer| outcome(answer)).collect();
    repo.open_gate(key, "Go on?", &answers).await.expect("open");
}

fn service(
    repo: &FakeRepo,
    executor: &FakeExecutor,
    notifier: &FakeNotifier,
    notes: &FakeTaskNotes,
) -> StepServiceImpl<FakeRepo, FakeExecutor, FakeNotifier, FakeTaskNotes> {
    StepServiceImpl::new(repo.clone(), executor.clone())
        .with_owner_gates(notifier.clone(), notes.clone())
}

/// With the owner section on, a Gate waiting for a person is told to the
/// Run's owner, about its task; once answered, nothing more is told.
#[tokio::test]
async fn a_waiting_gate_is_told_to_the_run_s_owner() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(true)).await;
    let key = StepKey { run_id, seq: 1 };
    gate_at(&repo, key, "plan", "done", &["approve", "revise"]).await;
    let notifier = FakeNotifier::default();
    let steps = service(
        &repo,
        &FakeExecutor::default(),
        &notifier,
        &FakeTaskNotes::default(),
    );

    steps.notify_gate(key).await.expect("notify");
    {
        let sent = notifier.sent.lock().expect("sent");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].owner, julia());
        assert_eq!(sent[0].task_id, task);
        assert_eq!(sent[0].metadata.run_id, run_id.as_uuid());
        assert_eq!(sent[0].metadata.seq, 1);
        assert_eq!(sent[0].metadata.question, "Go on?");
    }

    repo.answer_gate(key, &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer");
    steps.notify_gate(key).await.expect("notify");
    assert_eq!(notifier.sent.lock().expect("sent").len(), 1);
}

/// With the owner section off the factory behaves as before: no Gate is
/// told to anyone, and no comment is read.
#[tokio::test]
async fn with_the_owner_section_off_no_gate_is_told_and_no_comment_read() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(false)).await;
    let key = StepKey { run_id, seq: 1 };
    gate_at(&repo, key, "plan", "done", &["approve", "revise"]).await;
    let notifier = FakeNotifier::default();
    let notes = FakeTaskNotes::default();
    notes.notes.lock().expect("notes").push((
        task,
        OwnerNote {
            written_at: Utc::now() + TimeDelta::minutes(1),
            text: "Keep the old index".to_string(),
        },
    ));
    let executor = FakeExecutor::default();
    let steps = service(&repo, &executor, &notifier, &notes);

    steps.notify_gate(key).await.expect("notify");
    repo.answer_gate(key, &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer");
    steps
        .start_step(reply_step(run_id, 2))
        .await
        .expect("start");
    steps.run_step(reply_step(run_id, 2)).await.expect("run");

    assert!(notifier.sent.lock().expect("sent").is_empty());
    assert!(notes.asked.lock().expect("asked").is_empty());
    assert_eq!(
        *executor.noted.lock().expect("noted"),
        [Vec::<String>::new()]
    );
}

/// The owner's comments on the Run's task since its latest Gate opened
/// reach the next Step; a Step before any Gate reads none.
#[tokio::test]
async fn the_owner_s_comments_since_the_gate_reach_the_next_step() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(true)).await;
    let notes = FakeTaskNotes::default();
    let executor = FakeExecutor::default();
    let steps = service(&repo, &executor, &FakeNotifier::default(), &notes);

    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    steps.run_step(reply_step(run_id, 1)).await.expect("run");
    assert!(notes.asked.lock().expect("asked").is_empty());
    steps
        .end_step(StepEnding {
            key: StepKey { run_id, seq: 1 },
            report: StepReport {
                outcome: Outcome::done(),
                output: "Replied".to_string(),
            },
        })
        .await
        .expect("end");
    let key = StepKey { run_id, seq: 1 };
    repo.open_gate(key, "Go on?", &[outcome("approve")])
        .await
        .expect("open");
    let opened_at = repo.list_steps(run_id).await.expect("steps")[0]
        .gate
        .as_ref()
        .expect("gate")
        .opened_at;
    repo.answer_gate(key, &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer");
    {
        let mut kept = notes.notes.lock().expect("notes");
        kept.push((
            task,
            OwnerNote {
                written_at: opened_at - TimeDelta::minutes(5),
                text: "Before the gate".to_string(),
            },
        ));
        kept.push((
            task,
            OwnerNote {
                written_at: opened_at + TimeDelta::minutes(1),
                text: "The cause: bwrap cannot mount /proc".to_string(),
            },
        ));
        // What the factory posted as the owner is not their note.
        kept.push((
            task,
            OwnerNote {
                written_at: opened_at + TimeDelta::minutes(2),
                text: factory_signed("Reported again in review round 2."),
            },
        ));
    }

    steps
        .start_step(reply_step(run_id, 2))
        .await
        .expect("start");
    steps.run_step(reply_step(run_id, 2)).await.expect("run");

    assert_eq!(*notes.asked.lock().expect("asked"), [(task, opened_at)]);
    assert_eq!(
        executor.noted.lock().expect("noted")[1],
        ["The cause: bwrap cannot mount /proc"]
    );
}

/// Comments that cannot be read now fail the Step's attempt, to be tried
/// again, rather than leave the owner's notes out.
#[tokio::test]
async fn comments_that_cannot_be_read_fail_the_attempt_for_a_retry() {
    let repo = FakeRepo::default();
    let (run_id, _task) = run_on_task(&repo, rules(true)).await;
    gate_at(
        &repo,
        StepKey { run_id, seq: 1 },
        "plan",
        "done",
        &["approve"],
    )
    .await;
    let notes = FakeTaskNotes {
        down: true,
        ..FakeTaskNotes::default()
    };
    let steps = service(
        &repo,
        &FakeExecutor::default(),
        &FakeNotifier::default(),
        &notes,
    );

    let failed = steps.run_step(reply_step(run_id, 2)).await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
}

/// A Blocker of `role` in `path`, at `lines`, found by the Step `seq`.
pub(super) fn blocker(seq: u16, role: &str, path: &str, lines: (u32, u32)) -> ReviewFinding {
    ReviewFinding {
        seq,
        place: 1,
        role: Some(Role::try_from(role.to_string()).expect("valid role")),
        path: path.to_string(),
        lines: Some(Lines {
            start: lines.0,
            end: lines.1,
        }),
        problem: "Drops the error".to_string(),
        why: "The caller never learns it failed".to_string(),
        blocks_anywhere: false,
        kind: Some(FindingKind::Blocker),
        follow_up_task_id: None,
        follow_up_key: None,
        follow_up_commented: false,
    }
}

fn round(visit: u16, seq: u16, findings: Vec<ReviewFinding>) -> ReviewRound {
    ReviewRound {
        round: visit,
        visit,
        seq,
        reviewers: 1,
        head_sha: Some("aaa".to_string()),
        diff_base: Some("main".to_string()),
        fix_diff: visit > 1,
        failure: None,
        started_at: Utc::now(),
        end: None,
        findings,
        answered: Vec::new(),
    }
}

/// A Run whose first review round (Step 1) found a Blocker the rule sent
/// to a fix (Step 2), and whose second round (Step 3) found `again`, at
/// its Blockers Gate.
pub(super) async fn second_round_finding(
    repo: &FakeRepo,
    rules: RunRules,
    again: ReviewFinding,
) -> StepKey {
    let (run_id, _task) = run_on_task(repo, rules).await;
    let first = StepKey { run_id, seq: 1 };
    gate_at(repo, first, "review", "changes", &["fix", "stop"]).await;
    repo.answer_gate_by_rule(first, &outcome("fix"), StandingRule::FixBlockers)
        .await
        .expect("rule fix");
    let fix = EntryId::try_from("fix".to_string()).expect("valid");
    let fixed = StepKey { run_id, seq: 2 };
    repo.start_step(fixed, &fix, 1).await.expect("start");
    repo.end_step(fixed, &Outcome::done(), "Fixed")
        .await
        .expect("end");
    let second = StepKey { run_id, seq: 3 };
    gate_at(repo, second, "review", "changes", &["fix", "stop"]).await;
    let mut rounds = repo.review_rounds.lock().expect("rounds");
    rounds.push((
        run_id,
        round(
            1,
            1,
            vec![blocker(1, "correctness-reviewer", "src/a.rs", (10, 14))],
        ),
    ));
    rounds.push((run_id, round(2, 3, vec![again])));
    second
}

fn blockers_ruling(key: StepKey) -> GateRuling {
    GateRuling {
        key,
        kind: RuleGateKind::Blockers,
        fix_rounds: 1,
        fix_limit: 2,
        answers: vec![outcome("fix"), outcome("stop")],
    }
}

/// Blockers found again, unchanged, after the rule's fix wait for the
/// owner with the owner section on; with it off, or when they changed,
/// the rule sends them to another fix, as before.
#[tokio::test]
async fn repeated_blockers_after_a_rule_fix_wait_for_the_owner() {
    let same = || blocker(3, "correctness-reviewer", "src/a.rs", (12, 16));
    let fixed_by_rule = Some(RuleAnswer {
        rule: StandingRule::FixBlockers,
        answer: outcome("fix"),
    });

    let repo = FakeRepo::default();
    let key = second_round_finding(&repo, rules(true), same()).await;
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());
    assert_eq!(
        steps.rule_gate(blockers_ruling(key)).await.expect("rule"),
        None
    );

    let repo = FakeRepo::default();
    let key = second_round_finding(&repo, rules(false), same()).await;
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());
    assert_eq!(
        steps.rule_gate(blockers_ruling(key)).await.expect("rule"),
        fixed_by_rule
    );

    let repo = FakeRepo::default();
    let elsewhere = blocker(3, "correctness-reviewer", "src/b.rs", (12, 16));
    let key = second_round_finding(&repo, rules(true), elsewhere).await;
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());
    assert_eq!(
        steps.rule_gate(blockers_ruling(key)).await.expect("rule"),
        fixed_by_rule
    );
}

/// An answered Gate's notification is marked done, whoever answered, with
/// the owner section on; with it off nothing is marked.
#[tokio::test]
async fn an_answered_gate_s_notification_is_done() {
    for owner_gates in [true, false] {
        let repo = FakeRepo::default();
        let (run_id, _task) = run_on_task(&repo, rules(owner_gates)).await;
        let key = StepKey { run_id, seq: 1 };
        gate_at(&repo, key, "plan", "done", &["approve", "revise"]).await;
        let notifier = FakeNotifier::default();
        let steps = service(
            &repo,
            &FakeExecutor::default(),
            &notifier,
            &FakeTaskNotes::default(),
        );

        steps
            .answer_gate(GateAnswered {
                key,
                answer: AnswerGate {
                    seq: 1,
                    answer: outcome("approve"),
                    note: None,
                    user_id: julia(),
                    approved_control_files: None,
                    agent_session: None,
                },
            })
            .await
            .expect("answer");

        let done = notifier.done.lock().expect("done").clone();
        if owner_gates {
            assert_eq!(done, [notice_id(run_id, 1)]);
        } else {
            assert!(done.is_empty());
        }
    }
}

/// The owner's notes and their words on a Gate stay apart: a comment on
/// the task answers no Gate, words that answer one reach the next Step as
/// that Gate's answer only, never as an owner's note, and answering in
/// words marks the Gate's notification done.
#[tokio::test]
async fn owner_notes_and_a_reply_on_a_gate_do_not_mix() {
    let repo = FakeRepo::default();
    let (run_id, task) = run_on_task(&repo, rules(true)).await;
    let notes = FakeTaskNotes::default();
    let notifier = FakeNotifier::default();
    let executor = FakeExecutor::default();
    let steps = service(&repo, &executor, &notifier, &notes);
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let gates = GateRepliesImpl::new(repo.clone(), model, GateReplySettings::default())
        .with_notifier(notifier.clone());

    let key = StepKey { run_id, seq: 1 };
    gate_at(&repo, key, "reply", "done", &["approve", "revise"]).await;
    let opened_at = repo.list_steps(run_id).await.expect("steps")[0]
        .gate
        .as_ref()
        .expect("gate")
        .opened_at;
    steps.notify_gate(key).await.expect("notify");
    notes.notes.lock().expect("notes").push((
        task,
        OwnerNote {
            written_at: opened_at + TimeDelta::minutes(1),
            text: "approve".to_string(),
        },
    ));
    // A comment, even one that reads as an answer, leaves the Gate waiting.
    let gate = repo.list_steps(run_id).await.expect("steps")[0]
        .gate
        .clone()
        .expect("gate");
    assert!(gate.answer.is_none());

    let read = gates
        .read_reply(ReplyReading {
            key,
            question: false,
            reply: "Go ahead, but keep the old index".to_string(),
            user_id: julia(),
        })
        .await
        .expect("read");
    assert_eq!(read, ReplyRead::Answered(outcome("approve")));
    assert_eq!(*notifier.done.lock().expect("done"), [notice_id(run_id, 1)]);

    steps
        .start_step(reply_step(run_id, 2))
        .await
        .expect("start");
    steps.run_step(reply_step(run_id, 2)).await.expect("run");
    assert_eq!(*executor.noted.lock().expect("noted"), [["approve"]]);
    let answer = repo.list_steps(run_id).await.expect("steps")[0]
        .gate
        .clone()
        .and_then(|gate| gate.answer)
        .expect("answered");
    assert_eq!(
        answer.note.as_deref(),
        Some("Go ahead, but keep the old index")
    );
}

/// Rules that fix Blockers and stand in for the owner at the Blockers
/// Gate, with the owner section `owner_gates`.
fn standing_in_at_blockers(owner_gates: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            owner_gates,
            stand_in: StandInKinds {
                plan: false,
                blockers: true,
            },
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

fn blockers_asking(key: StepKey, fix_rounds: u16) -> StandInAsking {
    StandInAsking {
        key,
        kind: RuleGateKind::Blockers,
        person_only: false,
        fix_rounds,
        fix_limit: 2,
        answers: vec![outcome("fix"), outcome("stop")],
    }
}

/// The stand-in leaves to the owner the Blockers Gates the rule leaves to
/// them: Blockers found again unchanged after a fix, with the owner section
/// on, and Blockers past the fix rounds the rule may send. It asks the
/// model about neither; Blockers that changed it answers as before.
#[tokio::test]
async fn the_stand_in_leaves_the_owner_s_blockers_to_the_owner() {
    let same = || blocker(3, "correctness-reviewer", "src/a.rs", (12, 16));
    let elsewhere = || blocker(3, "correctness-reviewer", "src/b.rs", (12, 16));
    let stands_in = |repo: &FakeRepo, model: &FakeGateModel| {
        GateRepliesImpl::new(repo.clone(), model.clone(), GateReplySettings::default())
    };

    // Repeated, with the owner section on: the owner's.
    let repo = FakeRepo::default();
    let key = second_round_finding(&repo, standing_in_at_blockers(true), same()).await;
    let model = FakeGateModel::picking(Some("fix"), 0.99);
    let stood = stands_in(&repo, &model)
        .stand_in(blockers_asking(key, 1))
        .await
        .expect("stand in");
    assert_eq!(stood, None);
    assert!(model.asked().is_empty());

    // Past the rule's fix rounds: the owner's, whatever they found.
    let repo = FakeRepo::default();
    let key = second_round_finding(&repo, standing_in_at_blockers(true), elsewhere()).await;
    let model = FakeGateModel::picking(Some("fix"), 0.99);
    let stood = stands_in(&repo, &model)
        .stand_in(blockers_asking(key, 2))
        .await
        .expect("stand in");
    assert_eq!(stood, None);
    assert!(model.asked().is_empty());

    // Changed Blockers within the fix rounds: the stand-in answers.
    let repo = FakeRepo::default();
    let key = second_round_finding(&repo, standing_in_at_blockers(true), elsewhere()).await;
    let model = FakeGateModel::picking(Some("fix"), 0.99);
    let stood = stands_in(&repo, &model)
        .stand_in(blockers_asking(key, 1))
        .await
        .expect("stand in");
    assert_eq!(stood.map(|stood| stood.answer), Some(outcome("fix")));
}

/// A Run that ends while a Gate waits (cancelled, or ended for a person)
/// has that Gate's notification marked done; an answered Gate's is left
/// as its answer left it, and with the owner section off nothing is.
#[tokio::test]
async fn an_open_gate_s_notification_is_done_once_its_run_ends() {
    for owner_gates in [true, false] {
        let repo = FakeRepo::default();
        let (run_id, _task) = run_on_task(&repo, rules(owner_gates)).await;
        let plan = StepKey { run_id, seq: 1 };
        gate_at(&repo, plan, "plan", "done", &["approve", "revise"]).await;
        repo.answer_gate(plan, &outcome("approve"), None, &julia(), None, None)
            .await
            .expect("answer");
        let merge = StepKey { run_id, seq: 2 };
        gate_at(&repo, merge, "merge", "done", &["approve", "revise"]).await;
        let notifier = FakeNotifier::default();
        let steps = service(
            &repo,
            &FakeExecutor::default(),
            &notifier,
            &FakeTaskNotes::default(),
        );

        steps
            .end_run(RunFinish {
                run_id,
                outcome: RunOutcome::Cancelled,
            })
            .await
            .expect("end");

        let done = notifier.done.lock().expect("done").clone();
        if owner_gates {
            assert_eq!(done, [notice_id(run_id, 2)]);
        } else {
            assert!(done.is_empty());
        }
    }
}
