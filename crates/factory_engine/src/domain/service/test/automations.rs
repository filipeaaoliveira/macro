//! Automations: making them, and the Runs each kind of trigger starts.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use factory_records::domain::automations::{
    AutomationTrigger, FiringSkip, RunAutomation, TriggerKind,
};
use factory_records::domain::models::{RunOutcome, TeamMembership};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::OrchestratorDefinition;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeExecutor, FakeWorkflows, code_service, julia, service, teo};
use crate::domain::automations::service::AutomationServiceImpl;
use crate::domain::automations::{AutomationRequest, GithubFiring, RoutineFiring};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::{
    AutomationService, AutomationTriggerError, AutomationTriggers, FactoryService,
};
use crate::domain::service::FactoryServiceImpl;

const TEAM: Uuid = Uuid::from_u128(0x7ea);
const TASK: Uuid = Uuid::from_u128(0x7a5c);
const ROUTINE: Uuid = Uuid::from_u128(0x5c4ed);

/// The routines it was asked to make, switch and delete; it refuses every
/// routine when `refusal` is set.
#[derive(Clone, Default)]
struct FakeTriggers {
    created: Arc<Mutex<Vec<(Uuid, TriggerKind)>>>,
    switched: Arc<Mutex<Vec<(Uuid, bool)>>>,
    deleted: Arc<Mutex<Vec<Uuid>>>,
    refusal: Option<&'static str>,
}

impl AutomationTriggers for FakeTriggers {
    async fn create(
        &self,
        automation: &factory_records::domain::automations::Automation,
    ) -> Result<Uuid, AutomationTriggerError> {
        if let Some(reason) = self.refusal {
            return Err(AutomationTriggerError::Refused(reason.to_string()));
        }
        self.created
            .lock()
            .expect("created")
            .push((automation.id, automation.trigger.kind()));
        Ok(ROUTINE)
    }

    async fn set_enabled(
        &self,
        _owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        enabled: bool,
    ) -> Result<(), AutomationTriggerError> {
        self.switched
            .lock()
            .expect("switched")
            .push((routine_id, enabled));
        Ok(())
    }

    async fn delete(
        &self,
        _owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
    ) -> Result<(), AutomationTriggerError> {
        self.deleted.lock().expect("deleted").push(routine_id);
        Ok(())
    }
}

type Runs =
    FactoryServiceImpl<FakeRepo, FakeWorkflows, FakeExecutor, FakeRepo, FakeRepo, FakeGithub>;

/// A repository where Julia owns the team and Teo is a member, and the task
/// exists.
fn team_repo() -> FakeRepo {
    let repo = FakeRepo::default();
    let mut teams = repo.teams.lock().expect("teams");
    teams.insert(
        julia().to_string(),
        TeamMembership {
            team_id: TEAM,
            admin: true,
        },
    );
    teams.insert(
        teo().to_string(),
        TeamMembership {
            team_id: TEAM,
            admin: false,
        },
    );
    drop(teams);
    repo.tasks.lock().expect("tasks").insert(TASK);
    repo
}

fn automations(
    repo: &FakeRepo,
    runs: Runs,
    triggers: &FakeTriggers,
) -> AutomationServiceImpl<FakeRepo, Runs, FakeTriggers> {
    AutomationServiceImpl::new(repo.clone(), Arc::new(runs), triggers.clone())
}

fn reply_automation(trigger: AutomationTrigger) -> AutomationRequest {
    AutomationRequest {
        name: "Answer it".to_string(),
        trigger,
        flow: "reply".to_string(),
        repo_url: None,
        request_template: "Reply to the task".to_string(),
    }
}

fn status_in_progress() -> AutomationTrigger {
    AutomationTrigger::TaskEvent {
        property_id: Uuid::from_u128(0x57a7),
        values: vec!["in-progress".to_string()],
    }
}

fn routine_firing(key: &str, task_id: Option<Uuid>) -> RoutineFiring {
    RoutineFiring {
        firing: key.to_string(),
        task_id,
        github: None,
    }
}

