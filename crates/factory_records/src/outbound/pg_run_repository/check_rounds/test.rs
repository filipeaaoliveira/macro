use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{
    CheckRoundOutcome, CheckRoundStart, EntryId, HandBack, Outcome, RunId, RunInsert, StepKey,
    StepSession,
};
use crate::domain::ports::RunRepository;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

/// A started `write` Step of a new Run of Julia's, working in a session it
/// claimed offering `done`.
async fn writing(repo: &PgRunRepository) -> (StepKey, StepSession) {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(id, "code", "Add dark mode", &julia, None, None)
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    let key = StepKey { run_id: id, seq: 1 };
    let entry = EntryId::try_from("write".to_string()).expect("entry");
    repo.start_step(key, &entry, 1).await.expect("start");
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
    };
    repo.claim_session(key, session, &[outcome("done")])
        .await
        .expect("claim");
    (key, session)
}

/// A later prompt offers its own outcomes and starts with no hand back; the
/// same prompt recorded again, as a retried attempt does, keeps the hand
/// back it got. A stop reads the latest prompt.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_later_prompt_offers_its_own_outcomes_and_clears_the_hand_back_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let written = HandBack {
        outcome: outcome("done"),
        output: "Wrote the toggle".to_string(),
    };
    repo.hand_back(session.session_id, &written)
        .await
        .expect("hand back");
    let checks = Uuid::now_v7();
    let offered = [outcome("passed"), outcome("failed")];

    repo.begin_turn(key, checks, &offered).await.expect("turn");
    let cleared = repo.handed_back(key).await.expect("read");
    let passed = HandBack {
        outcome: outcome("passed"),
        output: "web passed".to_string(),
    };
    repo.hand_back(session.session_id, &passed)
        .await
        .expect("hand back");
    repo.begin_turn(key, checks, &offered)
        .await
        .expect("turn again");

    assert_eq!(cleared, None);
    assert_eq!(repo.handed_back(key).await.expect("read"), Some(passed));
    let step = repo
        .session_step(session.session_id)
        .await
        .expect("step")
        .expect("the session's step");
    assert_eq!(step.offered, offered);
    let stopping = repo
        .stopping_session(key, std::time::Duration::from_secs(300))
        .await
        .expect("stopping")
        .expect("a session");
    assert_eq!(stopping.session.session_id, session.session_id);
    assert_eq!(stopping.session.action_id, checks);
}

/// A round keeps its first record and its first end, goes back to the
/// agent in one prompt however often that is recorded, and shows on its
/// Step.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_round_keeps_its_first_record_end_and_fix(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, _) = writing(&repo).await;
    let first = CheckRoundStart {
        round: 1,
        work_output: "Wrote the toggle".to_string(),
        checks: vec!["web".to_string(), "rust".to_string()],
        check_action_id: Uuid::now_v7(),
    };
    let fix = Uuid::now_v7();

    repo.start_check_round(key, &first).await.expect("start");
    repo.start_check_round(
        key,
        &CheckRoundStart {
            work_output: "Another attempt".to_string(),
            check_action_id: Uuid::now_v7(),
            ..first.clone()
        },
    )
    .await
    .expect("start again");
    let before_end = repo
        .send_back_check_round(key, 1, fix)
        .await
        .expect("send back");
    repo.end_check_round(key, 1, CheckRoundOutcome::Failed, "rust: 1 test failed")
        .await
        .expect("end");
    repo.end_check_round(key, 1, CheckRoundOutcome::Passed, "all passed")
        .await
        .expect("end again");
    let sent = repo
        .send_back_check_round(key, 1, fix)
        .await
        .expect("send back");
    let sent_again = repo
        .send_back_check_round(key, 1, Uuid::now_v7())
        .await
        .expect("send back again");

    assert_eq!(before_end, None);
    assert_eq!((sent, sent_again), (Some(fix), Some(fix)));
    let rounds = repo.check_rounds(key).await.expect("rounds");
    assert_eq!(rounds.len(), 1);
    let round = &rounds[0];
    assert_eq!(round.round, 1);
    assert_eq!(round.work_output, "Wrote the toggle");
    assert_eq!(round.check_action_id, first.check_action_id);
    assert_eq!(round.checks, ["web", "rust"]);
    assert_eq!(round.fix_action_id, Some(fix));
    let end = round.end.as_ref().expect("the round ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert_eq!(end.report, "rust: 1 test failed");
    let steps = repo.list_steps(key.run_id).await.expect("steps");
    assert_eq!(steps[0].checks, rounds);
    let by_run = repo
        .list_runs_steps(&[key.run_id])
        .await
        .expect("steps by run");
    assert_eq!(by_run[&key.run_id][0].checks, rounds);
}
