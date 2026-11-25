//! A Step is placed on a Machine before its session opens: the first Agent
//! of its Role whose Machine has the tags the Agent asks for and a free slot
//! takes it. While each such Machine is full the Step waits, and says on
//! which; when no Machine has the tags, it fails at once.

use agent_session::domain::routines::RoutineActionStatus;
use bot_id::BotId;
use chrono::Utc;
use factory_records::domain::machines::{MachineLogin, MachineRepository, SlotTake, StepMachine};
use factory_records::domain::models::{EntryId, Outcome, Role, StepKey};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{
    BACKUP, FakeSessions, PRIMARY, SPARE, agent, agents, name, no_quota, plan_task, recorded,
    the_plan,
};
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::StepName;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::session_agent::placement::TAKE_SLOT_WITHIN;
use crate::domain::session_agent::{SessionAgent, SessionAgentError};

const M4: Uuid = Uuid::from_u128(0x0a4);
const M3: Uuid = Uuid::from_u128(0x0a3);

/// Bind `bot` to the Machine `id`, named `name`, with `slots` and `tags`.
fn bind(repo: &FakeRepo, bot: BotId, id: Uuid, name: &str, slots: Option<u16>, tags: &[&str]) {
    repo.machines.lock().expect("machines").insert(
        bot.as_uuid(),
        StepMachine {
            id,
            name: name.to_string(),
            step_slots: slots,
            tags: tags.iter().map(ToString::to_string).collect(),
            login: MachineLogin::Owner("macro|julia@example.com".to_string()),
        },
    );
}

/// The Definition a Run kept: a planner is `primary`, which asks for a
/// `GPU` Machine, then `backup`, which asks for `gpu` too; a writer is
/// `spare`, which asks for `rust-build`.
fn kept() -> serde_json::Value {
    serde_json::json!({
        "agents": {
            "primary": { "bot": PRIMARY.as_uuid(), "machine_tags": ["GPU"] },
            "backup": { "bot": BACKUP.as_uuid(), "machine_tags": ["gpu"] },
            "spare": { "bot": SPARE.as_uuid(), "machine_tags": ["rust-build"] }
        },
        "roles": { "planner": ["primary", "backup"], "writer": ["spare"] }
    })
}

/// The bots of the sessions the Steps prepared, in order.
fn prepared_bots(sessions: &FakeSessions) -> Vec<BotId> {
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    prepared.iter().map(|call| call.selection.bot_id).collect()
}

/// Two Machines with different tags take Steps from one Run, each where its
/// tags allow: the planner's first Agent is passed over, as its Machine
/// `m4` has no `gpu` tag, and `backup` plans on `m3`; the writer writes on
/// `m4`, which has `rust-build`. Each Step holds a slot on its Machine.
#[tokio::test]
async fn each_step_runs_on_a_machine_with_the_tags_its_agent_asks_for() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M4, "m4", Some(2), &["rust-build"]);
    bind(&repo, BACKUP, M3, "m3", Some(1), &["gpu"]);
    bind(&repo, SPARE, M4, "m4", Some(2), &["rust-build"]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let plan = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(plan.key.run_id, kept());
    let mut write = plan.clone();
    write.key = StepKey {
        run_id: plan.key.run_id,
        seq: 2,
    };
    write.step = StepName::try_from("write".to_string()).expect("valid");
    write.role = Role::try_from("writer".to_string()).expect("valid");
    let (plan_key, write_key) = (plan.key, write.key);

    agent(&sessions, &repo).answer(plan).await.expect("plan");
    repo.start_step(
        write_key,
        &EntryId::try_from("write".to_string()).expect("valid"),
        1,
    )
    .await
    .expect("start");
    // Each Step's session is a fresh one of its own.
    let writing = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    agent(&writing, &repo).answer(write).await.expect("write");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    assert_eq!(prepared_bots(&writing), [SPARE]);
    assert_eq!(recorded(&repo, plan_key).1, Some(name("backup")));
    assert_eq!(recorded(&repo, write_key).1, Some(name("spare")));
    let slots = repo.machine_slots.lock().expect("slots").clone();
    assert_eq!(slots.get(&plan_key), Some(&M3));
    assert_eq!(slots.get(&write_key), Some(&M4));
}

