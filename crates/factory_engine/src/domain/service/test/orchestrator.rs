//! The Orchestrator: an agent session in Macro chat answering a Gate for
//! the Run's owner, which may always send work back, approves only what
//! the Run's Orchestrator Definition lets it, and never approves a merge;
//! and the Queue limit, above which no new Run starts.

use chrono::Utc;
use factory_records::domain::gate_replies::GateReplyRepository as _;
use factory_records::domain::models::{
    AgentName, ControlFileChanges, EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert,
    RunOutcome, StandInAnswer, StepKey, StepSession, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, OrchestratorKinds, RulesSource, RunRules,
};
use uuid::Uuid;

use super::gate_owner::{blocker, second_round_finding};
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

/// The team's rules, with the Orchestrator allowed to approve `plan` and
/// send `blockers` to a fix as given, and the owner section `owner_gates`.
fn rules(plan: bool, blockers: bool, owner_gates: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            owner_gates,
            orchestrator: OrchestratorKinds { plan, blockers },
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// A `code` Run of Julia's with `rules`, whose Step 1, at `entry` on its
/// `visit`, ended `ended` and asks `answers` on a Gate.
async fn waiting_with(
    repo: &FakeRepo,
    rules: RunRules,
    entry: &str,
    visit: u16,
    ended: &str,
    answers: &[&str],
) -> RunId {
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
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
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
    repo.start_step(key, &entry, visit).await.expect("start");
    repo.end_step(key, &outcome(ended), "Step output")
        .await
        .expect("end");
    let answers: Vec<Outcome> = answers.iter().map(|answer| outcome(answer)).collect();
    repo.open_gate(key, "Go on?", &answers).await.expect("open");
    run.id
}

/// A Run with the default rules, waiting at `entry`'s Gate on its first
/// visit.
async fn waiting_at(repo: &FakeRepo, entry: &str, ended: &str, answers: &[&str]) -> RunId {
    waiting_with(repo, RunRules::DEFAULT, entry, 1, ended, answers).await
}

/// What `service` says to an agent answering `answer` on Step 1 of `run`.
async fn agent_answers(
    service: &impl FactoryService,
    run: RunId,
    answer: &str,
) -> Result<(), FactoryError> {
    service
        .answer_gate(julia(), run, 1, outcome(answer), None, None, orchestrator())
        .await
}

/// The Orchestrator may always send work back; it gives the approving
/// answer to a plan or Blockers Gate only where the Run's definition lets
/// it, and never approves a merge. Its answers reach the workflow with
/// its session, so they are recorded as its own.
#[tokio::test]
async fn an_agent_sends_work_back_and_approves_only_what_the_rules_let_it() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());

    for (entry, ended, answers, back, approving) in [
        ("plan", "done", ["approve", "revise"], "revise", "approve"),
        ("review", "changes", ["fix", "stop"], "stop", "fix"),
        (
            "merge-gate",
            "done",
            ["approve", "revise"],
            "revise",
            "approve",
        ),
    ] {
        let off = waiting_at(&repo, entry, ended, &answers).await;
        agent_answers(&service, off, back)
            .await
            .unwrap_or_else(|error| panic!("{entry} sent back: {error}"));
        let refused = agent_answers(&service, off, approving).await;
        let expected = if entry == "merge-gate" {
            PersonOnly::Merge
        } else {
            PersonOnly::NotAllowed
        };
        assert!(
            matches!(refused, Err(FactoryError::ReservedForAPerson(why)) if why == expected),
            "{entry}: {refused:?}"
        );
    }

    let on = rules(true, true, false);
    let plan = waiting_with(&repo, on, "plan", 1, "done", &["approve", "revise"]).await;
    agent_answers(&service, plan, "approve")
        .await
        .expect("the rules let it approve the plan");
    let review = waiting_with(&repo, on, "review", 1, "changes", &["fix", "stop"]).await;
    agent_answers(&service, review, "fix")
        .await
        .expect("the rules let it send blockers to a fix");
    let merge = waiting_with(&repo, on, "merge-gate", 1, "done", &["approve", "revise"]).await;
    let refused = agent_answers(&service, merge, "approve").await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::ReservedForAPerson(PersonOnly::Merge))
        ),
        "{refused:?}"
    );

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 5);
    assert!(answers.iter().all(|answer| answer.user_id == julia()));
    assert!(answers.iter().all(|answer| answer.agent_session.is_some()));
}

