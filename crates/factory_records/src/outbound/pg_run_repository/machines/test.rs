//! Where the factory places agent Steps: the Machine an agent's bot is bound
//! to, and the slots its Steps hold on it.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::{entry, insert, key, outcome};
use crate::domain::machines::{MachineRepository, StepMachine};
use crate::domain::models::RunId;
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

/// Julia's agent bound to her Machine `m4` (`fixtures/agents.sql`).
const WRITER: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b1);
/// Julia's agent on an in-memory runtime, bound to no Machine.
const CLOUD: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b2);
/// Her Machine `m4`.
const M4: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00a1);

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

    assert!(
        repo.take_machine_slot(key(first, 1), M4)
            .await
            .expect("take")
    );
    assert!(
        repo.take_machine_slot(key(first, 1), M4)
            .await
            .expect("retake"),
        "a retried attempt keeps its slot"
    );
    assert!(
        !repo
            .take_machine_slot(key(second, 1), M4)
            .await
            .expect("take")
    );

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
    assert!(
        repo.take_machine_slot(key(second, 1), M4)
            .await
            .expect("take")
    );
    let placed = repo.list_steps(second).await.expect("steps")[0]
        .runtime
        .clone();
    assert_eq!(placed.waiting_for_slot_since, None);
    assert!(placed.waiting_on_machines.is_empty());

    set_m4(&pool, None).await;
    let third = started(&repo).await;
    assert!(
        repo.take_machine_slot(key(third, 1), M4)
            .await
            .expect("take")
    );
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
    assert!(
        repo.take_machine_slot(key(elsewhere, 1), m3)
            .await
            .expect("take")
    );
    let _unplaced = started(&repo).await;

    let ended = started(&repo).await;
    assert!(
        repo.take_machine_slot(key(ended, 1), M4)
            .await
            .expect("take")
    );
    sqlx::query!(
        "UPDATE factory_run SET outcome = 'cancelled', ended_at = now() WHERE id = $1",
        ended.as_uuid()
    )
    .execute(&pool)
    .await
    .expect("end the run");

    let next = started(&repo).await;
    assert!(
        repo.take_machine_slot(key(next, 1), M4)
            .await
            .expect("take")
    );
}