/// A Step whose every Machine is full waits, saying on which, and fails for
/// a retry once its attempt's time is up, as infrastructure's to wait on,
/// with no session opened; once a Step ends and frees a slot, its retry
/// takes it.
#[tokio::test(start_paused = true)]
async fn a_full_machine_makes_a_step_wait_for_a_slot() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M4, "m4", Some(1), &[]);
    bind(&repo, BACKUP, M3, "m3", Some(1), &[]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let busy_m4 = plan_task(&repo).await.key;
    let busy_m3 = plan_task(&repo).await.key;
    for (busy, machine) in [(busy_m4, M4), (busy_m3, M3)] {
        let taken = repo
            .take_machine_slot(busy, machine, TAKE_SLOT_WITHIN)
            .await
            .expect("take");
        assert_eq!(taken, SlotTake::Taken);
    }
    let mut task = plan_task(&repo).await;
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let key = task.key;

    let waited = agent(&sessions, &repo).answer(task.clone()).await;

    let Err(error) = waited else {
        panic!("a step with every machine full started");
    };
    assert!(
        matches!(&error, SessionAgentError::WaitingForSlot(full) if full == &["m4", "m3"]),
        "{error:?}"
    );
    assert!(error.retryable());
    assert!(error.waits_for_machine());
    assert!(prepared_bots(&sessions).is_empty());
    assert_eq!(
        repo.slot_waits.lock().expect("waits").get(&key),
        Some(&vec!["m4".to_string(), "m3".to_string()])
    );

    repo.end_step(busy_m3, &Outcome::done(), "Planned")
        .await
        .expect("end");
    task.answer_by = None;
    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&key),
        Some(&M3)
    );
    assert_eq!(repo.slot_waits.lock().expect("waits").get(&key), None);
}

/// A Step whose Agents ask for tags no Machine they run on has fails at
/// once, never retried, saying which tags, as a refusal its owner reads.
#[tokio::test]
async fn a_step_no_machine_has_the_tags_for_fails_saying_why() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M4, "m4", None, &["rust-build"]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(task.key.run_id, kept());

    let error = agent(&sessions, &repo)
        .answer(task)
        .await
        .expect_err("no machine has the tags");

    assert!(!error.retryable());
    assert!(
        error.refused().is_some_and(|why| why.contains("(gpu)")),
        "{error:?}"
    );
    assert!(prepared_bots(&sessions).is_empty());
}

const M5: Uuid = Uuid::from_u128(0x0a5);

/// A Definition whose planners all ask for a `gpu` Machine: `primary`, then
/// `backup`, then `spare`.
fn gpu_planners() -> serde_json::Value {
    serde_json::json!({
        "agents": {
            "primary": { "bot": PRIMARY.as_uuid(), "machine_tags": ["gpu"] },
            "backup": { "bot": BACKUP.as_uuid(), "machine_tags": ["gpu"] },
            "spare": { "bot": SPARE.as_uuid(), "machine_tags": ["gpu"] }
        },
        "roles": { "planner": ["primary", "backup", "spare"] }
    })
}

/// When the Agent a Step was placed on cannot start for lack of quota, the
/// Step passes over the next Agent, whose Machine lacks the tag, to the one
/// after it, and its slot moves to that Agent's Machine.
#[tokio::test]
async fn a_pass_over_skips_an_agent_whose_machine_lacks_the_tags() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M3, "m3", Some(1), &["gpu"]);
    bind(&repo, BACKUP, M4, "m4", Some(1), &["rust-build"]);
    bind(&repo, SPARE, M5, "m5", Some(1), &["gpu"]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(task.key.run_id, gpu_planners());
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [PRIMARY, SPARE]);
    assert_eq!(recorded(&repo, key).1, Some(name("spare")));
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&key),
        Some(&M5)
    );
}

/// An Agent its owner's quota refused lately is passed over by placement as
/// its start would pass it over: the Step's slot is booked on the Agent
/// that opens its session, never on the one skipped.
#[tokio::test]
async fn placement_books_no_slot_on_an_agent_the_quota_refused_lately() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M3, "m3", Some(1), &["gpu"]);
    bind(&repo, BACKUP, M5, "m5", Some(2), &["gpu"]);
    let refused = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let planner = agent(&refused, &repo);
    let first = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(first.key.run_id, gpu_planners());
    planner.answer(first).await.expect("first");

    let next = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(next.key.run_id, gpu_planners());
    let key = next.key;
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let planner = SessionAgent {
        sessions: sessions.clone(),
        repo: repo.clone(),
        agents: agents(),
        quota: planner.quota.clone(),
        pause_notices: None,
        key_plans: planner.key_plans.clone(),
    };
    planner.answer(next).await.expect("next");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    assert_eq!(recorded(&repo, key).1, Some(name("backup")));
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&key),
        Some(&M5)
    );
}

/// A Step that started too long ago to take a slot, with every Machine
/// full, fails for good as one whose Machines stayed full.
#[tokio::test]
async fn a_step_too_late_for_a_slot_fails_for_good() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M4, "m4", Some(1), &[]);
    bind(&repo, BACKUP, M4, "m4", Some(1), &[]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let busy = plan_task(&repo).await.key;
    let taken = repo
        .take_machine_slot(busy, M4, TAKE_SLOT_WITHIN)
        .await
        .expect("take");
    assert_eq!(taken, SlotTake::Taken);
    let task = plan_task(&repo).await;
    let key = task.key;
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .for_each(|(_, step)| step.started_at = Utc::now() - chrono::Duration::minutes(100));

    let error = agent(&sessions, &repo)
        .answer(task)
        .await
        .expect_err("too late for a slot");

    assert!(
        matches!(&error, SessionAgentError::SlotTimeRanOut(full) if full == &["m4"]),
        "{error:?}"
    );
    assert!(!error.retryable());
    assert!(error.waits_for_machine());
    assert!(prepared_bots(&sessions).is_empty());
}