/// A schedule's Automation is a routine of its creator's; each run of it
/// starts one Run as the creator, outside their signed-in session, that
/// keeps which Automation started it. A retried routine run starts
/// nothing more.
#[tokio::test]
async fn a_schedule_starts_a_run_each_time_its_routine_runs() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);

    let created = automations
        .create_automation(
            julia(),
            true,
            reply_automation(AutomationTrigger::Schedule {
                cron: "0 0 9 * * Mon-Fri".to_string(),
                timezone: "Europe/Lisbon".to_string(),
            }),
        )
        .await
        .expect("created");
    let fired = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", None))
        .await
        .expect("fired");
    let retried = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", None))
        .await
        .expect("fired again");

    assert_eq!(created.automation.routine_id, Some(ROUTINE));
    assert_eq!(created.webhook_secret, None);
    assert_eq!(
        *triggers.created.lock().expect("created"),
        vec![(created.automation.id, TriggerKind::Schedule)]
    );
    let run_id = fired.run_id.expect("a run started");
    assert!(!fired.repeated);
    assert_eq!(retried.run_id, Some(run_id));
    assert!(retried.repeated);
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.user_id, julia());
    assert!(!run.created_in_person);
    assert_eq!(
        run.automation,
        Some(RunAutomation {
            id: created.automation.id,
            name: "Answer it".to_string(),
            trigger: TriggerKind::Schedule,
        })
    );
    assert!(
        run.request
            .starts_with("Reply to the task\n\nStarted by the automation")
    );
}

/// A Task event starts a Run on its task, and never a second one while a
/// Run runs there, whoever started it; once it ends, the next event starts
/// one again.
#[tokio::test]
async fn a_task_event_starts_one_run_on_its_task_at_a_time() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");

    let first = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await
        .expect("fired");
    let second = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-2", Some(TASK)))
        .await
        .expect("fired");
    let run_id = first.run_id.expect("a run started");
    repo.end_run(run_id, RunOutcome::Success)
        .await
        .expect("ended");
    let third = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-3", Some(TASK)))
        .await
        .expect("fired");

    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.task_id, Some(TASK));
    assert_eq!(
        run.automation.map(|automation| automation.trigger),
        Some(TriggerKind::TaskEvent)
    );
    assert_eq!(second.run_id, None);
    assert_eq!(second.skipped, Some(FiringSkip::TaskBusy));
    assert!(third.run_id.is_some());
    assert_eq!(workflows.started.lock().expect("started").len(), 2);
}

/// A repository event's Automation needs a Flow that works on its
/// repository, which its creator reaches; its Run works there and its
/// request names the GitHub event.
#[tokio::test]
async fn a_repository_event_starts_a_run_on_its_repository() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = FakeGithub::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, code_service(&repo, &workflows, &github), &triggers);
    let request = AutomationRequest {
        name: "Review new pull requests".to_string(),
        trigger: AutomationTrigger::RepositoryEvent {
            events: vec!["pull_request".to_string()],
            actions: vec!["opened".to_string()],
        },
        flow: "code".to_string(),
        repo_url: Some("https://github.com/acme/widgets".to_string()),
        request_template: "Check the new pull request".to_string(),
    };

    let created = automations
        .create_automation(julia(), true, request.clone())
        .await
        .expect("created");
    let fired = automations
        .fire_from_routine(
            &julia(),
            ROUTINE,
            RoutineFiring {
                firing: "run-1".to_string(),
                task_id: None,
                github: Some(GithubFiring {
                    event: "pull_request".to_string(),
                    action: Some("opened".to_string()),
                    branch: Some("fix-login".to_string()),
                }),
            },
        )
        .await
        .expect("fired");
    // The Run's own pull request, from its branch, starts nothing.
    let own = automations
        .fire_from_routine(
            &julia(),
            ROUTINE,
            RoutineFiring {
                firing: "run-2".to_string(),
                task_id: None,
                github: Some(GithubFiring {
                    event: "pull_request".to_string(),
                    action: Some("opened".to_string()),
                    branch: Some(format!("factory/{}", fired.run_id.expect("started"))),
                }),
            },
        )
        .await
        .expect("fired");
    github.state().unreachable = true;
    let unreachable = automations.create_automation(julia(), true, request).await;

    assert_eq!(
        created.automation.trigger.kind(),
        TriggerKind::RepositoryEvent
    );
    let run = repo
        .get_run(fired.run_id.expect("a run started"))
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(
        run.repository.map(|repository| repository.url),
        Some("https://github.com/acme/widgets".to_string())
    );
    assert!(
        run.request
            .ends_with("on the GitHub event pull_request opened on acme/widgets.")
    );
    assert!(matches!(
        unreachable,
        Err(FactoryError::UnreachableRepository)
    ));
    assert_eq!(own.skipped, Some(FiringSkip::OwnBranch));
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
}

