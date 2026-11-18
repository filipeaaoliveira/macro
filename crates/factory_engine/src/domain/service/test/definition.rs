//! A team's Factory Definition: the Runs of its members start of its Flows,
//! under its rules, and keep the commit they read it at.

use factory_records::domain::models::{RunDefinition, TeamMembership};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource};
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::definition::DefinitionError;
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const TEAM: Uuid = Uuid::from_u128(0x7ea4);

const DEFINITION_REPO: &str = "https://github.com/macro/factory-definition";

/// The commit the Definition's default branch is at first.
const FIRST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The commit a merged change to the Definition takes it to.
const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const AGENTS: &str = r#"{
    "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
    "roles": { "writer": ["primary"] }
}"#;

/// A Definition of one Flow, `notes`, that replies, with `rules`.
fn definition_files(github: &FakeGithub, commit: &str, description: &str, rules: Option<&str>) {
    let flow = format!(
        r#"{{
            "description": "{description}",
            "entries": [{{
                "id": "reply",
                "step": "reply",
                "on": {{ "done": [{{ "to": {{ "kind": "end", "outcome": "success" }} }}] }}
            }}]
        }}"#
    );
    let mut files = vec![
        (
            "factory.json",
            r#"{ "version": 1, "flows": ["notes"] }"#.to_string(),
        ),
        ("flows/notes.json", flow),
        ("agents.json", AGENTS.to_string()),
    ];
    if let Some(rules) = rules {
        files.push(("rules.json", rules.to_string()));
    }
    let mut state = github.state();
    for (path, text) in files {
        state
            .files
            .insert((commit.to_string(), path.to_string()), text);
    }
}

/// Julia administers the team, which names the Definition; Teo is on it.
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
    repo.team_definitions
        .lock()
        .expect("team definitions")
        .insert(TEAM, DEFINITION_REPO.to_string());
    repo
}

fn github_at(commit: &str) -> FakeGithub {
    let github = FakeGithub::default();
    github.state().head = Some(commit.to_string());
    github
}

/// A Run of a team that names a Definition starts of the Definition's Flow,
/// under its rules and Control Files, keeps the commit it read and the
/// Agents its Steps take; a change merged later reaches only later Runs.
#[tokio::test]
async fn a_run_starts_of_its_team_s_definition_and_keeps_the_commit_it_read() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(
        &github,
        FIRST,
        "Notes, first.",
        Some(
            r#"{
                "standingRules": { "plan": true, "blockers": false, "merge": false },
                "controlFiles": ["docs/**"]
            }"#,
        ),
    );
    let service = code_service(&repo, &workflows, &github);

    let first = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Note it".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("a run of the definition's flow");

    // A change to the Definition merged meanwhile moves its default branch.
    definition_files(&github, SECOND, "Notes, second.", None);
    github.state().head = Some(SECOND.to_string());
    let second = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Note it".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("a second run");

    assert_eq!(
        first.run.definition,
        Some(RunDefinition {
            repo_url: DEFINITION_REPO.to_string(),
            commit: FIRST.to_string(),
        })
    );
    assert_eq!(
        second.run.definition.map(|definition| definition.commit),
        Some(SECOND.to_string())
    );
    // The first Run keeps its commit once the Definition moved on.
    let read = service.get_run(&teo(), first.run.id).await.expect("read");
    assert_eq!(
        read.run.definition.map(|definition| definition.commit),
        Some(FIRST.to_string())
    );

    let started = workflows.started.lock().expect("started").clone();
    assert_eq!(started[0].flow.description, "Notes, first.");
    assert_eq!(started[1].flow.description, "Notes, second.");

    let rules = first.run.standing_rules.expect("rules");
    assert_eq!(rules.source, RulesSource::Definition);
    assert!(rules.definition.plan);
    assert_eq!(
        first.run.control_file_patterns,
        Some(vec!["docs/**".to_string()])
    );
    // The second commit's Definition sets no rules: the factory's default.
    let rules = second.run.standing_rules.expect("rules");
    assert_eq!(rules.definition, OrchestratorDefinition::DEFAULT);
    assert_eq!(rules.source, RulesSource::Definition);

    let agents = repo
        .run_definition_agents(first.run.id)
        .await
        .expect("read")
        .expect("the run keeps its definition's agents");
    assert_eq!(
        agents["agents"]["primary"]["bot"],
        "00000000-0000-0000-0000-00000000c1a0"
    );
}