/// A Step past the time to wait for a slot still takes a free one, on a
/// Machine with a slot left as on one with no limit: only a full Machine is
/// too late.
#[tokio::test]
async fn a_late_step_takes_a_free_slot() {
    for slots in [Some(2), None] {
        let repo = FakeRepo::default();
        bind(&repo, PRIMARY, M4, "m4", slots, &[]);
        let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
        let busy = plan_task(&repo).await.key;
        let taken = repo
            .take_machine_slot(busy, M4, TAKE_SLOT_WITHIN)
            .await
            .expect("take");
        assert_eq!(taken, SlotTake::Taken);
        let task = plan_task(&repo).await;
        let key = task.key;
        repo.steps
            .lock()
            .expect("steps")
            .iter_mut()
            .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
            .for_each(|(_, step)| step.started_at = Utc::now() - chrono::Duration::minutes(100));

        agent(&sessions, &repo).answer(task).await.expect("answer");

        assert_eq!(prepared_bots(&sessions), [PRIMARY]);
        assert_eq!(
            repo.machine_slots.lock().expect("slots").get(&key),
            Some(&M4)
        );
    }
}

/// A Step whose Agent cannot start, passed on to an Agent whose Machine is
/// full, waits there for a slot having let go of the one it held: two Steps
/// moving between two full Machines never hold each other's way.
#[tokio::test(start_paused = true)]
async fn a_moving_step_lets_go_of_its_slot_while_it_waits_for_the_next() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M3, "m3", Some(1), &[]);
    bind(&repo, BACKUP, M4, "m4", Some(1), &[]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let busy = plan_task(&repo).await.key;
    let taken = repo
        .take_machine_slot(busy, M4, TAKE_SLOT_WITHIN)
        .await
        .expect("take");
    assert_eq!(taken, SlotTake::Taken);
    let mut task = plan_task(&repo).await;
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let key = task.key;

    let error = agent(&sessions, &repo)
        .answer(task)
        .await
        .expect_err("m4 stays full");

    assert!(
        matches!(&error, SessionAgentError::WaitingForSlot(full) if full == &["m4"]),
        "{error:?}"
    );
    assert_eq!(prepared_bots(&sessions), [PRIMARY]);
    assert_eq!(repo.machine_slots.lock().expect("slots").get(&key), None);
    assert_eq!(
        repo.slot_waits.lock().expect("waits").get(&key),
        Some(&vec!["m4".to_string()])
    );
}

/// A retried Step whose session already opened takes no slot: its session
/// stays where it opened, whatever its Machine's slots say now, as for a
/// Step running from before Machines had slots.
#[tokio::test(start_paused = true)]
async fn a_retry_of_a_step_whose_session_opened_takes_no_slot() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let mut task = plan_task(&repo).await;
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let key = task.key;
    let planner = agent(&sessions, &repo);
    planner.answer(task.clone()).await.expect("first");

    bind(&repo, PRIMARY, M4, "m4", Some(1), &["gpu"]);
    let busy = plan_task(&repo).await.key;
    let taken = repo
        .take_machine_slot(busy, M4, TAKE_SLOT_WITHIN)
        .await
        .expect("take");
    assert_eq!(taken, SlotTake::Taken);
    planner.answer(task).await.expect("the retry");

    assert_eq!(repo.machine_slots.lock().expect("slots").get(&key), None);
    assert_eq!(repo.slot_waits.lock().expect("waits").get(&key), None);
}

/// A first turn its usage limit ended passes the Step on to the next Agent
/// whose Machine has the tags, past one whose Machine lacks them.
#[tokio::test(start_paused = true)]
async fn a_usage_limit_passes_the_step_to_an_agent_with_the_tags() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M3, "m3", Some(1), &["gpu"]);
    bind(&repo, BACKUP, M4, "m4", Some(1), &["rust-build"]);
    bind(&repo, SPARE, M5, "m5", Some(1), &["gpu"]);
    let sessions =
        FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded).limiting(1);
    let task = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(task.key.run_id, gpu_planners());
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [PRIMARY, SPARE]);
    assert_eq!(recorded(&repo, key).1, Some(name("spare")));
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&key),
        Some(&M5)
    );
}

/// A late Step that finds its first Agent's Machine full goes on to the
/// next Agent whose Machine has a slot free, rather than stop at the first.
#[tokio::test]
async fn a_late_step_passes_a_full_machine_for_a_free_one() {
    let repo = FakeRepo::default();
    bind(&repo, PRIMARY, M4, "m4", Some(1), &[]);
    bind(&repo, BACKUP, M3, "m3", Some(1), &[]);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let busy = plan_task(&repo).await.key;
    let taken = repo
        .take_machine_slot(busy, M4, TAKE_SLOT_WITHIN)
        .await
        .expect("take");
    assert_eq!(taken, SlotTake::Taken);
    let task = plan_task(&repo).await;
    let key = task.key;
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .for_each(|(_, step)| step.started_at = Utc::now() - chrono::Duration::minutes(100));

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&key),
        Some(&M3)
    );
}
