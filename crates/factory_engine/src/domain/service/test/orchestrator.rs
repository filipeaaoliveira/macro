//! The Orchestrator: an agent session answering a Gate for the Run's owner,
//! which answers only what a Standing Rule could, and the Queue limit,
//! above which no new Run starts.

use chrono::Utc;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    AgentName, ControlFileChanges, EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert,
    StepKey, StepSession, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RunRules};
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::{Answerer, FactoryError, PersonOnly};
use crate::domain::ports::FactoryService;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn base() -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    }
}

/// An agent session in Macro chat, not one a factory Step works in.
fn orchestrator() -> Answerer {
    Answerer::Agent {
        session_id: Uuid::now_v7(),
    }
}

/// A `code` Run of Julia's whose Step 1, at `entry`, ended `ended` and
/// asks `answers` on a Gate.
async fn waiting_at(repo: &FakeRepo, entry: &str, ended: &str, answers: &[&str]) -> RunId {
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: None,
            repository: Some(&base()),
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let key = StepKey {
        run_id: run.id,
        seq: 1,
    };
    let entry = EntryId::try_from(entry.to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &outcome(ended), "Step output")
        .await
        .expect("end");
    let answers: Vec<Outcome> = answers.iter().map(|answer| outcome(answer)).collect();
    repo.open_gate(key, "Go on?", &answers).await.expect("open");
    run.id
}

/// An agent session answers a Flow's Gate of a kind for the Run's owner,
/// as the owner's answer: a plan, a review's Blockers, and a merge with
/// no Main Hold.
#[tokio::test]
async fn an_agent_answers_a_gate_a_standing_rule_could() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());

    for (entry, ended, answers, answer) in [
        ("plan", "done", ["approve", "revise"], "approve"),
        ("review", "changes", ["fix", "stop"], "fix"),
        ("merge-gate", "done", ["approve", "revise"], "approve"),
    ] {
        let run_id = waiting_at(&repo, entry, ended, &answers).await;
        service
            .answer_gate(
                julia(),
                run_id,
                1,
                outcome(answer),
                None,
                None,
                orchestrator(),
            )
            .await
            .unwrap_or_else(|error| panic!("{entry}: {error}"));
    }

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 3);
    assert!(answers.iter().all(|answer| answer.user_id == julia()));
}

/// What the factory keeps for the owner in person stays refused to an
/// agent answering for them, before the workflow hears of it: an agent's
/// question, a Tampered Run's approval, Control Files sent with an
/// answer, a merge Gate while its base has a Main Hold, a Gate of an
/// entry no Standing Rule decides, and any answer from a factory Step's
/// own session.
#[tokio::test]
async fn an_agent_never_answers_what_is_kept_for_a_person() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());

    let question = waiting_at(&repo, "write", "question", &["answer"]).await;
    let refused = service
        .answer_gate(
            julia(),
            question,
            1,
            Outcome::answer(),
            Some("Use the new table".to_string()),
            None,
            orchestrator(),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::ReservedForAPerson(PersonOnly::Question))
        ),
        "{refused:?}"
    );

    let tampered = waiting_at(&repo, "merge-gate", "done", &["approve", "revise"]).await;
    let changes = ControlFileChanges {
        tampered: vec![".github/workflows/ci.yml".to_string()],
        named: Vec::new(),
        unlisted: false,
    };
    repo.set_control_file_changes(tampered, Some(&changes))
        .await
        .expect("keep");
    for seen in [None, Some(changes.tampered.clone())] {
        let refused = service
            .answer_gate(
                julia(),
                tampered,
                1,
                outcome("approve"),
                None,
                seen,
                orchestrator(),
            )
            .await;
        assert!(
            matches!(
                refused,
                Err(FactoryError::ReservedForAPerson(PersonOnly::ControlFiles))
            ),
            "{refused:?}"
        );
    }

    let held = waiting_at(&repo, "merge-gate", "done", &["approve", "revise"]).await;
    repo.hold_main(&base(), "abc123", "ci", Utc::now())
        .await
        .expect("hold");
    let refused = service
        .answer_gate(
            julia(),
            held,
            1,
            outcome("approve"),
            None,
            None,
            orchestrator(),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::ReservedForAPerson(PersonOnly::MainHold))
        ),
        "{refused:?}"
    );

    // `ci` asks nothing in the built-in Flows: no rule could decide a
    // Gate there.
    let unkinded = waiting_at(&repo, "ci", "done", &["approve", "revise"]).await;
    let refused = service
        .answer_gate(
            julia(),
            unkinded,
            1,
            outcome("approve"),
            None,
            None,
            orchestrator(),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::ReservedForAPerson(PersonOnly::PersonsGate))
        ),
        "{refused:?}"
    );

    // A planner's session works as the owner too: it never approves the
    // plan it wrote.
    let plan = waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from("primary".to_string()).expect("valid")),
        bot: None,
    };
    let session_id = session.session_id;
    let planner = Role::try_from("planner".to_string()).expect("valid");
    repo.claim_session(
        StepKey {
            run_id: plan,
            seq: 1,
        },
        session,
        &planner,
        &[Outcome::done()],
    )
    .await
    .expect("claim");
    let refused = service
        .answer_gate(
            julia(),
            plan,
            1,
            outcome("approve"),
            None,
            None,
            Answerer::Agent { session_id },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::ReservedForAPerson(PersonOnly::StepSession))
        ),
        "{refused:?}"
    );

    assert!(workflows.answers.lock().expect("answers").is_empty());

    // The owner in person still answers each of them.
    service
        .answer_gate(
            julia(),
            held,
            1,
            outcome("approve"),
            None,
            None,
            Answerer::InPerson,
        )
        .await
        .expect("the owner answers a held merge gate");
    service
        .answer_gate(
            julia(),
            question,
            1,
            Outcome::answer(),
            Some("Use the new table".to_string()),
            None,
            Answerer::InPerson,
        )
        .await
        .expect("the owner answers a question");
    assert_eq!(workflows.answers.lock().expect("answers").len(), 2);
}

