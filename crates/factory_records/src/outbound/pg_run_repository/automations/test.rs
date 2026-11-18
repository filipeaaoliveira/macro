//! Automations, their firings, and the Runs they start.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::julia;
use crate::domain::automations::{
    AutomationRepository, AutomationStart, AutomationTrigger, FiringClaim, FiringEnd, FiringSkip,
    NewAutomation, RunAutomation, TriggerKind,
};
use crate::domain::models::{NewRun, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::PgRunRepository;

const TEAM: &str = "00000000-0000-7000-8000-000000000201";
const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";

fn id(id: &str) -> Uuid {
    Uuid::parse_str(id).expect("valid id")
}

async fn keep(repo: &PgRunRepository, trigger: &AutomationTrigger, secret: Option<&str>) -> Uuid {
    let automation_id = Uuid::now_v7();
    repo.insert_automation(NewAutomation {
        id: automation_id,
        team_id: id(TEAM),
        created_by: &julia(),
        name: "Triage new tasks",
        trigger,
        flow: "plan",
        repo_url: None,
        request_template: "Plan the task",
        webhook_secret: secret,
    })
    .await
    .expect("insert automation");
    automation_id
}

/// Keep a Run of Julia's, started by `automation`'s firing keyed `key`
/// when one is given.
async fn run_by(
    repo: &PgRunRepository,
    automation: Option<(&RunAutomation, &str)>,
    task_id: Option<Uuid>,
) -> RunInsert {
    repo.insert_run(NewRun {
        id: RunId::generate(),
        flow: "plan",
        request: "Plan the task",
        user_id: &julia(),
        task_id,
        repository: None,
        created_in_person: false,
        control_file_patterns: &[],
        control_files_team_id: None,
        standing_rules: RunRules::DEFAULT,
        automation: automation.map(|(automation, firing_key)| AutomationStart {
            automation,
            firing_key,
        }),
        definition: None,
        kept_flow: None,
    })
    .await
    .expect("insert run")
}

/// An Automation reads back as it was kept, with its trigger's own
/// settings, and is found by the routine that fires it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn an_automation_reads_back_and_is_found_by_its_routine(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let trigger = AutomationTrigger::TaskEvent {
        property_id: Uuid::now_v7(),
        values: vec!["in-progress".to_string()],
    };
    let automation_id = keep(&repo, &trigger, None).await;
    let routine_id = Uuid::now_v7();
    repo.set_automation_routine(automation_id, routine_id)
        .await
        .expect("set routine");

    let read = repo
        .automation(automation_id)
        .await
        .expect("read")
        .expect("the automation exists");
    let by_routine = repo
        .automation_of_routine(routine_id)
        .await
        .expect("read by routine")
        .expect("the routine fires it");

    assert_eq!(read.trigger, trigger);
    assert_eq!(read.created_by, julia());
    assert!(read.enabled);
    assert_eq!(read.routine_id, Some(routine_id));
    assert_eq!(by_routine, read);
    assert_eq!(
        repo.team_automations(id(TEAM), 10)
            .await
            .expect("list")
            .len(),
        1
    );
}

/// Only a webhook Automation has a secret, and a webhook is never fired by
/// a routine.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn only_a_webhook_keeps_a_secret(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let webhook = keep(&repo, &AutomationTrigger::Webhook, Some("whsec_test")).await;

    assert_eq!(
        repo.webhook_secret(webhook)
            .await
            .expect("secret")
            .as_deref(),
        Some("whsec_test")
    );
    let schedule = AutomationTrigger::Schedule {
        cron: "0 0 9 * * Mon-Fri".to_string(),
        timezone: "Europe/Lisbon".to_string(),
    };
    let refused = repo
        .insert_automation(NewAutomation {
            id: Uuid::now_v7(),
            team_id: id(TEAM),
            created_by: &julia(),
            name: "Nightly",
            trigger: &schedule,
            flow: "plan",
            repo_url: None,
            request_template: "Plan",
            webhook_secret: Some("whsec_test"),
        })
        .await;
    assert!(refused.is_err(), "a schedule with a secret is refused");
    assert!(
        repo.set_automation_routine(webhook, Uuid::now_v7())
            .await
            .is_err(),
        "a webhook has no routine"
    );
}

/// A firing's key is claimed once: the same key again reads what the first
/// came to, and while it goes on, that it does.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_firing_key_is_claimed_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let automation_id = keep(&repo, &AutomationTrigger::Webhook, Some("whsec_test")).await;

    let first = repo
        .claim_firing(automation_id, "v1=abc")
        .await
        .expect("claim");
    let again = repo
        .claim_firing(automation_id, "v1=abc")
        .await
        .expect("claim again");
    repo.end_firing(
        automation_id,
        "v1=abc",
        FiringEnd::Skipped(FiringSkip::TaskBusy),
    )
    .await
    .expect("end");
    // The first end is kept.
    repo.end_firing(
        automation_id,
        "v1=abc",
        FiringEnd::Skipped(FiringSkip::Refused),
    )
    .await
    .expect("end again");
    let after = repo
        .claim_firing(automation_id, "v1=abc")
        .await
        .expect("claim after");
    let other = repo
        .claim_firing(automation_id, "v1=def")
        .await
        .expect("claim another");

    assert_eq!(first, FiringClaim::Claimed);
    assert_eq!(again, FiringClaim::InProgress);
    assert_eq!(
        after,
        FiringClaim::Ended(FiringEnd::Skipped(FiringSkip::TaskBusy))
    );
    assert_eq!(other, FiringClaim::Claimed);
}

