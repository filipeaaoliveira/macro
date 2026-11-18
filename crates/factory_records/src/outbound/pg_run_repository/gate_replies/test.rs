//! Gate replies, the stand-in's answers, and the answered Gates they read.

use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::super::test::{entry, insert, julia, key, outcome};
use crate::domain::gate_replies::GateReplyRepository;
use crate::domain::models::{GateKind, NewRun, RunId, RunInsert, StandInAnswer, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::PgRunRepository;

/// Start the Step `key` of the entry `plan` and open its plan Gate.
async fn open_plan_gate(repo: &PgRunRepository, key: StepKey) {
    repo.start_step(key, &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key, "Approve?", &[outcome("approve"), outcome("revise")])
        .await
        .expect("open");
}

fn stand_in(answer: &str) -> StandInAnswer {
    StandInAnswer {
        answer: outcome(answer),
        reason: "The owner approved every plan like it.".to_string(),
        confidence: 92,
        until: Utc::now() + TimeDelta::minutes(10),
    }
}

/// A reply the factory cannot read is kept as asked back, replaced by the
/// next one; a reply it reads answers the Gate once, keeping the words and
/// how sure it was, and an answer after it changes nothing.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_reply_is_asked_back_or_answers_the_gate_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = RunId::generate();
    insert(&repo, run_id, "Fix it").await;
    open_plan_gate(&repo, key(run_id, 1)).await;

    assert!(repo.ask_back(key(run_id, 1), "hmm").await.expect("ask"));
    assert!(
        repo.ask_back(key(run_id, 1), "not sure yet")
            .await
            .expect("ask again")
    );
    let steps = repo.list_steps(run_id).await.expect("steps");
    let gate = steps[0].gate.as_ref().expect("gate");
    assert_eq!(
        gate.asked_back.as_ref().map(|asked| asked.reply.as_str()),
        Some("not sure yet")
    );
    assert_eq!(gate.answer, None);

    let kept = repo
        .answer_gate_from_reply(
            key(run_id, 1),
            &outcome("approve"),
            "Looks right, go ahead.",
            &julia(),
            Some(93),
        )
        .await
        .expect("answer")
        .expect("kept");
    assert_eq!(kept.answer, outcome("approve"));
    assert_eq!(kept.note.as_deref(), Some("Looks right, go ahead."));
    assert_eq!(kept.reply_confidence, Some(93));
    assert_eq!(kept.user_id, Some(julia()));

    let again = repo
        .answer_gate_from_reply(
            key(run_id, 1),
            &outcome("revise"),
            "Wait, no.",
            &julia(),
            Some(99),
        )
        .await
        .expect("answer again")
        .expect("kept");
    assert_eq!(again.answer, outcome("approve"));
    assert!(
        !repo
            .ask_back(key(run_id, 1), "late")
            .await
            .expect("ask late")
    );
}

/// The stand-in's answer is kept once, shown on the Gate, and answers it
/// only when the owner let it stand; an owner's answer first wins.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_stand_in_answers_only_a_gate_the_owner_left(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = RunId::generate();
    insert(&repo, run_id, "Fix it").await;
    open_plan_gate(&repo, key(run_id, 1)).await;
    open_plan_gate(&repo, key(run_id, 2)).await;

    let kept = repo
        .stand_in(key(run_id, 1), &stand_in("approve"))
        .await
        .expect("stand in")
        .expect("kept");
    assert_eq!(kept.answer, outcome("approve"));
    let second = repo
        .stand_in(key(run_id, 1), &stand_in("revise"))
        .await
        .expect("stand in again")
        .expect("kept");
    assert_eq!(second.answer, outcome("approve"));

    let answered = repo
        .answer_gate_by_stand_in(key(run_id, 1))
        .await
        .expect("let stand")
        .expect("answered");
    assert_eq!(answered.answer, outcome("approve"));
    assert!(answered.by_stand_in);
    assert_eq!(answered.user_id, None);
    let steps = repo.list_steps(run_id).await.expect("steps");
    let gate = steps[0].gate.as_ref().expect("gate");
    assert_eq!(gate.stand_in.as_ref().map(|s| s.confidence), Some(92));
    assert!(
        gate.answer
            .as_ref()
            .is_some_and(|answer| answer.by_stand_in)
    );

    // The owner overrides in the window: their answer stands, and the
    // stand-in's stays beside it.
    repo.stand_in(key(run_id, 2), &stand_in("approve"))
        .await
        .expect("stand in");
    repo.answer_gate(key(run_id, 2), &outcome("revise"), None, &julia(), None)
        .await
        .expect("override");
    let kept = repo
        .answer_gate_by_stand_in(key(run_id, 2))
        .await
        .expect("window ends")
        .expect("answered");
    assert_eq!(kept.answer, outcome("revise"));
    assert!(!kept.by_stand_in);
    assert_eq!(
        repo.stand_in(key(run_id, 2), &stand_in("approve"))
            .await
            .expect("stand in after"),
        None
    );

    // A Gate the stand-in never answered has nothing to let stand, nor
    // one whose stand-in answer was taken back.
    open_plan_gate(&repo, key(run_id, 3)).await;
    assert_eq!(
        repo.answer_gate_by_stand_in(key(run_id, 3))
            .await
            .expect("nothing"),
        None
    );
    repo.stand_in(key(run_id, 3), &stand_in("approve"))
        .await
        .expect("stand in");
    repo.withdraw_stand_in(key(run_id, 3))
        .await
        .expect("withdraw");
    assert_eq!(
        repo.answer_gate_by_stand_in(key(run_id, 3))
            .await
            .expect("withdrawn"),
        None
    );
    let steps = repo.list_steps(run_id).await.expect("steps");
    assert_eq!(
        steps[2]
            .gate
            .as_ref()
            .and_then(|gate| gate.stand_in.clone()),
        None
    );
}

