//! Where the factory places agent Steps: the Machine an agent's bot is bound
//! to, and the slots its Steps hold on it.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::{entry, insert, key, outcome};
use crate::domain::machines::{MachineRepository, SlotTake, StepMachine};
use crate::domain::models::RunId;
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

/// Julia's agent bound to her Machine `m4` (`fixtures/agents.sql`).
const WRITER: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b1);
/// Julia's agent on an in-memory runtime, bound to no Machine.
const CLOUD: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b2);
/// Her Machine `m4`.
const M4: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00a1);

/// How long after its start a Step may take a slot, in these tests.
const WITHIN: Duration = Duration::from_secs(90 * 60);

/// Take a slot on `machine` for `step`, as placement does.
async fn take(
    repo: &PgRunRepository,
    step: crate::domain::models::StepKey,
    machine: Uuid,
) -> SlotTake {
    repo.take_machine_slot(step, machine, WITHIN)
        .await
        .expect("take")
}

/// Give `m4` `slots` and the tags `gpu` and `rust-build`.
async fn set_m4(pool: &PgPool, slots: Option<i32>) {
    sqlx::query!(
        "UPDATE harnesses SET step_slots = $1, tags = '{gpu,rust-build}' WHERE id = $2",
        slots,
        M4,
    )
    .execute(pool)
    .await
    .expect("set m4");
}

/// A new Run of Julia's with its first Step started.
async fn started(repo: &PgRunRepository) -> RunId {
    let id = RunId::generate();
    insert(repo, id, "Plan the export").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    id
}

/// A bot's Machine reads with its slots and tags; a bot bound to none, and
/// one whose Machine was removed, read none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_bot_reads_the_machine_it_is_bound_to(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(2)).await;

    assert_eq!(
        repo.bot_machine(WRITER).await.expect("read"),
        Some(StepMachine {
            id: M4,
            name: "m4".to_string(),
            step_slots: Some(2),
            tags: vec!["gpu".to_string(), "rust-build".to_string()],
        })
    );
    assert_eq!(repo.bot_machine(CLOUD).await.expect("read"), None);

    sqlx::query!(
        "UPDATE agent_configs SET harness_id = NULL WHERE bot_id = $1",
        WRITER
    )
    .execute(&pool)
    .await
    .expect("unbind");
    assert_eq!(repo.bot_machine(WRITER).await.expect("read"), None);
}

/// A Machine with one slot runs one Step: a second waits, saying on which
/// Machine and since when, until the first ends. A retried attempt of the
/// Step that holds the slot keeps it, and a Machine with no limit takes
/// every Step.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_full_machine_makes_the_next_step_wait_until_a_slot_frees(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(1)).await;
    let first = started(&repo).await;
    let second = started(&repo).await;

    assert_eq!(take(&repo, key(first, 1), M4).await, SlotTake::Taken);
    assert_eq!(
        take(&repo, key(first, 1), M4).await,
        SlotTake::Taken,
        "a retried attempt keeps its slot"
    );
    assert_eq!(take(&repo, key(second, 1), M4).await, SlotTake::Full);

    repo.wait_for_machine_slot(key(second, 1), &["m4".to_string()])
        .await
        .expect("wait");
    let waiting = repo.list_steps(second).await.expect("steps")[0]
        .runtime
        .clone();
    let since = waiting.waiting_for_slot_since.expect("waiting");
    assert_eq!(waiting.waiting_on_machines, ["m4"]);
    repo.wait_for_machine_slot(key(second, 1), &["m4".to_string()])
        .await
        .expect("wait again");
    assert_eq!(
        repo.list_steps(second).await.expect("steps")[0]
            .runtime
            .waiting_for_slot_since,
        Some(since),
        "a Step waits since it first did"
    );

    repo.end_step(key(first, 1), &outcome("done"), "Planned")
        .await
        .expect("end");
    assert_eq!(take(&repo, key(second, 1), M4).await, SlotTake::Taken);
    let placed = repo.list_steps(second).await.expect("steps")[0]
        .runtime
        .clone();
    assert_eq!(placed.waiting_for_slot_since, None);
    assert!(placed.waiting_on_machines.is_empty());

    set_m4(&pool, None).await;
    let third = started(&repo).await;
    assert_eq!(take(&repo, key(third, 1), M4).await, SlotTake::Taken);
}

/// Steps that hold no slot, of another Machine, or of a Run that ended, do
/// not count against a Machine's slots.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn only_running_steps_on_the_machine_hold_its_slots(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(1)).await;
    let elsewhere = started(&repo).await;
    let m3 = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00a2);
    assert_eq!(take(&repo, key(elsewhere, 1), m3).await, SlotTake::Taken);
    let _unplaced = started(&repo).await;

    let ended = started(&repo).await;
    assert_eq!(take(&repo, key(ended, 1), M4).await, SlotTake::Taken);
    sqlx::query!(
        "UPDATE factory_run SET outcome = 'cancelled', ended_at = now() WHERE id = $1",
        ended.as_uuid()
    )
    .execute(&pool)
    .await
    .expect("end the run");

    let next = started(&repo).await;
    assert_eq!(take(&repo, key(next, 1), M4).await, SlotTake::Taken);
}

