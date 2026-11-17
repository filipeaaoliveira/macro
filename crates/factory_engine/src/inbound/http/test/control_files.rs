//! The Control File settings routes, Runs created in person, and answering
//! a Tampered Run's Gate.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{ControlFileChanges, TeamMembership};
use factory_records::domain::ports::RunRepository;
use macro_authorization::CODE_EXECUTION_HEADER;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, ROBOT, alice_s_run, body, call, records_router};
use crate::domain::control_files::ControlFiles;
use crate::domain::fakes::FakeRepo;

const PATH: &str = "/settings/control-files";

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

/// Only the team admin in their own signed-in session sets the team's
/// patterns: a shorter list lets an agent change more files unseen. An
/// internal caller acting for them, their API key or token, and an agent
/// working as them are refused, and nothing is kept.
#[tokio::test]
async fn only_the_admin_s_own_session_sets_the_team_s_control_files() {
    let repo = FakeRepo::default();
    let team_id = alice_s_team(&repo);
    let router = records_router(&repo);
    let patterns = serde_json::json!({ "patterns": ["ci/**"] });
    let send = |request: Request<Body>| {
        let router = router.clone();
        async move { router.oneshot(request).await.expect("response").status() }
    };

    let internal = send(call(
        "PUT",
        PATH,
        Caller::Internal(Some(ALICE)),
        patterns.clone(),
    ))
    .await;
    let api_key = send(call(
        "PUT",
        PATH,
        Caller::ApiKey("alice-key"),
        patterns.clone(),
    ))
    .await;
    let sdk = send(call("PUT", PATH, Caller::User("sdk"), patterns.clone())).await;
    let agent = send(call("PUT", PATH, Caller::User("agent"), patterns.clone())).await;
    let mut in_code_mode = call("PUT", PATH, Caller::User("alice"), patterns.clone());
    in_code_mode
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    let in_code_mode = send(in_code_mode).await;
    let routine = send(call("PUT", PATH, Caller::User("routine"), patterns.clone())).await;
    let no_team = send(call("PUT", PATH, Caller::User("eve"), patterns.clone())).await;

    for refused in [
        internal,
        api_key,
        sdk,
        agent,
        routine,
        in_code_mode,
        no_team,
    ] {
        assert_eq!(refused, StatusCode::FORBIDDEN);
    }
    assert!(repo.control_files.lock().expect("patterns").is_empty());

    let person = router
        .clone()
        .oneshot(call("PUT", PATH, Caller::User("alice"), patterns))
        .await
        .expect("response");
    assert_eq!(person.status(), StatusCode::OK);
    assert_eq!(
        body(person).await,
        serde_json::json!({
            "patterns": ["ci/**"],
            "isDefault": false,
            "teamId": team_id,
            "canEdit": true,
        })
    );
}

/// The patterns read as the caller's team's, or the default, by any caller
/// acting for them; patterns that do not read are refused.
#[tokio::test]
async fn the_control_files_read_as_the_team_s_or_the_default() {
    let repo = FakeRepo::default();
    alice_s_team(&repo);
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
    assert_eq!(default.status(), StatusCode::OK);
    assert_eq!(
        body(default).await,
        serde_json::json!({
            "patterns": ControlFiles::default_patterns(),
            "isDefault": true,
            "teamId": null,
            "canEdit": false,
        })
    );

    let invalid = router
        .oneshot(call(
            "PUT",
            PATH,
            Caller::User("alice"),
            serde_json::json!({ "patterns": ["a**b"] }),
        ))
        .await
        .expect("response");
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

/// A Run created in the owner's own session is kept as created in person;
/// one an internal caller, an API key or token, an agent or a routine
/// working as them, or an agent's own account created is not.
#[tokio::test]
async fn a_run_is_created_in_person_only_from_the_owner_s_own_session() {
    let repo = FakeRepo::default();
    repo.agent_accounts
        .lock()
        .expect("agent accounts")
        .insert(ROBOT.to_string());
    let router = records_router(&repo);
    let create = serde_json::json!({ "flow": "reply", "request": "Fix ci.yml" });
    let mut in_code_mode = call("POST", "/runs", Caller::User("alice"), create.clone());
    in_code_mode
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));

    let cases = [
        (
            "alice",
            call("POST", "/runs", Caller::User("alice"), create.clone()),
            true,
        ),
        (
            "internal",
            call(
                "POST",
                "/runs",
                Caller::Internal(Some(ALICE)),
                create.clone(),
            ),
            false,
        ),
        (
            "api key",
            call("POST", "/runs", Caller::ApiKey("alice-key"), create.clone()),
            false,
        ),
        (
            "sdk",
            call("POST", "/runs", Caller::User("sdk"), create.clone()),
            false,
        ),
        (
            "agent",
            call("POST", "/runs", Caller::User("agent"), create.clone()),
            false,
        ),
        (
            "routine",
            call("POST", "/runs", Caller::User("routine"), create.clone()),
            false,
        ),
        (
            "routine session",
            call(
                "POST",
                "/runs",
                Caller::User("routine-session"),
                create.clone(),
            ),
            false,
        ),
        (
            "agent account",
            call("POST", "/runs", Caller::User("robot"), create.clone()),
            false,
        ),
        ("code mode", in_code_mode, false),
    ];
    for (caller, request, expected) in cases {
        let response = router.clone().oneshot(request).await.expect("response");

        assert_eq!(response.status(), StatusCode::CREATED, "{caller}");
        assert_eq!(
            body(response).await["run"]["createdInPerson"],
            expected,
            "{caller}"
        );
    }
}

/// An agent's account that administers a team signs in as a person does,
/// yet sets nothing.
#[tokio::test]
async fn an_agent_account_sets_no_team_s_control_files() {
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
    let router = records_router(&repo);

    let refused = router
        .oneshot(call(
            "PUT",
            PATH,
            Caller::User("robot"),
            serde_json::json!({ "patterns": [] }),
        ))
        .await
        .expect("response");

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(repo.control_files.lock().expect("patterns").is_empty());
}

/// A Tampered Run shows its files; approving its Gate without listing them
/// is refused with a code a client acts on, and with them is taken.
#[tokio::test]
async fn approving_a_tampered_run_needs_the_files_listed() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let run_id = alice_s_run(&repo).await;
    let changes = ControlFileChanges {
        tampered: vec!["package.json".to_string()],
        named: vec!["Makefile".to_string()],
        unlisted: false,
    };
    repo.set_control_file_changes(run_id, Some(&changes))
        .await
        .expect("keep");
    let answer = format!("/runs/{run_id}/steps/4/answer");

    let read = router
        .clone()
        .oneshot(call(
            "GET",
            &format!("/runs/{run_id}"),
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    assert_eq!(
        body(read).await["run"]["controlFiles"],
        serde_json::json!({
            "tampered": ["package.json"],
            "named": ["Makefile"],
            "unlisted": false,
        })
    );

    let refused = router
        .clone()
        .oneshot(call(
            "POST",
            &answer,
            Caller::User("alice"),
            serde_json::json!({ "answer": "approve" }),
        ))
        .await
        .expect("response");
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(refused).await["code"], "controlFilesNotApproved");

    let approved = router
        .oneshot(call(
            "POST",
            &answer,
            Caller::User("alice"),
            serde_json::json!({
                "answer": "approve",
                "approvedControlFiles": ["package.json"],
            }),
        ))
        .await
        .expect("response");
    assert_eq!(approved.status(), StatusCode::ACCEPTED);
}