/// A webhook's call must be signed with the secret shown once when it was
/// made, at a timestamp near now; the same call again is a replay and
/// starts nothing.
#[tokio::test]
async fn a_signed_webhook_call_starts_a_run_once() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    let created = automations
        .create_automation(julia(), true, reply_automation(AutomationTrigger::Webhook))
        .await
        .expect("created");
    let id = created.automation.id;
    let secret = created.webhook_secret.expect("a webhook has a secret");
    let body = format!(r#"{{"taskId":"{TASK}"}}"#);
    let now = Utc::now().timestamp().to_string();
    let signature = webhook_signature::sign(&secret, &now, body.as_bytes()).expect("signed");

    let fired = automations
        .fire_webhook(id, &now, &signature, body.as_bytes())
        .await
        .expect("fired");
    let replayed = automations
        .fire_webhook(id, &now, &signature, body.as_bytes())
        .await;
    // The same call with its digest in upper case verifies too: it is the
    // same firing.
    let shouted = format!("v1={}", signature["v1=".len()..].to_uppercase());
    let replayed_shouting = automations
        .fire_webhook(id, &now, &shouted, body.as_bytes())
        .await;
    let unknown = automations
        .fire_webhook(Uuid::now_v7(), &now, &signature, body.as_bytes())
        .await;
    let forged = automations
        .fire_webhook(
            id,
            &now,
            &webhook_signature::sign("whsec_guess", &now, body.as_bytes()).expect("signed"),
            body.as_bytes(),
        )
        .await;
    let old = (Utc::now().timestamp() - 600).to_string();
    let stale = automations
        .fire_webhook(
            id,
            &old,
            &webhook_signature::sign(&secret, &old, body.as_bytes()).expect("signed"),
            body.as_bytes(),
        )
        .await;

    assert!(triggers.created.lock().expect("created").is_empty());
    assert_eq!(created.automation.routine_id, None);
    let run = repo
        .get_run(fired.run_id.expect("a run started"))
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(run.task_id, Some(TASK));
    assert!(matches!(replayed, Err(FactoryError::FiringRepeated)));
    assert!(matches!(
        replayed_shouting,
        Err(FactoryError::FiringRepeated)
    ));
    assert!(matches!(unknown, Err(FactoryError::BadSignature)));
    assert!(matches!(forged, Err(FactoryError::BadSignature)));
    assert!(matches!(stale, Err(FactoryError::BadSignature)));
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
}

