//! The Standing Rules settings routes, and the rules a new Run keeps.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::models::TeamMembership;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use macro_authorization::CODE_EXECUTION_HEADER;
use serde_json::json;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, ROBOT, body, call, records_router};
use crate::domain::fakes::FakeRepo;

const PATH: &str = "/settings/standing-rules";

const PLAN_ONLY: OrchestratorDefinition = OrchestratorDefinition {
    stand_in: factory_records::domain::standing_rules::StandInKinds {
        plan: false,
        blockers: false,
    },
    plan: true,
    blockers: false,
    merge: false,
    owner_gates: false,
};

/// A repository where Alice administers a team.
fn alice_s_team(repo: &FakeRepo) -> Uuid {
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        ALICE.to_string(),
        TeamMembership {
            team_id,
            admin: true,
        },
    );
    team_id
}

fn project_path(project: Uuid) -> String {
    format!("{PATH}/projects/{project}")
}

/// Only the team admin in their own signed-in session sets the team's
/// rules: a rule answers Gates in a person's place. An internal caller
/// acting for them, their API key or token, an agent working as them, code
/// mode and a person on no team are refused, and nothing is kept.
#[tokio::test]
async fn only_the_admin_s_own_session_sets_the_team_s_rules() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    let router = records_router(&repo);
    let rules = json!({ "definition": { "plan": true, "blockers": false, "merge": true, "standIn": { "plan": true, "blockers": false } } });
    let send = |request: Request<Body>| {
        let router = router.clone();
        async move { router.oneshot(request).await.expect("response").status() }
    };

    let internal = send(call(
        "PUT",
        PATH,
        Caller::Internal(Some(ALICE)),
        rules.clone(),
    ))
    .await;
    let api_key = send(call(
        "PUT",
        PATH,
        Caller::ApiKey("alice-key"),
        rules.clone(),
    ))
    .await;
    let agent = send(call("PUT", PATH, Caller::User("agent"), rules.clone())).await;
    let mut in_code_mode = call("PUT", PATH, Caller::User("alice"), rules.clone());
    in_code_mode
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    let in_code_mode = send(in_code_mode).await;
    let no_team = send(call("PUT", PATH, Caller::User("eve"), rules.clone())).await;
    for refused in [internal, api_key, agent, in_code_mode, no_team] {
        assert_eq!(refused, StatusCode::FORBIDDEN);
    }
    assert!(repo.team_rules.lock().expect("rules").is_empty());

    let person = router
        .clone()
        .oneshot(call("PUT", PATH, Caller::User("alice"), rules))
        .await
        .expect("response");
    assert_eq!(person.status(), StatusCode::OK);
    assert_eq!(
        body(person).await,
        json!({
            "definition": { "plan": true, "blockers": false, "merge": true, "ownerGates": false, "standIn": { "plan": true, "blockers": false } },
            "isDefault": false,
            "projects": [],
            "teamId": team_id,
            "canEdit": true,
        })
    );

    // Null goes back to the default, which fixes Blockers on its own and
    // leaves the plan and the merge to a person.
    let reset = router
        .oneshot(call(
            "PUT",
            PATH,
            Caller::User("alice"),
            json!({ "definition": null }),
        ))
        .await
        .expect("response");
    assert_eq!(
        body(reset).await["definition"],
        json!({ "plan": false, "blockers": true, "merge": false, "ownerGates": false, "standIn": { "plan": false, "blockers": false } })
    );
}

