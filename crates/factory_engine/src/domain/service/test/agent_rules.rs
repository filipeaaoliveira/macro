//! A Run of a Definition with Agent Rules keeps, as it starts, the rule
//! that chose each Role's first Agent: one its task's tags or its
//! repository matched, past the Roles an Experiment arm chose for. The Run
//! API shows them.

use factory_records::domain::evaluation::RunAgentRule;
use factory_records::domain::models::{AgentName, Role};
use serde_json::json;
use uuid::Uuid;

use super::definition::{FIRST, github_at, team_repo};
use super::{FakeWorkflows, code_service, teo};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::RunDetail;
use crate::domain::ports::FactoryService;

const APP: &str = "https://github.com/macro/app";
const SITE: &str = "https://github.com/Macro/Site";

/// A Flow on a repository whose one Step replies: the Run's Agents are
/// read by its Steps, which these tests leave out.
const NOTES: &str = r#"{
    "repository": true,
    "entries": [{
        "id": "reply", "step": "reply",
        "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
    }]
}"#;

/// A Definition of the `notes` Flow whose task tagged `docs` is written by
/// `docs-writer` first, and whose `macro/site` Runs are planned by
/// `backup` first, with `scorers` as its `scorers.json`.
fn definition(github: &FakeGithub, scorers: Option<&str>) {
    let agents = r#"{
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0d1" },
            "docs-writer": { "bot": "00000000-0000-0000-0000-00000000d0c5" }
        },
        "roles": {
            "planner": ["primary", "backup"], "writer": ["primary", "backup", "docs-writer"],
            "reviewer": ["backup"], "fixer": ["primary"]
        },
        "rules": [
            { "name": "docs", "role": "writer", "tag": "docs", "agent": "docs-writer" },
            { "name": "site", "role": "planner", "repository": "macro/site", "agent": "backup" }
        ]
    }"#;
    let mut files = vec![
        ("factory.json", r#"{ "version": 1, "flows": ["notes"] }"#),
        ("flows/notes.json", NOTES),
        ("agents.json", agents),
    ];
    if let Some(scorers) = scorers {
        files.push(("scorers.json", scorers));
    }
    let mut state = github.state();
    for (path, text) in files {
        state
            .files
            .insert((FIRST.to_string(), path.to_string()), text.to_string());
    }
}

fn agent_rule(role: &str, rule: &str, agent: &str) -> RunAgentRule {
    RunAgentRule {
        role: Role::try_from(role.to_string()).expect("valid role"),
        rule: rule.to_string(),
        agent: AgentName::try_from(agent.to_string()).expect("valid agent"),
    }
}

/// Start a `notes` Run of Teo's on `repo_url`, on a task tagged `tags`.
async fn start(repo: &FakeRepo, github: &FakeGithub, repo_url: &str, tags: &[&str]) -> RunDetail {
    let workflows = FakeWorkflows::default();
    let task_id = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task_id);
    repo.task_tags
        .lock()
        .expect("task tags")
        .insert(task_id, tags.iter().map(ToString::to_string).collect());
    let detail = code_service(repo, &workflows, github)
        .create_run(
            teo(),
            "notes".to_string(),
            "Document the export".to_string(),
            Some(task_id),
            Some(repo_url.to_string()),
            true,
            None,
        )
        .await
        .expect("a run of the definition's flow");
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
    detail
}