/// What the factory keeps for the owner in person stays refused to an
/// agent answering for them, before the workflow hears of it, whatever the
/// rules let it approve: an agent's question, the Gate of an agent that
/// stopped without a result, a Tampered Run's plan,
/// Control Files sent with an answer, a Gate of an entry no Standing Rule
/// decides, Blockers past the rule's fix rounds or found again after a
/// fix, a Gate answered since the agent read it, and any answer from a
/// factory Step's own session.
#[tokio::test]
async fn an_agent_never_answers_what_is_kept_for_a_person() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let on = rules(true, true, true);
    let refused_for = |result: Result<(), FactoryError>, expected: PersonOnly| {
        assert!(
            matches!(result, Err(FactoryError::ReservedForAPerson(why)) if why == expected),
            "{expected:?}: {result:?}"
        );
    };

    let question = waiting_with(&repo, on, "write", 1, "question", &["answer"]).await;
    let asked = service
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
    refused_for(asked, PersonOnly::Question);

    // The Gate of an agent that stopped without a result is the owner's,
    // whatever answers it offers.
    let stopped = waiting_with(&repo, on, "write", 1, "no_result", &["retry", "stop"]).await;
    refused_for(
        agent_answers(&service, stopped, "stop").await,
        PersonOnly::PersonsGate,
    );
    let stopped_at_plan =
        waiting_with(&repo, on, "plan", 1, "no_result", &["approve", "revise"]).await;
    refused_for(
        agent_answers(&service, stopped_at_plan, "revise").await,
        PersonOnly::PersonsGate,
    );

    let tampered = waiting_with(&repo, on, "plan", 1, "done", &["approve", "revise"]).await;
    let changes = ControlFileChanges {
        tampered: vec![".github/workflows/ci.yml".to_string()],
        named: Vec::new(),
        unlisted: false,
    };
    repo.set_control_file_changes(tampered, Some(&changes))
        .await
        .expect("keep");
    refused_for(
        agent_answers(&service, tampered, "approve").await,
        PersonOnly::ControlFiles,
    );
    let sent = service
        .answer_gate(
            julia(),
            tampered,
            1,
            outcome("revise"),
            None,
            Some(changes.tampered.clone()),
            orchestrator(),
        )
        .await;
    refused_for(sent, PersonOnly::ControlFiles);

    // `ci` asks nothing in the built-in Flows: no rule could decide a
    // Gate there.
    let unkinded = waiting_with(&repo, on, "ci", 1, "done", &["approve", "revise"]).await;
    refused_for(
        agent_answers(&service, unkinded, "revise").await,
        PersonOnly::PersonsGate,
    );

    // The `code` Flow's review panel runs three rounds: the rule sends at
    // most two to a fix in a row, so the third round's Blockers are the
    // owner's.
    let third = waiting_with(&repo, on, "review", 3, "changes", &["fix", "stop"]).await;
    refused_for(
        agent_answers(&service, third, "fix").await,
        PersonOnly::OwnersBlockers,
    );
    agent_answers(&service, third, "stop")
        .await
        .expect("stopping is always the agent's to say");

    // Blockers found again, unchanged, after the rule's fix.
    let same = blocker(3, "correctness-reviewer", "src/a.rs", (12, 16));
    let repeated = second_round_finding(&repo, on, same).await;
    let again = service
        .answer_gate(
            julia(),
            repeated.run_id,
            repeated.seq,
            outcome("fix"),
            None,
            None,
            orchestrator(),
        )
        .await;
    refused_for(again, PersonOnly::OwnersBlockers);

    // A planner's session works as the owner too: it never approves the
    // plan it wrote.
    let plan = waiting_with(&repo, on, "plan", 1, "done", &["approve", "revise"]).await;
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
    let own = service
        .answer_gate(
            julia(),
            plan,
            1,
            outcome("revise"),
            None,
            None,
            Answerer::Agent { session_id },
        )
        .await;
    refused_for(own, PersonOnly::StepSession);

    // The agent decided on a Gate the owner answered meanwhile.
    let raced = waiting_with(&repo, on, "plan", 1, "done", &["approve", "revise"]).await;
    repo.answer_gate(
        StepKey {
            run_id: raced,
            seq: 1,
        },
        &outcome("revise"),
        None,
        &julia(),
        None,
        None,
    )
    .await
    .expect("the owner answers first");
    let late = agent_answers(&service, raced, "approve").await;
    assert!(
        matches!(late, Err(FactoryError::AnswerRefused(_))),
        "{late:?}"
    );

    // `stop` on the third round went through; nothing else did.
    assert_eq!(workflows.answers.lock().expect("answers").len(), 1);

    // The owner in person still answers what the agent may not.
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
    service
        .answer_gate(
            julia(),
            third,
            1,
            outcome("fix"),
            None,
            None,
            Answerer::InPerson,
        )
        .await
        .expect("the owner sends blockers to another fix");
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