/// The owner's past answers to the same Flow's entry are read newest
/// first, without the Run asking, a rule's, the stand-in's, an agent's
/// question or another Flow's; every Gate a person answered with a button
/// is read for a replay, and none read from their words.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn past_answers_are_the_owner_s_own(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let older = RunId::generate();
    insert(&repo, older, "Older").await;
    open_plan_gate(&repo, key(older, 1)).await;
    repo.answer_gate(
        key(older, 1),
        &outcome("revise"),
        Some("Cover the edge cases"),
        &julia(),
        None,
    )
    .await
    .expect("answer");
    open_plan_gate(&repo, key(older, 2)).await;
    repo.stand_in(key(older, 2), &stand_in("approve"))
        .await
        .expect("stand in");
    repo.answer_gate_by_stand_in(key(older, 2))
        .await
        .expect("let stand");
    // An agent's question asked at the same entry is not a past answer to
    // the entry's Gate.
    repo.start_step(key(older, 3), &entry("plan"), 2)
        .await
        .expect("start");
    repo.end_step(key(older, 3), &outcome("question"), "Which column?")
        .await
        .expect("end");
    repo.open_gate(key(older, 3), "Which column?", &[outcome("answer")])
        .await
        .expect("open");
    repo.answer_gate(
        key(older, 3),
        &outcome("answer"),
        Some("Due date"),
        &julia(),
        None,
    )
    .await
    .expect("answer the question");

    // Nor is an answer to the same entry in a Run of another Flow.
    let other_flow = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: other_flow,
            flow: "code",
            request: "Elsewhere",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert")
    else {
        panic!("kept");
    };
    open_plan_gate(&repo, key(other_flow, 1)).await;
    repo.answer_gate(
        key(other_flow, 1),
        &outcome("approve"),
        None,
        &julia(),
        None,
    )
    .await
    .expect("answer");

    let current = RunId::generate();
    insert(&repo, current, "Current").await;
    open_plan_gate(&repo, key(current, 1)).await;
    repo.answer_gate(key(current, 1), &outcome("approve"), None, &julia(), None)
        .await
        .expect("answer");
    // An answer the model read from words is not the replay's ground
    // truth.
    open_plan_gate(&repo, key(current, 2)).await;
    repo.answer_gate_from_reply(
        key(current, 2),
        &outcome("approve"),
        "Go ahead",
        &julia(),
        Some(90),
    )
    .await
    .expect("answer from a reply");

    let past = repo
        .past_answers(&julia(), &entry("plan"), current, 10)
        .await
        .expect("past");
    assert_eq!(past.len(), 1);
    assert_eq!(past[0].answer, outcome("revise"));
    assert_eq!(past[0].note.as_deref(), Some("Cover the edge cases"));

    let answered = repo.answered_gates(10).await.expect("answered");
    let read: Vec<_> = answered
        .iter()
        .map(|gate| (gate.run_id, gate.seq, gate.answer.as_str().to_string()))
        .collect();
    assert_eq!(
        read,
        [
            (older, 1, "revise".to_string()),
            (older, 3, "answer".to_string()),
            (other_flow, 1, "approve".to_string()),
            (current, 1, "approve".to_string())
        ]
    );
    assert_eq!(answered[0].kind, GateKind::Flow);
    assert_eq!(answered[0].request, "Older");
    assert_eq!(answered[0].flow.as_deref(), Some("plan"));
}