/// A Run whose task is tagged `docs` keeps the `docs` rule for its writer,
/// one on `macro/site` the `site` rule for its planner, whatever the case
/// of the repository's name, and one neither matches keeps none, so its
/// Roles keep their own order. The Run API lists the rules each kept.
#[tokio::test]
async fn a_run_keeps_the_agent_rules_its_task_tags_and_repository_match() {
    let repo = team_repo();
    let github = github_at(FIRST);
    definition(&github, None);

    let docs = start(&repo, &github, APP, &["Docs", "urgent"]).await;
    let site = start(&repo, &github, SITE, &["docs"]).await;
    let neither = start(&repo, &github, APP, &["urgent"]).await;

    assert_eq!(
        docs.agent_rules,
        [agent_rule("writer", "docs", "docs-writer")]
    );
    assert_eq!(
        site.agent_rules,
        [
            agent_rule("planner", "site", "backup"),
            agent_rule("writer", "docs", "docs-writer"),
        ]
    );
    assert_eq!(neither.agent_rules, []);
    // Tags that were read, matched or not, are not unread.
    assert!(!docs.task_tags_unread && !neither.task_tags_unread);
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert!(!kept.agent_rules.contains_key(&neither.run.id));

    // The Run API reads them back, as `agentRules`.
    let service = code_service(&repo, &FakeWorkflows::default(), &github);
    let read = service.get_run(&teo(), site.run.id).await.expect("read");
    assert_eq!(read.agent_rules, site.agent_rules);
    assert_eq!(
        serde_json::to_value(&read).expect("json")["agentRules"],
        json!([
            { "role": "planner", "rule": "site", "agent": "backup" },
            { "role": "writer", "rule": "docs", "agent": "docs-writer" }
        ])
    );
    let task_id = docs.run.task_id.expect("a task");
    let listed = service
        .list_task_runs(&teo(), task_id)
        .await
        .expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].agent_rules, docs.agent_rules);
}

/// A Role an Experiment arm chooses for takes no rule: the arm's choice
/// comes first, so the Experiment compares what it set out to.
#[tokio::test]
async fn an_arm_s_role_takes_no_agent_rule() {
    let repo = team_repo();
    let github = github_at(FIRST);
    // Both arms choose the writer, so the Run's arm does whatever its task.
    definition(
        &github,
        Some(
            r#"{ "experiments": [{
                "name": "writer-agent", "flow": "notes",
                "arms": [
                    { "name": "backup", "agents": { "writer": "backup" } },
                    { "name": "primary", "agents": { "writer": "primary" } }
                ]
            }] }"#,
        ),
    );

    let run = start(&repo, &github, SITE, &["docs"]).await;

    assert_eq!(run.agent_rules, [agent_rule("planner", "site", "backup")]);
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert_eq!(kept.arms[&run.run.id].len(), 1);
}

/// A task whose tags cannot be read is taken to carry none: no rule on a
/// tag matches, a rule on the repository still does, and the Run starts.
#[tokio::test]
async fn a_task_whose_tags_cannot_be_read_matches_no_tag_rule() {
    let repo = team_repo();
    let github = github_at(FIRST);
    definition(&github, None);
    repo.fail_once_after("task_tags", 0);

    let run = start(&repo, &github, SITE, &["docs"]).await;

    assert_eq!(run.agent_rules, [agent_rule("planner", "site", "backup")]);
    // The Run API says the tags went unread.
    assert!(run.task_tags_unread);
    let service = code_service(&repo, &FakeWorkflows::default(), &github);
    let read = service.get_run(&teo(), run.run.id).await.expect("read");
    assert!(read.task_tags_unread);
    assert_eq!(
        serde_json::to_value(&read).expect("json")["taskTagsUnread"],
        json!(true)
    );
    let task_id = run.run.task_id.expect("a task");
    let listed = service
        .list_task_runs(&teo(), task_id)
        .await
        .expect("listed");
    assert!(listed[0].task_tags_unread);
}

/// A Run whose rules cannot be kept does not start.
#[tokio::test]
async fn a_run_whose_agent_rules_are_not_kept_does_not_start() {
    let repo = team_repo();
    let github = github_at(FIRST);
    definition(&github, None);
    repo.fail_once_after("record_agent_rules", 0);
    let workflows = FakeWorkflows::default();
    let task_id = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task_id);

    let refused = code_service(&repo, &workflows, &github)
        .create_run(
            teo(),
            "notes".to_string(),
            "Document the export".to_string(),
            Some(task_id),
            Some(APP.to_string()),
            true,
            None,
        )
        .await;

    assert!(refused.is_err(), "{refused:?}");
    assert!(workflows.started.lock().expect("started").is_empty());
    assert!(repo.runs.lock().expect("runs").is_empty());
}