/// A team's Definition names its Flows: a built-in Flow it does not name
/// starts no Run, and the Flows listed are the Definition's.
#[tokio::test]
async fn a_team_s_definition_names_the_flows_its_runs_start_of() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    let service = code_service(&repo, &workflows, &github);

    let refused = service
        .create_run(
            teo(),
            "reply".to_string(),
            "Hi".to_string(),
            None,
            None,
            true,
        )
        .await;
    let flows = service.list_flows(&teo()).await.expect("flows");

    assert!(
        matches!(refused, Err(FactoryError::UnknownFlow)),
        "{refused:?}"
    );
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].name, "notes");
    assert_eq!(flows[0].description, "Notes.");
}

/// A Definition that does not read at its head, or that the person cannot
/// reach, starts no Run, and says why.
#[tokio::test]
async fn a_definition_that_does_not_read_starts_no_run() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    github
        .state()
        .files
        .remove(&(FIRST.to_string(), "agents.json".to_string()));
    let service = code_service(&repo, &workflows, &github);

    let invalid = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Hi".to_string(),
            None,
            None,
            true,
        )
        .await;
    github.state().unreachable = true;
    let unreachable = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Hi".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(
        matches!(
            &invalid,
            Err(FactoryError::InvalidDefinition(DefinitionError::Missing(path))) if path == "agents.json"
        ),
        "{invalid:?}"
    );
    assert!(
        matches!(unreachable, Err(FactoryError::UnreachableDefinition)),
        "{unreachable:?}"
    );
    assert!(workflows.started.lock().expect("started").is_empty());
}

/// An admin names the Definition only once it reads; while one is named,
/// the team's rules and Control Files change only through it.
#[tokio::test]
async fn naming_a_definition_checks_it_and_holds_the_team_s_settings() {
    let repo = team_repo();
    repo.team_definitions
        .lock()
        .expect("team definitions")
        .clear();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    let service = code_service(&repo, &workflows, &github);

    let unreadable = service
        .set_definition(julia(), Some(DEFINITION_REPO.to_string()))
        .await;
    assert!(
        matches!(unreadable, Err(FactoryError::InvalidDefinition(_))),
        "{unreadable:?}"
    );
    assert_eq!(repo.team_definitions.lock().expect("kept").len(), 0);

    definition_files(&github, FIRST, "Notes.", None);
    let member = service
        .set_definition(teo(), Some(DEFINITION_REPO.to_string()))
        .await;
    assert!(
        matches!(member, Err(FactoryError::NotATeamAdmin)),
        "{member:?}"
    );

    let named = service
        .set_definition(julia(), Some(DEFINITION_REPO.to_string()))
        .await
        .expect("named");
    assert_eq!(named.repo_url.as_deref(), Some(DEFINITION_REPO));
    assert_eq!(named.commit.as_deref(), Some(FIRST));
    assert_eq!(named.problem, None);
    assert!(named.can_edit);

    let rules = service
        .set_standing_rules(julia(), Some(OrchestratorDefinition::DEFAULT))
        .await;
    let control_files = service.set_control_files(julia(), None).await;
    assert!(
        matches!(rules, Err(FactoryError::DefinitionNamed)),
        "{rules:?}"
    );
    assert!(
        matches!(control_files, Err(FactoryError::DefinitionNamed)),
        "{control_files:?}"
    );
    let settings = service.standing_rules(&julia()).await.expect("settings");
    assert!(!settings.can_edit);

    let cleared = service
        .set_definition(julia(), None)
        .await
        .expect("cleared");
    assert_eq!(cleared.repo_url, None);
    service
        .set_control_files(julia(), None)
        .await
        .expect("the team's own settings again");
}