/// A Project's rules are set by the team's admin, for a Project they may
/// edit; one they may not edit and one that does not exist read the same.
#[tokio::test]
async fn an_admin_sets_rules_for_a_project_they_may_edit() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    let (project, hidden) = (Uuid::now_v7(), Uuid::now_v7());
    repo.projects
        .lock()
        .expect("projects")
        .extend([project, hidden]);
    repo.uneditable_projects
        .lock()
        .expect("uneditable")
        .insert((ALICE.to_string(), hidden));
    let router = records_router(&repo);
    let rules = json!({ "definition": { "plan": true, "blockers": true, "merge": true } });
    let send = |path: String, caller: Caller<'static>, body: serde_json::Value| {
        let router = router.clone();
        async move {
            router
                .oneshot(call("PUT", &path, caller, body))
                .await
                .expect("response")
        }
    };

    let missing = send(
        project_path(Uuid::now_v7()),
        Caller::User("alice"),
        rules.clone(),
    )
    .await;
    let uneditable = send(project_path(hidden), Caller::User("alice"), rules.clone()).await;
    let internal = send(
        project_path(project),
        Caller::Internal(Some(ALICE)),
        rules.clone(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(uneditable.status(), StatusCode::BAD_REQUEST);
    assert_eq!(internal.status(), StatusCode::FORBIDDEN);
    assert!(repo.project_rules.lock().expect("rules").is_empty());

    let set = send(project_path(project), Caller::User("alice"), rules).await;
    assert_eq!(set.status(), StatusCode::OK);
    assert_eq!(
        body(set).await["projects"],
        json!([{
            "projectId": project,
            "definition": { "plan": true, "blockers": true, "merge": true, "ownerGates": false, "standIn": { "plan": false, "blockers": false } },
        }])
    );
    assert!(
        repo.project_rules
            .lock()
            .expect("rules")
            .contains_key(&(team_id, project))
    );

    let removed = send(
        project_path(project),
        Caller::User("alice"),
        json!({ "definition": null }),
    )
    .await;
    assert_eq!(body(removed).await["projects"], json!([]));
}

/// The settings read as the team's or the default, by any caller acting
/// for the person.
#[tokio::test]
async fn the_rules_read_as_the_team_s_or_the_default() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    repo.team_rules
        .lock()
        .expect("rules")
        .insert(team_id, PLAN_ONLY);
    let router = records_router(&repo);

    let default = router
        .clone()
        .oneshot(call(
            "GET",
            PATH,
            Caller::Internal(Some("macro|eve@example.com")),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(
        body(default).await,
        json!({
            "definition": { "plan": false, "blockers": true, "merge": false, "ownerGates": false, "standIn": { "plan": false, "blockers": false } },
            "isDefault": true,
            "projects": [],
            "teamId": null,
            "canEdit": false,
        })
    );
    let own = router
        .oneshot(call(
            "GET",
            PATH,
            Caller::Internal(Some(ALICE)),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(
        body(own).await["definition"],
        json!({ "plan": true, "blockers": false, "merge": false, "ownerGates": false, "standIn": { "plan": false, "blockers": false } })
    );
}

/// A new Run keeps the rules of the Project its task is in when the team
/// set some, else the team's, else the default; changing them later does
/// not change the Run's.
#[tokio::test]
async fn a_new_run_keeps_its_project_s_or_its_team_s_rules() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    let (in_project, elsewhere, project) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    repo.tasks
        .lock()
        .expect("tasks")
        .extend([in_project, elsewhere]);
    repo.task_projects
        .lock()
        .expect("task projects")
        .insert(in_project, project);
    let every_rule = OrchestratorDefinition {
        stand_in: factory_records::domain::standing_rules::StandInKinds {
            plan: false,
            blockers: false,
        },
        plan: true,
        blockers: true,
        merge: true,
        owner_gates: false,
    };
    repo.team_rules
        .lock()
        .expect("rules")
        .insert(team_id, PLAN_ONLY);
    repo.project_rules
        .lock()
        .expect("rules")
        .insert((team_id, project), every_rule);
    let router = records_router(&repo);
    let start = |caller: Caller<'static>, task: Option<Uuid>| {
        let router = router.clone();
        async move {
            let response = router
                .oneshot(call(
                    "POST",
                    "/runs",
                    caller,
                    json!({ "flow": "plan", "request": "Plan it", "taskId": task }),
                ))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::CREATED);
            body(response).await["run"]["standingRules"].clone()
        }
    };

    let projects = start(Caller::User("alice"), Some(in_project)).await;
    let teams = start(Caller::User("alice"), Some(elsewhere)).await;
    let defaults = start(Caller::Internal(Some("macro|eve@example.com")), None).await;

    assert_eq!(
        projects,
        serde_json::to_value(RunRules {
            definition: every_rule,
            source: RulesSource::Project,
            project_id: Some(project),
        })
        .expect("json")
    );
    assert_eq!(
        teams,
        json!({
            "definition": { "plan": true, "blockers": false, "merge": false, "ownerGates": false, "standIn": { "plan": false, "blockers": false } },
            "source": "team",
            "projectId": null,
        })
    );
    assert_eq!(
        defaults,
        serde_json::to_value(RunRules::DEFAULT).expect("json")
    );

    // The team goes back to the default: a new Run takes it, and the Run
    // created before keeps the team's.
    repo.team_rules.lock().expect("rules").clear();
    let after = start(Caller::User("alice"), Some(elsewhere)).await;
    assert_eq!(after["source"], "default");
    let sources: Vec<_> = repo
        .runs
        .lock()
        .expect("runs")
        .values()
        .filter_map(|run| run.standing_rules.map(|rules| rules.source))
        .collect();
    assert_eq!(
        sources
            .iter()
            .filter(|source| **source == RulesSource::Team)
            .count(),
        1
    );
}

/// A Project that cannot be read when a Run starts leaves it its team's
/// rules: the Run is not refused.
#[tokio::test]
async fn a_run_whose_project_cannot_be_read_keeps_its_team_s_rules() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    repo.team_rules
        .lock()
        .expect("rules")
        .insert(team_id, PLAN_ONLY);
    *repo.project_reads_fail.lock().expect("fail") = true;

    let response = records_router(&repo)
        .oneshot(call(
            "POST",
            "/runs",
            Caller::User("alice"),
            json!({ "flow": "plan", "request": "Plan it", "taskId": task }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        body(response).await["run"]["standingRules"]["source"],
        "team"
    );
}

/// An agent's account reads its team's rules, and may never change them,
/// whatever its role on the team.
#[tokio::test]
async fn an_agent_s_account_may_not_change_the_rules() {
    let repo = FakeRepo::default();
    repo.agent_accounts
        .lock()
        .expect("agent accounts")
        .insert(ROBOT.to_string());
    repo.teams.lock().expect("teams").insert(
        ROBOT.to_string(),
        TeamMembership {
            team_id: Uuid::now_v7(),
            admin: true,
        },
    );

    let read = records_router(&repo)
        .oneshot(call(
            "GET",
            PATH,
            Caller::User("robot"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(body(read).await["canEdit"], false);
}