/// Only its creator's routine fires an Automation, and only a person in
/// their own session, on a team, makes one.
#[tokio::test]
async fn only_its_creator_makes_and_fires_an_automation() {
    let repo = team_repo();
    repo.agent_accounts
        .lock()
        .expect("agents")
        .insert(teo().to_string());
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    let loner = MacroUserIdStr::try_from("macro|loner@example.com".to_string()).expect("valid");

    let by_program = automations
        .create_automation(julia(), false, reply_automation(AutomationTrigger::Webhook))
        .await;
    let by_agent = automations
        .create_automation(teo(), true, reply_automation(AutomationTrigger::Webhook))
        .await;
    let by_loner = automations
        .create_automation(loner, true, reply_automation(AutomationTrigger::Webhook))
        .await;
    automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");
    let by_someone_else = automations
        .fire_from_routine(&teo(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await;

    assert!(matches!(by_program, Err(FactoryError::NotAPerson)));
    assert!(matches!(by_agent, Err(FactoryError::NotAPerson)));
    assert!(matches!(by_loner, Err(FactoryError::NoTeam)));
    assert!(matches!(by_someone_else, Err(FactoryError::NotFound)));
    assert!(workflows.started.lock().expect("started").is_empty());
}

/// An Automation switched off starts nothing; only its creator or a team
/// admin switches it, and its routine follows. Deleting it deletes its
/// routine.
#[tokio::test]
async fn an_automation_switched_off_starts_nothing() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    let created = automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");
    let id = created.automation.id;

    let by_member = automations
        .set_automation_enabled(&teo(), true, id, false)
        .await;
    automations
        .set_automation_enabled(&julia(), true, id, false)
        .await
        .expect("switched off");
    let fired = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await
        .expect("fired");
    automations
        .delete_automation(&julia(), id)
        .await
        .expect("deleted");

    assert!(matches!(
        by_member,
        Err(FactoryError::NotTheAutomationOwner)
    ));
    assert_eq!(
        *triggers.switched.lock().expect("switched"),
        vec![(ROUTINE, false)]
    );
    assert_eq!(fired.skipped, Some(FiringSkip::Disabled));
    assert!(workflows.started.lock().expect("started").is_empty());
    assert_eq!(*triggers.deleted.lock().expect("deleted"), vec![ROUTINE]);
    assert!(
        automations
            .automations(&julia())
            .await
            .expect("listed")
            .automations
            .is_empty()
    );
}

/// A full Queue holds a firing back, and says so: the creator's Queue
/// limit, the one their own starts meet, counting the places their Runs
/// hold, the Run an earlier firing just started among them.
#[tokio::test]
async fn a_full_queue_starts_no_run() {
    let repo = team_repo();
    repo.team_rules.lock().expect("rules").insert(
        TEAM,
        OrchestratorDefinition {
            queue_limit: Some(1),
            ..OrchestratorDefinition::DEFAULT
        },
    );
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let github = FakeGithub::default();
    let automations = automations(&repo, code_service(&repo, &workflows, &github), &triggers);
    let own = code_service(&repo, &workflows, &github);
    let own_start = || {
        own.create_run(
            julia(),
            "plan".to_string(),
            "Plan it".to_string(),
            None,
            None,
            false,
        )
    };
    automations
        .create_automation(
            julia(),
            true,
            AutomationRequest {
                flow: "plan".to_string(),
                ..reply_automation(AutomationTrigger::Schedule {
                    cron: "0 0 9 * * Mon-Fri".to_string(),
                    timezone: "Europe/Lisbon".to_string(),
                })
            },
        )
        .await
        .expect("created");

    own_start().await.expect("her own run holds one place");
    let fired = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", None))
        .await
        .expect("fired");
    assert!(fired.run_id.is_some(), "at the limit, not above: {fired:?}");

    let held = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-2", None))
        .await
        .expect("fired");
    assert_eq!(held.skipped, Some(FiringSkip::QueueFull));
    assert_eq!(held.run_id, None);
    assert_eq!(workflows.started.lock().expect("started").len(), 2);
    let refused = own_start().await;
    assert!(
        matches!(refused, Err(FactoryError::QueueFull { limit: 1, .. })),
        "the firing's run holds its place against her own starts too: {refused:?}"
    );
}

/// An Automation acts as its creator, so a task they cannot see, someone
/// else's, gets no Run, and so no thread posted in their name: the firing
/// is refused as the Run API would refuse them, and says nothing of the
/// task, not even that a Run runs there.
#[tokio::test]
async fn a_task_its_creator_cannot_see_gets_no_run() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");
    let busy = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await
        .expect("fired");
    assert!(busy.run_id.is_some(), "{busy:?}");

    repo.hidden
        .lock()
        .expect("hidden")
        .insert((julia().to_string(), TASK));
    let fired = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-2", Some(TASK)))
        .await
        .expect("fired");

    assert_eq!(fired.skipped, Some(FiringSkip::Refused));
    assert_eq!(fired.run_id, None);
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
    assert_eq!(repo.runs.lock().expect("runs").len(), 1);
}