/// A Definition of one Flow, `change`, that works on a repository and asks
/// a person once its reply is done.
fn asking_definition(github: &FakeGithub, commit: &str) {
    let flow = r#"{
        "description": "Change it.",
        "repository": true,
        "entries": [{
            "id": "reply",
            "step": "reply",
            "gate": { "question": "Good?", "answers": ["yes", "no"] },
            "on": {
                "yes": [{ "to": { "kind": "end", "outcome": "success" } }],
                "no": [{ "to": { "kind": "end", "outcome": "manual" } }]
            }
        }]
    }"#;
    let files = [
        ("factory.json", r#"{ "version": 1, "flows": ["change"] }"#),
        ("flows/change.json", flow),
        ("agents.json", AGENTS),
        ("rules.json", r#"{ "controlFiles": ["ci/**"] }"#),
    ];
    let mut state = github.state();
    for (path, text) in files {
        state
            .files
            .insert((commit.to_string(), path.to_string()), text.to_string());
    }
}

/// A Run on its team's Definition repository counts the Definition's own
/// files as Control Files, beside the list its rules give; a Run on
/// another repository does not.
#[tokio::test]
async fn a_run_on_the_definition_repository_guards_its_files() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    asking_definition(&github, FIRST);
    let service = code_service(&repo, &workflows, &github);

    let on_definition = service
        .create_run(
            teo(),
            "change".to_string(),
            "Change the rules".to_string(),
            None,
            Some(DEFINITION_REPO.to_string()),
            true,
        )
        .await
        .expect("a run on the definition repository");
    let elsewhere = service
        .create_run(
            teo(),
            "change".to_string(),
            "Change the app".to_string(),
            None,
            Some("https://github.com/macro/app".to_string()),
            true,
        )
        .await
        .expect("a run on another repository");

    let guarded = on_definition.run.control_file_patterns.expect("patterns");
    assert_eq!(guarded[0], "ci/**");
    for pattern in crate::domain::definition::DEFINITION_FILE_PATTERNS {
        assert!(guarded.iter().any(|kept| kept == pattern), "{pattern}");
    }
    assert_eq!(
        elsewhere.run.control_file_patterns,
        Some(vec!["ci/**".to_string()])
    );
}

/// A Run of a Definition's Flow that asks holds a place in its owner's
/// Queue before its first Gate, by the Flow it kept: the Flows the factory
/// knows by name never had it.
#[tokio::test]
async fn a_definition_run_that_asks_holds_its_queue_place() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    asking_definition(&github, FIRST);
    let service = code_service(&repo, &workflows, &github);

    let started = service
        .create_run(
            teo(),
            "change".to_string(),
            "Change it".to_string(),
            None,
            Some("https://github.com/macro/app".to_string()),
            true,
        )
        .await
        .expect("a run");
    let starting = service.list_starting_runs(&teo()).await.expect("starting");

    assert_eq!(
        starting
            .iter()
            .map(|detail| detail.run.id)
            .collect::<Vec<_>>(),
        [started.run.id]
    );
}

/// The settings say whether the Definition's branch takes changes only by
/// pull request, and that the team's own rules and Control Files apply to
/// no Run while it is named.
#[tokio::test]
async fn the_settings_say_what_the_definition_decides() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    let service = code_service(&repo, &workflows, &github);

    github.state().requires_pull_request = Some(false);
    let unprotected = service.definition(&julia()).await.expect("settings");
    github.state().requires_pull_request = Some(true);
    let protected = service.definition(&julia()).await.expect("settings");
    let control_files = service.control_files(&julia()).await.expect("settings");
    let rules = service.standing_rules(&julia()).await.expect("settings");

    assert_eq!(unprotected.requires_pull_request, Some(false));
    assert_eq!(protected.requires_pull_request, Some(true));
    assert!(control_files.definition_named && !control_files.can_edit);
    assert!(rules.definition_named && !rules.can_edit);
}
