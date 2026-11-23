//! Changes to a team's Factory Definition proposed from the factory's
//! settings: checked, committed on a branch of their own, and opened as a
//! pull request, only by an admin of the team.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::definition::{DEFINITION_REPO, FIRST, github_at, team_repo};
use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::definition::{DefinitionChange, DefinitionChangeError, DefinitionError};
use crate::domain::fakes::FakeGithub;
use crate::domain::models::{DefinitionPullRequest, FactoryError, NewBranch};
use crate::domain::ports::FactoryService;

const CHANGE: Uuid = Uuid::from_u128(0xc4a9);

const NOTES: &str = r#"{
    "description": "Notes.",
    "entries": [{
        "id": "reply",
        "step": "reply",
        "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
    }]
}"#;

const NOTES_CHANGED: &str = r#"{
    "description": "Notes, replied to.",
    "entries": [{
        "id": "reply",
        "step": "reply",
        "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
    }]
}"#;

/// GitHub with the team's Definition, of one Flow, `notes`, at [`FIRST`].
fn definition_github() -> FakeGithub {
    let github = github_at(FIRST);
    let mut state = github.state();
    for (path, text) in [
        ("factory.json", r#"{ "version": 1, "flows": ["notes"] }"#),
        ("flows/notes.json", NOTES),
        (
            "agents.json",
            r#"{
                "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
                "roles": { "writer": ["primary"] }
            }"#,
        ),
    ] {
        state
            .files
            .insert((FIRST.to_string(), path.to_string()), text.to_string());
    }
    drop(state);
    github
}

fn change(path: &str, text: &str) -> DefinitionChange {
    DefinitionChange::new(
        "Say what notes reply to".to_string(),
        Some("Our notes flow replies.".to_string()),
        BTreeMap::from([(path.to_string(), text.to_string())]),
    )
    .expect("a change")
}

/// An admin's change that the Definition reads with is committed on its
/// own branch over the default branch's head and opened as a pull request
/// onto the default branch, which is not touched.
#[tokio::test]
async fn an_admin_s_change_opens_a_pull_request_on_the_definition() {
    let repo = team_repo();
    let github = definition_github();
    let service = code_service(&repo, &FakeWorkflows::default(), &github);

    let opened = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await
        .expect("the change is opened");

    assert_eq!(
        opened,
        DefinitionPullRequest {
            number: 7,
            title: "Say what notes reply to".to_string(),
            url: format!("{DEFINITION_REPO}/pull/7"),
            draft: false,
        }
    );
    let state = github.state();
    assert_eq!(
        state.created_branches,
        [NewBranch {
            name: "definition-change/00000000-0000-0000-0000-00000000c4a9".to_string(),
            base_commit: FIRST.to_string(),
            message: "Say what notes reply to".to_string(),
            files: BTreeMap::from([("flows/notes.json".to_string(), NOTES_CHANGED.to_string())]),
        }]
    );
    assert_eq!(state.opened, 1);
    assert!(
        state.bodies[0].starts_with("Our notes flow replies.\n\n"),
        "{}",
        state.bodies[0]
    );
    assert!(state.bodies[0].contains(FIRST), "{}", state.bodies[0]);
}

/// The same change sent again, as after a lost answer, finds its branch
/// and answers the pull request it opened: GitHub opens no second one.
#[tokio::test]
async fn a_change_sent_again_answers_its_pull_request() {
    let repo = team_repo();
    let github = definition_github();
    let service = code_service(&repo, &FakeWorkflows::default(), &github);
    let first = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await
        .expect("the change is opened");
    github.state().open_refusal = Some("A pull request already exists".to_string());

    let again = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await
        .expect("the same pull request");

    assert_eq!(again, first);
    assert_eq!(github.state().created_branches.len(), 1);
    assert_eq!(github.state().opened, 1);
}

/// A change the Definition does not read with is refused with the
/// validator's reason, and nothing reaches GitHub.
#[tokio::test]
async fn a_change_the_definition_does_not_read_with_is_refused() {
    let repo = team_repo();
    let github = definition_github();
    let service = code_service(&repo, &FakeWorkflows::default(), &github);

    let refused = service
        .propose_definition_change(
            julia(),
            CHANGE,
            change("factory.json", r#"{ "version": 2, "flows": ["notes"] }"#),
        )
        .await;

    assert!(
        matches!(
            refused,
            Err(FactoryError::InvalidDefinitionChange(
                DefinitionChangeError::Invalid(DefinitionError::Version(2))
            ))
        ),
        "{refused:?}"
    );
    assert!(github.state().created_branches.is_empty());
    assert_eq!(github.state().opened, 0);
}

/// Only an admin of the team changes its rules; an admin's GitHub account
/// must push to the Definition's repository; and a team that names no
/// Definition has none to change.
#[tokio::test]
async fn only_an_admin_who_may_push_changes_a_named_definition() {
    let repo = team_repo();
    let github = definition_github();
    let service = code_service(&repo, &FakeWorkflows::default(), &github);

    let member = service
        .propose_definition_change(teo(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await;
    repo.agent_accounts
        .lock()
        .expect("agents")
        .insert(julia().to_string());
    let agent = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await;
    repo.agent_accounts.lock().expect("agents").clear();
    github.state().read_only = true;
    let read_only = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await;
    github.state().read_only = false;
    repo.team_definitions.lock().expect("definitions").clear();
    let unnamed = service
        .propose_definition_change(julia(), CHANGE, change("flows/notes.json", NOTES_CHANGED))
        .await;

    assert!(
        matches!(member, Err(FactoryError::NotATeamAdmin)),
        "{member:?}"
    );
    assert!(matches!(agent, Err(FactoryError::NotAPerson)), "{agent:?}");
    assert!(
        matches!(read_only, Err(FactoryError::NoPushAccess)),
        "{read_only:?}"
    );
    assert!(
        matches!(unnamed, Err(FactoryError::NoDefinition)),
        "{unnamed:?}"
    );
    assert!(github.state().created_branches.is_empty());
    assert_eq!(github.state().opened, 0);
}