/// A routine the scheduler refuses takes its Automation with it, and the
/// refusal says why.
#[tokio::test]
async fn a_refused_routine_keeps_no_automation() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers {
        refusal: Some("event-trigger management is not enabled"),
        ..FakeTriggers::default()
    };
    let automations = automations(&repo, service(&repo, &workflows), &triggers);

    let refused = automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await;

    assert!(matches!(
        refused,
        Err(FactoryError::TriggerRefused(reason)) if reason == "event-trigger management is not enabled"
    ));
    assert!(repo.automations.lock().expect("automations").is_empty());
}

/// A team admin may stop a member's Automation, but only its creator, in
/// person, switches it on again: switching one on has it start Runs as
/// them.
#[tokio::test]
async fn only_its_creator_switches_an_automation_on() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    let id = automations
        .create_automation(teo(), true, reply_automation(status_in_progress()))
        .await
        .expect("created")
        .automation
        .id;

    automations
        .set_automation_enabled(&julia(), true, id, false)
        .await
        .expect("an admin stops it");
    let admin_starts = automations
        .set_automation_enabled(&julia(), true, id, true)
        .await;
    let program_starts = automations
        .set_automation_enabled(&teo(), false, id, true)
        .await;
    let creator_starts = automations
        .set_automation_enabled(&teo(), true, id, true)
        .await
        .expect("its creator starts it");

    assert!(matches!(
        admin_starts,
        Err(FactoryError::NotTheAutomationOwner)
    ));
    assert!(matches!(program_starts, Err(FactoryError::NotAPerson)));
    assert!(creator_starts.enabled);
}

/// An Automation acts as its creator only while they are a person on its
/// team: one whose creator left, or whose account is an agent's, starts
/// nothing and says why.
#[tokio::test]
async fn an_automation_whose_creator_left_starts_nothing() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    automations
        .create_automation(teo(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");

    repo.agent_accounts
        .lock()
        .expect("agents")
        .insert(teo().to_string());
    let as_agent = automations
        .fire_from_routine(&teo(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await
        .expect("fired");
    repo.agent_accounts.lock().expect("agents").clear();
    repo.teams.lock().expect("teams").remove(&teo().to_string());
    let after_leaving = automations
        .fire_from_routine(&teo(), ROUTINE, routine_firing("run-2", Some(TASK)))
        .await
        .expect("fired");

    assert_eq!(as_agent.skipped, Some(FiringSkip::AgentAccount));
    assert_eq!(after_leaving.skipped, Some(FiringSkip::CreatorLeftTeam));
    assert!(workflows.started.lock().expect("started").is_empty());
}

/// A routine whose Automation is gone, as with its team, is switched off
/// when it next fires, and starts nothing.
#[tokio::test]
async fn the_routine_of_a_deleted_automation_is_switched_off() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let triggers = FakeTriggers::default();
    let automations = automations(&repo, service(&repo, &workflows), &triggers);
    automations
        .create_automation(julia(), true, reply_automation(status_in_progress()))
        .await
        .expect("created");
    repo.automations.lock().expect("automations").clear();

    let fired = automations
        .fire_from_routine(&julia(), ROUTINE, routine_firing("run-1", Some(TASK)))
        .await;

    assert!(matches!(fired, Err(FactoryError::NotFound)));
    assert_eq!(
        *triggers.switched.lock().expect("switched"),
        vec![(ROUTINE, false)]
    );
    assert!(workflows.started.lock().expect("started").is_empty());
}
