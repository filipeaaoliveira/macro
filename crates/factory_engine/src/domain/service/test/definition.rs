//! A team's Factory Definition: the Runs of its members start of its Flows,
//! under its rules, and keep the commit they read it at.

use std::sync::Arc;

use factory_records::domain::evaluation::{AgentChoices, BENCHMARK_FLOW};
use factory_records::domain::models::{
    AgentName, EntryId, Role, RunDefinition, RunOutcome, StepKey, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource};
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::agents::AgentDefinition;
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::definition::DefinitionError;
use crate::domain::evaluation::EvaluationDefinition;
use crate::domain::evaluation::insights::{EvaluationService, EvaluationServiceImpl, SuiteError};
use crate::domain::evaluation::service::ScoringServiceImpl;
use crate::domain::fakes::{FakeGithub, FakeJudge, FakeRepo, FakeWatches};
use crate::domain::flow::StepName;
use crate::domain::insights::fakes::FakeUsage;
use crate::domain::models::{FactoryError, StepRequest};
use crate::domain::ports::{FactoryService, ScoringService, StepService};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::service::StepServiceImpl;
use crate::outbound::local_agent::LocalAgent;

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

    github.state().file_reads.clear();
    let named = service
        .set_definition(julia(), Some(DEFINITION_REPO.to_string()))
        .await
        .expect("named");
    let manifests = github
        .state()
        .file_reads
        .iter()
        .filter(|(_, path)| path.as_str() == "factory.json")
        .count();
    assert_eq!(manifests, 1, "the definition is read once");
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
            "Update rules.json".to_string(),
            None,
            Some("https://github.com/Macro/Factory-Definition".to_string()),
            true,
        )
        .await
        .expect("a run on the definition repository, named in another case");
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

    // Published through the worker's Steps, the Run's change to the file
    // its request names in person is Tampered all the same.
    github.state().changed = Some(vec!["rules.json".to_string()]);
    let worker = StepServiceImpl::new(
        repo.clone(),
        BuiltInSteps::new(
            LocalAgent,
            PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
        ),
    );
    let publish = StepRequest {
        key: StepKey {
            run_id: on_definition.run.id,
            seq: 1,
        },
        entry: EntryId::try_from("publish".to_string()).expect("valid"),
        step: StepName::try_from("publish".to_string()).expect("valid"),
        role: None,
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    };
    worker.run_step(publish).await.expect("published");
    let kept = repo
        .control_file_changes
        .lock()
        .expect("control files")
        .get(&on_definition.run.id)
        .cloned()
        .expect("control files kept");
    assert_eq!(kept.tampered, vec!["rules.json".to_string()]);
    assert!(kept.named.is_empty());
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

/// A file GitHub will not give, as one over the 1 MB its contents API
/// gives, is the Definition's to fix: the Run is refused saying which
/// file, not failed as an outage.
#[tokio::test]
async fn a_file_github_will_not_give_is_the_definition_s_problem() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    github
        .state()
        .refused_files
        .insert("agents.json".to_string());
    let service = code_service(&repo, &workflows, &github);

    let refused = service
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
            &refused,
            Err(FactoryError::InvalidDefinition(DefinitionError::TooLarge(path))) if path == "agents.json"
        ),
        "{refused:?}"
    );
}

/// GitHub follows a renamed or transferred repository: a Definition named
/// by its old name is refused, saying where it is now.
#[tokio::test]
async fn a_definition_repository_github_moved_is_named_where_it_is() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    github.state().full_name = Some("macro/rules".to_string());
    let service = code_service(&repo, &workflows, &github);

    let moved = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Hi".to_string(),
            None,
            None,
            true,
        )
        .await;
    github.state().full_name = Some("Macro/Factory-Definition".to_string());
    let same = service
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
            &moved,
            Err(FactoryError::InvalidDefinition(DefinitionError::Moved(now))) if now == "macro/rules"
        ),
        "{moved:?}"
    );
    assert!(same.is_ok(), "{same:?}");
}

