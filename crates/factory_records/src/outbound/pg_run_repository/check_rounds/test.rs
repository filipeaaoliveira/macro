use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{
    AgentName, CheckRoundOutcome, CheckRoundStart, EntryId, HandBack, HandedBack, Outcome,
    PassedOver, Role, RunId, RunInsert, StepKey, StepSession,
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
        agent: Some(agent("primary")),
        bot: Some(Uuid::now_v7()),
    };
    repo.claim_session(key, session.clone(), &writer(), &[outcome("done")])
        .await
        .expect("claim");
    (key, session)
}

fn writer() -> Role {
    Role::try_from("writer".to_string()).expect("valid role")
}

fn agent(name: &str) -> AgentName {
    AgentName::try_from(name.to_string()).expect("valid agent")
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
    // A stale attempt's earlier prompt, whose id its worker made later.
    let stale = Uuid::now_v7();
    let offered = [outcome("passed"), outcome("failed")];

    let began = repo
        .begin_turn(key, session.session_id, checks, 2, &offered)
        .await
        .expect("turn");
    let cleared = repo.handed_back(key, checks).await.expect("read");
    let passed = HandBack {
        outcome: outcome("passed"),
        output: "web passed".to_string(),
    };
    repo.hand_back(session.session_id, &passed)
        .await
        .expect("hand back");
    let again = repo
        .begin_turn(key, session.session_id, checks, 2, &offered)
        .await
        .expect("turn again");

    let rewound = repo
        .begin_turn(key, session.session_id, stale, 1, &offered)
        .await
        .expect("an earlier turn");

    assert!(began && again);
    assert!(!rewound);
    assert_eq!(cleared, HandedBack::Nothing);
    assert_eq!(
        repo.handed_back(key, checks).await.expect("read"),
        HandedBack::Handed(passed)
    );
    // The work's prompt reads as gone on from, never with the checks'
    // answer.
    assert_eq!(
        repo.handed_back(key, session.action_id)
            .await
            .expect("read"),
        HandedBack::Superseded
    );
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

/// The Step's place orders its later prompts, not their ids, which each
/// worker makes on its own clock: a fix whose id sorts before the checks'
/// it fixes is still recorded after them.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_later_prompt_is_later_by_its_place_not_its_id(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    // Made on a worker whose clock lags the one that made the checks'.
    let fix = Uuid::now_v7();
    let checks = Uuid::now_v7();
    let offered = [outcome("passed"), outcome("failed")];

    let checked = repo
        .begin_turn(key, session.session_id, checks, 2, &offered)
        .await
        .expect("checks");
    let fixing = repo
        .begin_turn(key, session.session_id, fix, 3, &[outcome("done")])
        .await
        .expect("fix");
    let checks_again = repo
        .begin_turn(key, session.session_id, checks, 2, &offered)
        .await
        .expect("checks again");

    assert!(fix < checks);
    assert!(checked && fixing);
    assert!(!checks_again);
    let stopping = repo
        .stopping_session(key, std::time::Duration::from_secs(300))
        .await
        .expect("stopping")
        .expect("a session");
    assert_eq!(stopping.session.action_id, fix);
}

/// A later prompt goes only to the session the Step works in: one named
/// for a session it left is not recorded. Once a later prompt is recorded,
/// the Agent at work is never passed over, so the rounds stay in its
/// session.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_later_prompt_stays_in_the_session_the_step_works_in(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let offered = [outcome("passed"), outcome("failed")];

    let elsewhere_began = repo
        .begin_turn(key, Uuid::now_v7(), Uuid::now_v7(), 2, &offered)
        .await
        .expect("turn in another session");
    let elsewhere = repo
        .stopping_session(key, std::time::Duration::from_secs(300))
        .await
        .expect("stopping")
        .expect("a session");
    let checks = Uuid::now_v7();
    repo.begin_turn(key, session.session_id, checks, 2, &offered)
        .await
        .expect("turn");
    let next = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(agent("backup")),
        bot: Some(Uuid::now_v7()),
    };
    let passed = repo
        .pass_session(
            key,
            session.session_id,
            &writer(),
            &PassedOver {
                agent: agent("primary"),
                reason: "ai_allowance_exhausted".to_string(),
            },
            Some(&next),
        )
        .await
        .expect("pass");

    assert!(!elsewhere_began);
    assert_eq!(elsewhere.session.action_id, session.action_id);
    assert!(!passed);
    let claimed = repo
        .claimed_session(key)
        .await
        .expect("claimed")
        .expect("a session");
    assert_eq!(claimed, session);
    let stopping = repo
        .stopping_session(key, std::time::Duration::from_secs(300))
        .await
        .expect("stopping")
        .expect("a session");
    assert_eq!(stopping.session.action_id, checks);
    assert_eq!(stopping.session.bot, session.bot);
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
        commands: vec!["bun run check".to_string(), "cargo test".to_string()],
        declared: r#"{"checks":[]}"#.to_string(),
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
    assert_eq!(round.commands, ["bun run check", "cargo test"]);
    assert_eq!(round.declared, r#"{"checks":[]}"#);
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

/// A round the agent's quota ended is kept as ended by it: an open round
/// ends failed with the report given, one that failed keeps its report,
/// and one that passed is left alone.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_round_the_quota_ended_is_kept_as_ended_by_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, _) = writing(&repo).await;
    for round in 1..=3 {
        let start = CheckRoundStart {
            round,
            work_output: "Wrote the toggle".to_string(),
            checks: vec!["web".to_string()],
            commands: vec!["bun run check".to_string()],
            declared: r#"{"checks":[]}"#.to_string(),
            check_action_id: Uuid::now_v7(),
        };
        repo.start_check_round(key, &start).await.expect("start");
    }
    repo.end_check_round(key, 1, CheckRoundOutcome::Passed, "web passed")
        .await
        .expect("end");
    repo.end_check_round(key, 2, CheckRoundOutcome::Failed, "web: exit 1")
        .await
        .expect("end");

    for round in 1..=3 {
        repo.end_check_round_on_quota(key, round, "The quota ran out.")
            .await
            .expect("quota");
    }
    repo.end_check_round_on_quota(key, 3, "Again.")
        .await
        .expect("quota again");

    let rounds = repo.check_rounds(key).await.expect("rounds");
    let kept: Vec<(bool, CheckRoundOutcome, &str)> = rounds
        .iter()
        .map(|round| {
            let end = round.end.as_ref().expect("ended");
            (round.quota_ran_out, end.outcome, end.report.as_str())
        })
        .collect();
    assert_eq!(
        kept,
        [
            (false, CheckRoundOutcome::Passed, "web passed"),
            (true, CheckRoundOutcome::Failed, "web: exit 1"),
            (true, CheckRoundOutcome::Failed, "The quota ran out."),
        ]
    );
}