/// A claim that never ended is taken over once it is stale.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_stale_claim_is_taken_over(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let automation_id = keep(&repo, &AutomationTrigger::Webhook, Some("whsec_test")).await;
    repo.claim_firing(automation_id, "run-1")
        .await
        .expect("claim");
    sqlx::query!("UPDATE factory_automation_firing SET fired_at = now() - interval '29 minutes'")
        .execute(&pool)
        .await
        .expect("age the claim");
    let fresh = repo
        .claim_firing(automation_id, "run-1")
        .await
        .expect("claim again");
    sqlx::query!("UPDATE factory_automation_firing SET fired_at = now() - interval '31 minutes'")
        .execute(&pool)
        .await
        .expect("age the claim");

    let taken = repo
        .claim_firing(automation_id, "run-1")
        .await
        .expect("claim again");

    assert_eq!(fresh, FiringClaim::InProgress);
    assert_eq!(taken, FiringClaim::Claimed);
}

/// A claim taken over after its worker started the Run, short of
/// recording it, finds that Run rather than starting a second.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_stale_claim_finds_the_run_its_firing_started(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let automation_id = keep(&repo, &AutomationTrigger::Webhook, Some("whsec_test")).await;
    let started_by = RunAutomation {
        id: automation_id,
        name: "Triage new tasks".to_string(),
        trigger: TriggerKind::Webhook,
    };
    repo.claim_firing(automation_id, "run-1")
        .await
        .expect("claim");
    let RunInsert::Inserted(run) = run_by(&repo, Some((&started_by, "run-1")), None).await else {
        panic!("the run starts");
    };
    sqlx::query!("UPDATE factory_automation_firing SET fired_at = now() - interval '31 minutes'")
        .execute(&pool)
        .await
        .expect("age the claim");

    let taken = repo
        .claim_firing(automation_id, "run-1")
        .await
        .expect("claim again");

    assert_eq!(taken, FiringClaim::Ended(FiringEnd::Started(run.id)));
}

/// A Run keeps the Automation that started it, and an Automation never
/// starts a second Run on a task while one an Automation started runs
/// there; once it ends, the next may start. A Run a person starts is not
/// held back by the index.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "tasks"))
)]
async fn an_automation_starts_one_run_on_a_task_at_a_time(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let started_by = RunAutomation {
        id: Uuid::now_v7(),
        name: "Triage new tasks".to_string(),
        trigger: TriggerKind::TaskEvent,
    };
    let task = Some(id(LOGIN_TASK));

    let RunInsert::Inserted(first) = run_by(&repo, Some((&started_by, "f1")), task).await else {
        panic!("the first run starts");
    };
    let second = run_by(&repo, Some((&started_by, "f2")), task).await;
    let by_hand = run_by(&repo, None, task).await;
    repo.end_run(first.id, RunOutcome::Success)
        .await
        .expect("end");
    let after = run_by(&repo, Some((&started_by, "f3")), task).await;
    let again = run_by(&repo, Some((&started_by, "f3")), None).await;

    assert_eq!(first.automation, Some(started_by.clone()));
    assert_eq!(
        repo.get_run(first.id)
            .await
            .expect("read")
            .expect("exists")
            .automation,
        Some(started_by)
    );
    assert_eq!(second, RunInsert::TaskBusy);
    assert!(matches!(by_hand, RunInsert::Inserted(run) if run.automation.is_none()));
    assert!(matches!(after, RunInsert::Inserted(_)));
    assert_eq!(again, RunInsert::FiringStarted, "a firing starts one run");
}

/// A running Run on a task is found whoever owns it, and an ended one is
/// not.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "tasks"))
)]
async fn a_running_run_on_a_task_is_found(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let task = id(LOGIN_TASK);

    let before = repo.running_run_on_task(task).await.expect("read");
    let RunInsert::Inserted(run) = run_by(&repo, None, Some(task)).await else {
        panic!("the run starts");
    };
    let running = repo.running_run_on_task(task).await.expect("read");
    repo.end_run(run.id, RunOutcome::Cancelled)
        .await
        .expect("end");
    let ended = repo.running_run_on_task(task).await.expect("read");

    assert_eq!(before, None);
    assert_eq!(running, Some(run.id));
    assert_eq!(ended, None);
}