/// A Definition's `scorers.json` decides its Runs' Scorers and arms, as it
/// held them when each Run started: its Experiment splits its Flow's Runs,
/// and its Scorer judges them, never the deployment's. Once the team names
/// no Definition, the deployment's Scorers judge its Runs and its
/// Experiments split them.
#[tokio::test]
async fn a_definition_s_scorers_judge_its_runs_and_the_deployment_s_the_others() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    github.state().files.insert(
        (FIRST.to_string(), "scorers.json".to_string()),
        r#"{
            "scorers": [{
                "name": "reads-well", "instructions": "Do the notes read well?",
                "flows": ["notes"], "samplePercent": 100
            }],
            "experiments": [{
                "name": "notes-writer", "flow": "notes",
                "arms": [{ "name": "usual" }, { "name": "again", "agents": { "writer": "primary" } }]
            }]
        }"#
        .to_string(),
    );
    let deployment: EvaluationDefinition = serde_json::from_value(serde_json::json!({
        "scorers": [{
            "name": "deployment-wide", "instructions": "Is it right?", "samplePercent": 100
        }],
        "experiments": [{
            "name": "plan-writer", "flow": "plan",
            "arms": [{ "name": "usual" }, { "name": "other", "weight": 1 }]
        }]
    }))
    .expect("the deployment's definition");
    let deployment = Arc::new(deployment);
    let service = code_service(&repo, &workflows, &github).with_evaluation(deployment.clone());
    let task_id = Uuid::from_u128(7);
    repo.tasks.lock().expect("tasks").insert(task_id);

    let defined = service
        .create_run(
            teo(),
            "notes".to_string(),
            "Note it".to_string(),
            Some(task_id),
            None,
            true,
        )
        .await
        .expect("a run of the definition's flow");
    repo.end_run(defined.run.id, RunOutcome::Success)
        .await
        .expect("end");
    repo.team_definitions
        .lock()
        .expect("team definitions")
        .remove(&TEAM);
    let plain = service
        .create_run(
            teo(),
            "plan".to_string(),
            "Plan it".to_string(),
            Some(task_id),
            None,
            true,
        )
        .await
        .expect("a run of a built-in flow");
    repo.end_run(plain.run.id, RunOutcome::Success)
        .await
        .expect("end");

    let judge = Arc::new(FakeJudge::default());
    judge.then(0.8);
    judge.then(0.6);
    let scoring = ScoringServiceImpl::new(
        repo.clone(),
        judge,
        FakeUsage::new(Vec::new()),
        github.clone(),
        deployment,
    );
    scoring
        .score_run(defined.run.id, false)
        .await
        .expect("scored");
    scoring
        .score_run(plain.run.id, false)
        .await
        .expect("scored");

    let kept = repo.evaluation.lock().expect("evaluation").clone();
    let scorers = |run_id| -> Vec<String> {
        kept.run_scores[&run_id]
            .iter()
            .map(|judgment| judgment.scorer.clone())
            .collect()
    };
    assert_eq!(scorers(defined.run.id), ["reads-well"]);
    assert_eq!(scorers(plain.run.id), ["deployment-wide"]);
    let experiments = |run_id| -> Vec<String> {
        kept.arms[&run_id]
            .iter()
            .map(|arm| arm.experiment.clone())
            .collect()
    };
    assert_eq!(experiments(defined.run.id), ["notes-writer"]);
    assert_eq!(experiments(plain.run.id), ["plan-writer"]);
}

/// A team's Definition holds its Benchmarks: a suite of one is checked
/// against the Definition's Agents, never the deployment's, and starts its
/// trials on the factory's benchmark Flow, which the Definition does not
/// name, each keeping the Definition it started under.
#[tokio::test]
async fn a_suite_of_a_definition_s_benchmark_walks_the_benchmark_flow() {
    let repo = team_repo();
    let github = github_at(FIRST);
    definition_files(&github, FIRST, "Notes.", None);
    {
        let mut state = github.state();
        state.files.insert(
            (FIRST.to_string(), "agents.json".to_string()),
            r#"{
                "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
                "roles": { "planner": ["primary"], "writer": ["primary"] }
            }"#
            .to_string(),
        );
        state.files.insert(
            (FIRST.to_string(), "scorers.json".to_string()),
            r#"{
                "benchmarks": [{
                    "name": "core", "repository": "https://github.com/macro/widgets",
                    "tasks": [{ "name": "typo", "request": "Fix the typo." }]
                }]
            }"#
            .to_string(),
        );
    }
    let deployment_agents: AgentDefinition = serde_json::from_value(serde_json::json!({
        "agents": { "elsewhere": { "bot": "00000000-0000-0000-0000-00000000c0de" } },
        "roles": { "planner": ["elsewhere"], "writer": ["elsewhere"] }
    }))
    .expect("the deployment's agents");
    let suites = EvaluationServiceImpl::new(
        repo.clone(),
        Arc::new(code_service(&repo, &FakeWorkflows::default(), &github)),
        Arc::new(EvaluationDefinition::default()),
        Some(deployment_agents),
    );
    let writer = |agent: &str| -> AgentChoices {
        [(
            Role::try_from("writer".to_string()).expect("valid"),
            AgentName::try_from(agent.to_string()).expect("valid"),
        )]
        .into_iter()
        .collect()
    };

    let refused = suites.start_suite(teo(), "core", writer("elsewhere")).await;
    assert!(
        matches!(refused, Err(SuiteError::Configuration(_))),
        "{refused:?}"
    );
    let started = suites
        .start_suite(teo(), "core", writer("primary"))
        .await
        .expect("a suite of the definition's benchmark");

    let trial = started.runs["typo"];
    assert_eq!(
        repo.flows.lock().expect("flows")[&trial].as_str(),
        BENCHMARK_FLOW
    );
    let kept = repo.get_run(trial).await.expect("read").expect("the trial");
    assert_eq!(
        kept.definition,
        Some(RunDefinition {
            repo_url: DEFINITION_REPO.to_string(),
            commit: FIRST.to_string(),
        })
    );
}