/// At the Queue limit no new Run starts, whoever asks, and says why: a
/// limit of 2 lets a person hold at most 2 places. A Run just started
/// holds the place it will ask for; a Run already going goes on, and once
/// the Queue is back under its limit Runs start again. A Project's own
/// limit replaces the team's.
#[tokio::test]
async fn at_the_queue_limit_no_new_run_starts() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let team_id = julia_s_team(
        &repo,
        OrchestratorDefinition {
            queue_limit: Some(2),
            ..OrchestratorDefinition::DEFAULT
        },
    );
    let start_flow = |flow: &str| {
        service.create_run(
            julia(),
            flow.to_string(),
            "Plan it".to_string(),
            None,
            None,
            false,
        )
    };

    let start = || start_flow("plan");
    // A Run of a Flow with no Gate holds no place while it runs.
    start_flow("reply").await.expect("a reply run starts");
    start_flow("reply").await.expect("and another");

    let first = waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    let started = start().await.expect("one waits: under the limit").run.id;

    // The Run just started has not asked anything yet, but holds its place.
    let refused = start().await;
    assert!(
        matches!(
            refused,
            Err(FactoryError::QueueFull {
                limit: 2,
                waiting: 1,
                starting: 1,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(workflows.started.lock().expect("started").len(), 3);
    let held = service.list_starting_runs(&julia()).await.expect("listed");
    assert_eq!(
        held.iter().map(|detail| detail.run.id).collect::<Vec<_>>(),
        [started]
    );

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
        None,
    )
    .await
    .expect("recorded");
    let again = start().await.expect("back under the limit").run.id;
    for run in [started, again] {
        repo.end_run(run, RunOutcome::Cancelled)
            .await
            .expect("ended");
    }

    // A Gate the stand-in answered, which the owner may still override,
    // does not wait on them in person.
    let stood_in = waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    repo.stand_in(
        StepKey {
            run_id: stood_in,
            seq: 1,
        },
        &StandInAnswer {
            answer: outcome("approve"),
            reason: "The plan matches the request".to_string(),
            confidence: 95,
            until: Utc::now() + chrono::TimeDelta::minutes(10),
        },
    )
    .await
    .expect("stand in");
    waiting_at(&repo, "plan", "done", &["approve", "revise"]).await;
    start().await.expect("a stood-in gate is not in the queue");

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
    assert!(matches!(
        start().await,
        Err(FactoryError::QueueFull { limit: 2, .. })
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

/// A Queue limit below the lowest or above the highest one is refused
/// before anything is kept: 0 would start no Run. The lowest and highest
/// are kept.
#[tokio::test]
async fn a_queue_limit_out_of_range_is_refused() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let team_id = julia_s_team(&repo, OrchestratorDefinition::DEFAULT);
    let with_limit = |limit: u16| {
        Some(OrchestratorDefinition {
            queue_limit: Some(limit),
            ..OrchestratorDefinition::DEFAULT
        })
    };

    for limit in [0, 1001] {
        let refused = service.set_standing_rules(julia(), with_limit(limit)).await;
        assert!(
            matches!(
                refused,
                Err(FactoryError::InvalidQueueLimit { min: 1, max: 1000 })
            ),
            "{limit}: {refused:?}"
        );
        let refused = service
            .set_project_rules(julia(), Uuid::now_v7(), with_limit(limit))
            .await;
        assert!(
            matches!(refused, Err(FactoryError::InvalidQueueLimit { .. })),
            "{limit}: {refused:?}"
        );
    }
    assert_eq!(
        repo.team_rules.lock().expect("rules").get(&team_id),
        Some(&OrchestratorDefinition::DEFAULT)
    );

    for limit in [1, 1000] {
        service
            .set_standing_rules(julia(), with_limit(limit))
            .await
            .expect("in range");
    }
}

/// The kind of a Gate is its Run's own Flow's: a Definition's Flow whose
/// entry `plan` asks to merge is a merge Gate, which the Orchestrator never
/// approves, whatever a built-in Flow's `plan` entry asks.
#[tokio::test]
async fn an_entry_s_gate_kind_is_read_from_the_run_s_own_flow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run = waiting_with(
        &repo,
        rules(true, true, false),
        "plan",
        1,
        "done",
        &["approve", "revise"],
    )
    .await;
    let merging = serde_json::json!({
        "repository": true,
        "entries": [{
            "id": "plan",
            "step": "ready_to_merge",
            "gate": {
                "kind": "merge",
                "question": "Merge?",
                "answers": ["approve", "revise"]
            },
            "on": {
                "approve": [{ "to": { "kind": "end", "outcome": "success" } }],
                "revise": [{ "to": { "kind": "end", "outcome": "manual" } }]
            }
        }]
    });
    repo.kept_flows
        .lock()
        .expect("kept flows")
        .insert(run, (merging, true));

    let approved = agent_answers(&service, run, "approve").await;

    assert!(
        matches!(
            approved,
            Err(FactoryError::ReservedForAPerson(PersonOnly::Merge))
        ),
        "{approved:?}"
    );
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

/// A list of Runs reads the Flows of those that may be at their merge in
/// one read, not one for each Run.
#[tokio::test]
async fn a_list_of_runs_reads_their_flows_at_once() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    for _ in 0..3 {
        waiting_at(&repo, "merge-gate", "done", &["approve", "revise"]).await;
    }

    let waiting = service.list_waiting_runs(&julia()).await.expect("waiting");

    assert_eq!(waiting.len(), 3);
    assert_eq!(*repo.flow_reads.lock().expect("flow reads"), 0);
}
