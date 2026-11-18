//! The routines that fire Automations, against a stand-in scheduler.

use chrono::{TimeZone, Utc};
use factory_records::domain::automations::{Automation, AutomationTrigger};
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{FIRE_STEP, RoutineAutomationTriggers};
use crate::domain::ports::{AutomationTriggerError, AutomationTriggers};

const OWNER: &str = "macro|julia@example.com";
const ROUTINE: Uuid = Uuid::from_u128(0x5c4ed);
const STATUS: Uuid = Uuid::from_u128(0x57a7);

fn automation(trigger: AutomationTrigger, repo_url: Option<&str>) -> Automation {
    Automation {
        id: Uuid::from_u128(0xa7),
        team_id: Uuid::from_u128(0x7ea),
        created_by: MacroUserIdStr::try_from(OWNER.to_string()).expect("valid"),
        name: "Fix what lands in review".to_string(),
        trigger,
        flow: "code".to_string(),
        repo_url: repo_url.map(str::to_string),
        request_template: "Fix it".to_string(),
        enabled: true,
        routine_id: None,
        created_at: Utc.with_ymd_and_hms(2026, 11, 18, 9, 0, 0).unwrap(),
    }
}

/// The routine the stand-in scheduler was asked to save.
async fn saved_routine(trigger: AutomationTrigger, repo_url: Option<&str>) -> Value {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/scheduled-actions"))
        .and(header("x-internal-auth-key", "key"))
        .and(header("x-internal-macro-user-id", OWNER))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": ROUTINE })))
        .expect(1)
        .mount(&server)
        .await;
    let triggers = RoutineAutomationTriggers::new(&server.uri(), "key").expect("client");

    let routine_id = triggers
        .create(&automation(trigger, repo_url))
        .await
        .expect("created");

    assert_eq!(routine_id, ROUTINE);
    let requests = server.received_requests().await.expect("requests");
    serde_json::from_slice(&requests[0].body).expect("json body")
}

/// A schedule is the scheduler's cron trigger, and the routine's one code
/// step fires the Automation.
#[tokio::test]
async fn a_schedule_is_a_cron_routine_that_fires_the_automation() {
    let routine = saved_routine(
        AutomationTrigger::Schedule {
            cron: "0 0 9 * * Mon-Fri".to_string(),
            timezone: "Europe/Lisbon".to_string(),
        },
        None,
    )
    .await;

    assert_eq!(
        routine["trigger"],
        json!({ "type": "cron", "schedule": "0 0 9 * * Mon-Fri", "timezone": "Europe/Lisbon" })
    );
    assert_eq!(routine["enabled"], json!(true));
    assert_eq!(routine["kind"], json!("Agent"));
    let steps = routine["task"]["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["type"], json!("code"));
    assert_eq!(steps[0]["source"], json!(FIRE_STEP));
}

/// A repository event is a GitHub event filter on the Automation's
/// repository only.
#[tokio::test]
async fn a_repository_event_watches_the_automation_s_repository() {
    let routine = saved_routine(
        AutomationTrigger::RepositoryEvent {
            events: vec!["pull_request".to_string()],
            actions: vec!["opened".to_string()],
        },
        Some("https://github.com/acme/widgets"),
    )
    .await;

    assert_eq!(
        routine["trigger"],
        json!({
            "type": "events",
            "filters": [{
                "events": ["github"],
                "github": {
                    "events": ["pull_request"],
                    "actions": ["opened"],
                    "repositories": ["acme/widgets"],
                },
            }],
        })
    );
}

/// A Task event is a task property change filter on its property and the
/// values it must gain.
#[tokio::test]
async fn a_task_event_watches_the_property_change() {
    let routine = saved_routine(
        AutomationTrigger::TaskEvent {
            property_id: STATUS,
            values: vec!["in-progress".to_string()],
        },
        None,
    )
    .await;

    assert_eq!(
        routine["trigger"],
        json!({
            "type": "events",
            "filters": [{
                "events": ["task.property_changed"],
                "property": { "definitionId": STATUS, "values": ["in-progress"] },
            }],
        })
    );
}

/// The scheduler's refusal comes back with its reason; its failure as an
/// outage.
#[tokio::test]
async fn a_refusal_says_why_and_a_failure_is_an_outage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string("event-trigger management is not enabled"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let triggers = RoutineAutomationTriggers::new(&server.uri(), "key").expect("client");
    let task_event = automation(
        AutomationTrigger::TaskEvent {
            property_id: STATUS,
            values: Vec::new(),
        },
        None,
    );

    let refused = triggers.create(&task_event).await;
    let failed = triggers.create(&task_event).await;

    assert!(matches!(
        refused,
        Err(AutomationTriggerError::Refused(reason)) if reason == "event-trigger management is not enabled"
    ));
    assert!(matches!(
        failed,
        Err(AutomationTriggerError::Unavailable(_))
    ));
}

/// Deleting a routine already gone deletes it.
#[tokio::test]
async fn deleting_a_routine_already_gone_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/scheduled-actions/{ROUTINE}")))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let triggers = RoutineAutomationTriggers::new(&server.uri(), "key").expect("client");
    let owner = MacroUserIdStr::try_from(OWNER.to_string()).expect("valid");

    triggers.delete(&owner, ROUTINE).await.expect("deleted");
}
