//! The Runs waiting on their owner: those that have not ended and wait on
//! a Gate no one answered yet.

use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::{entry, insert, julia, key, outcome, teo};
use crate::domain::gate_replies::GateReplyRepository as _;
use crate::domain::models::{NewRun, RunId, RunInsert, RunOutcome, StandInAnswer};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::{OrchestratorDefinition, RunRules};
use crate::outbound::pg_run_repository::PgRunRepository;

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

/// The Queue counts the owner's Runs waiting on them in person, up to its
/// limit: not a Gate the stand-in answered while the owner may override
/// it, an answered one, or one of a Run that ended.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_queue_counts_gates_waiting_on_the_owner_in_person(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
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
    assert_eq!(repo.queue_size(&julia(), 10).await.expect("size"), 4);
    assert_eq!(repo.queue_size(&julia(), 2).await.expect("size"), 2);

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

    assert_eq!(repo.queue_size(&julia(), 10).await.expect("size"), 1);
    assert_eq!(repo.queue_size(&teo(), 10).await.expect("size"), 0);
}

/// A Run that has not reached a Gate yet holds the place it will ask for;
/// once its first Gate is answered, it holds none until it asks again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_holds_its_place_in_the_queue_before_its_first_gate(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the import").await;
    assert_eq!(repo.queue_size(&julia(), 10).await.expect("size"), 1);

    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key(id, 1), "Approve the plan?", &[outcome("approve")])
        .await
        .expect("open");
    assert_eq!(repo.queue_size(&julia(), 10).await.expect("size"), 1);

    repo.answer_gate(key(id, 1), &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer");
    assert_eq!(repo.queue_size(&julia(), 10).await.expect("size"), 0);
}

/// Under a Queue limit, a Run is kept only while its owner holds no more
/// places than the limit, counted with the Run kept: of starts made at
/// the same time, one after the other, so they never all pass a limit
/// only one of them fits under. A Run kept reads with no limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn starts_at_the_same_time_are_held_to_the_queue_limit(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let limited = RunRules {
        definition: OrchestratorDefinition {
            queue_limit: Some(1),
            ..OrchestratorDefinition::DEFAULT
        },
        ..RunRules::DEFAULT
    };
    let owner = julia();
    let start = |id: RunId| {
        repo.insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan the import",
            user_id: &owner,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: limited,
            automation: None,
        })
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
                RunInsert::Inserted(run) => {
                    assert_eq!(
                        run.standing_rules.map(|rules| rules.definition.queue_limit),
                        Some(None)
                    );
                    Some(run.id)
                }
                RunInsert::QueueFull => None,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kept.len(), 1, "round {round}: {kept:?}");
        for id in kept.into_iter().chain([held]) {
            repo.end_run(id, RunOutcome::Cancelled).await.expect("end");
        }
    }
}
