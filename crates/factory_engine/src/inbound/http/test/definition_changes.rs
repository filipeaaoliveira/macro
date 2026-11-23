//! The route that proposes a change to a team's Factory Definition as a
//! pull request.

use axum::http::StatusCode;
use factory_records::domain::models::TeamMembership;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, EVE, body, call, github_router};
use crate::domain::fakes::{FakeGithub, FakeRepo};

const PATH: &str = "/settings/definition/changes";

const REPO: &str = "https://github.com/macro/factory-definition";

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Alice administers the team that names the Definition; Eve is a member.
fn team() -> FakeRepo {
    let repo = FakeRepo::default();
    let team_id = Uuid::now_v7();
    let mut teams = repo.teams.lock().expect("teams");
    teams.insert(
        ALICE.to_string(),
        TeamMembership {
            team_id,
            admin: true,
        },
    );
    teams.insert(
        EVE.to_string(),
        TeamMembership {
            team_id,
            admin: false,
        },
    );
    drop(teams);
    repo.team_definitions
        .lock()
        .expect("definitions")
        .insert(team_id, REPO.to_string());
    repo
}

/// GitHub with the Definition, of one Flow, `notes`, at [`HEAD`].
fn github() -> FakeGithub {
    let github = FakeGithub::default();
    let mut state = github.state();
    state.head = Some(HEAD.to_string());
    for (path, text) in [
        ("factory.json", r#"{ "version": 1, "flows": ["notes"] }"#),
        (
            "flows/notes.json",
            r#"{ "entries": [{ "id": "reply", "step": "reply",
                "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] } }] }"#,
        ),
        (
            "agents.json",
            r#"{ "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
                "roles": { "writer": ["primary"] } }"#,
        ),
    ] {
        state
            .files
            .insert((HEAD.to_string(), path.to_string()), text.to_string());
    }
    drop(state);
    github
}

fn proposal(files: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "changeId": "00000000-0000-0000-0000-00000000c4a9",
        "title": "Describe the notes flow",
        "files": files,
    })
}

/// The admin's change, in their own session, answers its pull request; a
/// member, and the admin through an internal caller or an API key, are
/// refused, and nothing reaches GitHub.
#[tokio::test]
async fn only_the_admin_in_person_proposes_a_change() {
    let repo = team();
    let github = github();
    let router = github_router(&repo, github.clone());
    let change = proposal(serde_json::json!({
        "factory.json": r#"{ "version": 1, "description": "Ours.", "flows": ["notes"] }"#,
    }));

    for caller in [
        Caller::User("eve"),
        Caller::Internal(Some(ALICE)),
        Caller::ApiKey("alice-key"),
    ] {
        let refused = router
            .clone()
            .oneshot(call("POST", PATH, caller, change.clone()))
            .await
            .expect("response");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }
    assert!(github.state().created_branches.is_empty());

    let opened = router
        .oneshot(call("POST", PATH, Caller::User("alice"), change))
        .await
        .expect("response");
    assert_eq!(opened.status(), StatusCode::OK);
    assert_eq!(
        body(opened).await,
        serde_json::json!({
            "number": 7,
            "title": "Describe the notes flow",
            "url": format!("{REPO}/pull/7"),
            "draft": false,
        })
    );
    assert_eq!(
        github.state().created_branches[0].name,
        "definition-change/00000000-0000-0000-0000-00000000c4a9"
    );
}

/// A change the Definition does not read with is the caller's to fix, and
/// says so with its code.
#[tokio::test]
async fn a_change_that_does_not_read_is_refused_with_its_reason() {
    let repo = team();
    let github = github();
    let router = github_router(&repo, github.clone());

    let refused = router
        .oneshot(call(
            "POST",
            PATH,
            Caller::User("alice"),
            proposal(serde_json::json!({ "agents.json": "{ not json" })),
        ))
        .await
        .expect("response");

    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let refused = body(refused).await;
    assert_eq!(refused["code"], "invalidDefinitionChange");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("agents.json")),
        "{refused}"
    );
    assert!(github.state().created_branches.is_empty());
}

/// A file the Definition does not read and a change that changes nothing
/// are the caller's to fix, with the same code and their reason.
#[tokio::test]
async fn an_unread_file_or_no_change_is_refused_with_its_reason() {
    let repo = team();
    let github = github();
    let router = github_router(&repo, github.clone());
    let unchanged_manifest = r#"{ "version": 1, "flows": ["notes"] }"#;

    let mut answers = Vec::new();
    for files in [
        serde_json::json!({ "flows/other.json": "{}" }),
        serde_json::json!({ "factory.json": unchanged_manifest }),
    ] {
        let refused = router
            .clone()
            .oneshot(call("POST", PATH, Caller::User("alice"), proposal(files)))
            .await
            .expect("response");
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        answers.push(body(refused).await);
    }

    assert_eq!(answers[0]["code"], "invalidDefinitionChange");
    assert!(
        answers[0]["message"].as_str().is_some_and(
            |message| message.contains("flows/other.json is not a file the definition reads")
        ),
        "{}",
        answers[0]
    );
    assert_eq!(answers[1]["code"], "invalidDefinitionChange");
    assert!(
        answers[1]["message"]
            .as_str()
            .is_some_and(|message| message.contains("leaves every file as it is")),
        "{}",
        answers[1]
    );
    assert!(github.state().created_branches.is_empty());
}

/// A member is refused before the change is looked at, however wrong it
/// is.
#[tokio::test]
async fn a_member_is_refused_before_the_change_is_checked() {
    let repo = team();
    let router = github_router(&repo, github());

    let refused = router
        .oneshot(call(
            "POST",
            PATH,
            Caller::User("eve"),
            proposal(serde_json::json!({})),
        ))
        .await
        .expect("response");

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

/// The change sent again once its pull request was closed unmerged is a
/// conflict that says so.
#[tokio::test]
async fn a_closed_change_sent_again_is_a_conflict() {
    let repo = team();
    let github = github();
    let router = github_router(&repo, github.clone());
    let change = proposal(serde_json::json!({
        "factory.json": r#"{ "version": 1, "description": "Ours.", "flows": ["notes"] }"#,
    }));
    let opened = router
        .clone()
        .oneshot(call("POST", PATH, Caller::User("alice"), change.clone()))
        .await
        .expect("response");
    assert_eq!(opened.status(), StatusCode::OK);
    github.state().pull_request.as_mut().expect("opened").state =
        crate::domain::models::PullRequestState::Closed;

    let again = router
        .oneshot(call("POST", PATH, Caller::User("alice"), change))
        .await
        .expect("response");

    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(body(again).await["code"], "definitionChangeClosed");
}