/// Julia's team, which she administers, with `definition` as its
/// Standing Rules.
fn julia_s_team(repo: &FakeRepo, definition: OrchestratorDefinition) -> Uuid {
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        julia().to_string(),
        TeamMembership {
            team_id,
            admin: true,
        },
    );
    repo.team_rules
        .lock()
        .expect("rules")
        .insert(team_id, definition);
    team_id
}

/// Above the Queue limit no new Run starts, whoever asks, and says why;
/// a Run already going goes on, and once the Queue is back at its limit
/// Runs start again. A Project's own limit replaces the team's.
#[tokio::test]
async fn above_the_queue_limit_no_new_run_starts() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let team_id = julia_s_team(
        &repo,
        OrchestratorDefinition {
            queue_limit: Some(1),
            ..OrchestratorDefinition::DEFAULT
        },
    );
    let start = || {
        service.create_run(
            julia(),
            "reply".to_string(),
            "Say hello".to_string(),
            None,
            None,
            false,
        )
    };

    let first = waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    start().await.expect("one waits: at the limit, not above");

    waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    let refused = start().await;
    assert!(
        matches!(refused, Err(FactoryError::QueueFull { limit: 1 })),
        "{refused:?}"
    );
    let started = workflows.started.lock().expect("started").len();
    assert_eq!(started, 1);

    // The Runs under way go on: their Gates still take answers.
    service
        .answer_gate(
            julia(),
            first,
            1,
            outcome("approve"),
            None,
            None,
            Answerer::InPerson,
        )
        .await
        .expect("a waiting run goes on");
    repo.answer_gate(
        StepKey {
            run_id: first,
            seq: 1,
        },
        &outcome("approve"),
        None,
        &julia(),
        None,
    )
    .await
    .expect("recorded");
    start().await.expect("back at the limit");

    // A Project's own rules, with no limit, replace the team's.
    let project = Uuid::now_v7();
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    repo.task_projects
        .lock()
        .expect("projects")
        .insert(task, project);
    repo.project_rules
        .lock()
        .expect("rules")
        .insert((team_id, project), OrchestratorDefinition::DEFAULT);
    waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    assert!(matches!(
        start().await,
        Err(FactoryError::QueueFull { limit: 1 })
    ));
    service
        .create_run(
            julia(),
            "reply".to_string(),
            "Say hello".to_string(),
            Some(task),
            None,
            false,
        )
        .await
        .expect("the project sets no limit");
}

/// A Queue limit above the highest one is refused before anything is
/// kept.
#[tokio::test]
async fn a_queue_limit_above_the_highest_is_refused() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    julia_s_team(&repo, OrchestratorDefinition::DEFAULT);

    let refused = service
        .set_standing_rules(
            julia(),
            Some(OrchestratorDefinition {
                queue_limit: Some(1001),
                ..OrchestratorDefinition::DEFAULT
            }),
        )
        .await;

    assert!(
        matches!(refused, Err(FactoryError::InvalidQueueLimit { max: 1000 })),
        "{refused:?}"
    );
}