/// Move the start of the Step `step` back by `minutes`.
async fn started_ago(pool: &PgPool, step: crate::domain::models::StepKey, minutes: i32) {
    sqlx::query!(
        "UPDATE factory_step SET started_at = now() - make_interval(mins => $3)
         WHERE run_id = $1 AND seq = $2",
        step.run_id.as_uuid(),
        i32::from(step.seq),
        minutes,
    )
    .execute(pool)
    .await
    .expect("move the start");
}

/// A Step that started longer ago than its work can run holds no slot, as
/// one whose Run's workflow went without recording its end; a Step that
/// started too long ago finds a full Machine too late, but one that holds a slot
/// keeps it however late. Letting go frees the slot and the wait.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn old_steps_hold_no_slot_and_late_steps_take_none(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(1)).await;
    let gone = started(&repo).await;
    assert_eq!(take(&repo, key(gone, 1), M4).await, SlotTake::Taken);
    let next = started(&repo).await;
    assert_eq!(take(&repo, key(next, 1), M4).await, SlotTake::Full);

    started_ago(&pool, key(gone, 1), 180).await;
    assert_eq!(take(&repo, key(next, 1), M4).await, SlotTake::Taken);

    let late = started(&repo).await;
    started_ago(&pool, key(late, 1), 100).await;
    assert_eq!(take(&repo, key(late, 1), M4).await, SlotTake::TooLate);
    repo.end_step(key(next, 1), &outcome("done"), "Planned")
        .await
        .expect("end");

    let holder = started(&repo).await;
    assert_eq!(take(&repo, key(holder, 1), M4).await, SlotTake::Taken);
    started_ago(&pool, key(holder, 1), 100).await;
    assert_eq!(
        take(&repo, key(holder, 1), M4).await,
        SlotTake::Taken,
        "a Step keeps the slot it holds"
    );

    repo.wait_for_machine_slot(key(late, 1), &["m4".to_string()])
        .await
        .expect("wait");
    repo.release_machine_slot(key(holder, 1))
        .await
        .expect("release");
    repo.release_machine_slot(key(late, 1))
        .await
        .expect("release");
    assert_eq!(
        repo.list_steps(late).await.expect("steps")[0]
            .runtime
            .waiting_for_slot_since,
        None
    );
    let after = started(&repo).await;
    assert_eq!(take(&repo, key(after, 1), M4).await, SlotTake::Taken);
}

/// Steps that ask at once for a Machine's last slot: exactly one takes it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn concurrent_steps_never_both_take_the_last_slot(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(1)).await;
    for _ in 0..20 {
        let steps = [
            started(&repo).await,
            started(&repo).await,
            started(&repo).await,
            started(&repo).await,
        ];
        let taken = futures::future::join_all(
            steps
                .iter()
                .map(|run| repo.take_machine_slot(key(*run, 1), M4, WITHIN)),
        )
        .await;
        let won = taken
            .into_iter()
            .map(|taken| taken.expect("take"))
            .filter(|taken| *taken == SlotTake::Taken)
            .count();
        assert_eq!(won, 1, "one Step takes the last slot");
        for run in steps {
            repo.end_step(key(run, 1), &outcome("done"), "Planned")
                .await
                .expect("end");
        }
    }
}

/// A Step that moves to another Machine frees the slot it held: the next
/// Step takes it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_step_that_moves_frees_its_old_machine(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(1)).await;
    let m3 = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00a2);
    let moving = started(&repo).await;
    let next = started(&repo).await;
    assert_eq!(take(&repo, key(moving, 1), M4).await, SlotTake::Taken);
    assert_eq!(take(&repo, key(next, 1), M4).await, SlotTake::Full);

    assert_eq!(take(&repo, key(moving, 1), m3).await, SlotTake::Taken);
    assert_eq!(take(&repo, key(next, 1), M4).await, SlotTake::Taken);
}

/// A Step past the time to wait for a slot still takes a free one, on a
/// Machine with a slot left as on one with no limit: only a full Machine is
/// too late.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_late_step_takes_a_free_slot(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    set_m4(&pool, Some(2)).await;
    let busy = started(&repo).await;
    assert_eq!(take(&repo, key(busy, 1), M4).await, SlotTake::Taken);
    let late = started(&repo).await;
    started_ago(&pool, key(late, 1), 100).await;
    assert_eq!(take(&repo, key(late, 1), M4).await, SlotTake::Taken);

    set_m4(&pool, None).await;
    let unlimited = started(&repo).await;
    started_ago(&pool, key(unlimited, 1), 100).await;
    assert_eq!(take(&repo, key(unlimited, 1), M4).await, SlotTake::Taken);
}
