//! The Runs waiting on their owner: those that have not ended and wait on
//! a Gate no one answered yet.

use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::{entry, insert, julia, key, outcome, teo};
use crate::domain::gate_replies::GateReplyRepository as _;
use crate::domain::models::{
    NewRun, QueueHold, QueuePlaces, RunId, RunInsert, RunOutcome, StandInAnswer,
};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::{PgRunRepository, queue_places};

/// A Run waits on its owner from when its Gate opens until it is
/// answered or the Run ends; Runs with no Gate, and other people's, are
/// left out, and the newest come first, up to the limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_waits_on_its_owner_while_its_gate_is_open(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let answers = [outcome("approve"), outcome("revise")];

    let older = RunId::generate();
    insert(&repo, older, "Plan the migration").await;
    let newer = RunId::generate();
    insert(&repo, newer, "Plan the rollback").await;
    let working = RunId::generate();
    insert(&repo, working, "Plan the backfill").await;
    for id in [older, newer, working] {
        repo.start_step(key(id, 1), &entry("plan"), 1)
            .await
            .expect("start");
    }
    for id in [older, newer] {
        repo.open_gate(key(id, 1), "Approve the plan?", &answers)
            .await
            .expect("open");
    }
    // Someone else's Run waiting on its Gate is theirs to list.
    let theirs = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: theirs,
            flow: "plan",
            request: "Plan the cutover",
            user_id: &teo(),
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    repo.start_step(key(theirs, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key(theirs, 1), "Approve the plan?", &answers)
        .await
        .expect("open");

    let ids = |runs: Vec<crate::domain::models::Run>| -> Vec<RunId> {
        runs.into_iter().map(|run| run.id).collect()
    };
    let waiting = repo.list_waiting_runs(&julia(), 10).await.expect("list");
    assert_eq!(ids(waiting), [newer, older]);
    let newest = repo.list_waiting_runs(&julia(), 1).await.expect("list");
    assert_eq!(ids(newest), [newer]);

    repo.answer_gate(
        key(older, 1),
        &outcome("approve"),
        None,
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    repo.end_run(newer, RunOutcome::Cancelled)
        .await
        .expect("cancel");
    let waiting = repo.list_waiting_runs(&julia(), 10).await.expect("list");
    assert!(waiting.is_empty());
}

/// The Flows whose Runs hold a place before their first Gate, here.
const ASKING: &[&str] = &["plan"];

/// The places `user_id` holds, counted up to `limit`.
async fn places(pool: &PgPool, user_id: &MacroUserIdStr<'static>, limit: u16) -> QueuePlaces {
    let mut transaction = pool.begin().await.expect("begin");
    queue_places(&mut transaction, user_id, ASKING, limit)
        .await
        .expect("count")
}

/// The Queue counts the owner's Runs waiting on them in person, up to its
/// limit: not a Gate the stand-in answered while the owner may override
/// it, an answered one, or one of a Run that ended.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_queue_counts_gates_waiting_on_the_owner_in_person(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let answers = [outcome("approve"), outcome("revise")];
    let mut runs = Vec::new();
    for request in ["One", "Two", "Three", "Four"] {
        let id = RunId::generate();
        insert(&repo, id, request).await;
        repo.start_step(key(id, 1), &entry("plan"), 1)
            .await
            .expect("start");
        repo.open_gate(key(id, 1), "Approve the plan?", &answers)
            .await
            .expect("open");
        runs.push(id);
    }
    assert_eq!(
        places(&pool, &julia(), 10).await,
        QueuePlaces {
            waiting: 4,
            starting: 0
        }
    );
    assert_eq!(places(&pool, &julia(), 2).await.held(), 2);

    let stand_in = StandInAnswer {
        answer: outcome("approve"),
        reason: "The owner approved every plan like it.".to_string(),
        confidence: 92,
        until: Utc::now() + TimeDelta::minutes(10),
    };
    repo.stand_in(key(runs[0], 1), &stand_in)
        .await
        .expect("stand in");
    repo.answer_gate(
        key(runs[1], 1),
        &outcome("approve"),
        None,
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    repo.end_run(runs[2], RunOutcome::Cancelled)
        .await
        .expect("cancel");

    assert_eq!(
        places(&pool, &julia(), 10).await,
        QueuePlaces {
            waiting: 1,
            starting: 0
        }
    );
    assert_eq!(places(&pool, &teo(), 10).await.held(), 0);
}

/// A Run of a Flow with a Gate of its own holds the place it will ask for
/// before it reaches a Gate, and is listed as starting; once its first Gate
/// is answered, it holds none until it asks again. A Run of a Flow with no
/// Gate holds none while it runs.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_holds_its_place_in_the_queue_before_its_first_gate(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Plan the import").await;
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "reply",
            request: "Say hello",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    assert_eq!(
        places(&pool, &julia(), 10).await,
        QueuePlaces {
            waiting: 0,
            starting: 1
        }
    );
    let starting = repo
        .list_starting_runs(&julia(), ASKING, 10)
        .await
        .expect("list");
    assert_eq!(starting.iter().map(|run| run.id).collect::<Vec<_>>(), [id]);

    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key(id, 1), "Approve the plan?", &[outcome("approve")])
        .await
        .expect("open");
    assert_eq!(
        places(&pool, &julia(), 10).await,
        QueuePlaces {
            waiting: 1,
            starting: 0
        }
    );
    assert!(
        repo.list_starting_runs(&julia(), ASKING, 10)
            .await
            .expect("list")
            .is_empty()
    );

    repo.answer_gate(key(id, 1), &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer");
    assert_eq!(places(&pool, &julia(), 10).await.held(), 0);
}

/// Under a Queue limit, a Run is kept only while its owner holds fewer
/// places than the limit before it, so a limit of 2 lets them hold at
/// most 2; starts made at the same time are
/// counted one after the other, each with the places of those kept before
/// it, so they never all pass a limit only one of them fits under. A
/// refused start says which places were held. A Run kept reads with no
/// limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn starts_at_the_same_time_are_held_to_the_queue_limit(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let hold = QueueHold {
        limit: Some(2),
        asking_flows: ASKING,
    };
    let owner = julia();
    let start = |id: RunId| {
        repo.insert_run_in_queue(
            NewRun {
                id,
                flow: "plan",
                request: "Plan the import",
                user_id: &owner,
                task_id: None,
                repository: None,
                created_in_person: false,
                control_file_patterns: &[],
                control_files_team_id: None,
                standing_rules: RunRules::DEFAULT,
                automation: None,
                definition: None,
                kept_flow: None,
            },
            hold,
        )
    };

    for round in 0..10 {
        // One place is held already: one more fits under the limit.
        let held = RunId::generate();
        insert(&repo, held, "Plan the export").await;
        let ids: Vec<RunId> = (0..4).map(|_| RunId::generate()).collect();
        let results = futures::future::join_all(ids.iter().map(|id| start(*id))).await;
        let kept: Vec<RunId> = results
            .into_iter()
            .map(|result| result.expect("insert"))
            .filter_map(|insert| match insert {
                RunInsert::Inserted(run) => Some(run.id),
                RunInsert::QueueFull(places) => {
                    assert_eq!(
                        places,
                        QueuePlaces {
                            waiting: 0,
                            starting: 2
                        }
                    );
                    None
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kept.len(), 1, "round {round}: {kept:?}");
        for id in kept.into_iter().chain([held]) {
            repo.end_run(id, RunOutcome::Cancelled).await.expect("end");
        }
    }
}
