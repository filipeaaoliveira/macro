//! A Tampered Run's Gate, answered only with the Control Files the person
//! saw approved, and a team's Control File patterns.

use factory_records::domain::models::{
    ControlFileChanges, Outcome, RunId, StepKey, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{FakeExecutor, FakeWorkflows, julia, julia_s_run, service, teo};
use crate::domain::control_files::{ControlFiles, ControlFilesError};
use crate::domain::fakes::FakeRepo;
use crate::domain::gate::{AnswerGate, GateAnswered};
use crate::domain::models::{Answerer, FactoryError};
use crate::domain::ports::{FactoryService, StepService};
use crate::domain::service::StepServiceImpl;

fn approve() -> Outcome {
    Outcome::try_from("approve".to_string()).expect("valid")
}

fn revise() -> Outcome {
    Outcome::try_from("revise".to_string()).expect("valid")
}

fn files(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

fn changes() -> ControlFileChanges {
    ControlFileChanges {
        tampered: files(&[".github/workflows/ci.yml", "package.json"]),
        named: files(&["Makefile"]),
        unlisted: false,
    }
}

async fn run_changing(repo: &FakeRepo, changes: &ControlFileChanges) -> RunId {
    let run_id = julia_s_run(repo).await;
    repo.set_control_file_changes(run_id, Some(changes))
        .await
        .expect("keep");
    run_id
}

/// Approving a Tampered Run's Gate without the Control Files the person saw,
/// or with a list that is not the Run's, is refused before the workflow
/// hears of it; nothing else can approve them.
#[tokio::test]
async fn approving_a_tampered_run_needs_the_files_it_changed_approved() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = run_changing(&repo, &changes()).await;
    let service = service(&repo, &workflows);

    for seen in [
        None,
        Some(files(&["package.json"])),
        Some(files(&[
            ".github/workflows/ci.yml",
            "package.json",
            "Makefile",
        ])),
    ] {
        let refused = service
            .answer_gate(
                julia(),
                run_id,
                4,
                approve(),
                None,
                seen.clone(),
                Answerer::InPerson,
            )
            .await;

        assert!(
            matches!(refused, Err(FactoryError::ControlFilesNotApproved)),
            "approving {seen:?} is refused"
        );
    }
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

/// An approval of the files the person saw, in any order, goes to the
/// workflow with the Run's Tampered files; sending the Run back approves
/// none, and needs no approval.
#[tokio::test]
async fn an_approval_carries_the_tampered_files_to_the_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = run_changing(&repo, &changes()).await;
    let service = service(&repo, &workflows);
    let seen = files(&["package.json", ".github/workflows/ci.yml"]);

    service
        .answer_gate(
            julia(),
            run_id,
            4,
            approve(),
            None,
            Some(seen),
            Answerer::InPerson,
        )
        .await
        .expect("approve");
    service
        .answer_gate(julia(), run_id, 6, revise(), None, None, Answerer::InPerson)
        .await
        .expect("revise");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(
        answers,
        [
            AnswerGate {
                seq: 4,
                answer: approve(),
                note: None,
                user_id: julia(),
                approved_control_files: Some(changes().tampered),
            },
            AnswerGate {
                seq: 6,
                answer: revise(),
                note: None,
                user_id: julia(),
                approved_control_files: None,
            },
        ]
    );
}

/// A Run whose branch changed only Control Files its request named is not
/// Tampered: approving it records no approval.
#[tokio::test]
async fn a_run_that_is_not_tampered_approves_no_control_files() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let named_only = ControlFileChanges {
        tampered: Vec::new(),
        named: files(&["Makefile"]),
        unlisted: false,
    };
    let run_id = run_changing(&repo, &named_only).await;

    service(&repo, &workflows)
        .answer_gate(
            julia(),
            run_id,
            4,
            approve(),
            None,
            None,
            Answerer::InPerson,
        )
        .await
        .expect("approve");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers[0].approved_control_files, None);
}

/// The answer the workflow took is recorded with the files it approved.
#[tokio::test]
async fn the_recorded_answer_keeps_the_files_it_approved() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());
    let key = StepKey { run_id, seq: 4 };

    steps
        .answer_gate(GateAnswered {
            key,
            answer: AnswerGate {
                seq: 4,
                answer: approve(),
                note: None,
                user_id: julia(),
                approved_control_files: Some(changes().tampered),
            },
        })
        .await
        .expect("record");

    let approvals = repo.approvals.lock().expect("approvals").clone();
    assert_eq!(approvals, [(key, approve(), Some(changes().tampered))]);
}

/// A Run created in person is kept so; one created otherwise is not.
#[tokio::test]
async fn a_run_keeps_whether_it_was_created_in_person() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());

    for in_person in [true, false] {
        let created = service
            .create_run(
                julia(),
                "reply".to_string(),
                "Fix the login bug".to_string(),
                None,
                None,
                in_person,
            )
            .await
            .expect("create");

        assert_eq!(created.run.created_in_person, in_person);
    }
}

/// An admin of a team sets its patterns, which its members read and its
/// Runs use, until they go back to the default.
#[tokio::test]
async fn a_team_admin_sets_the_team_s_control_files() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let team_id = Uuid::now_v7();
    {
        let mut teams = repo.teams.lock().expect("teams");
        teams.insert(
            julia().to_string(),
            TeamMembership {
                team_id,
                admin: true,
            },
        );
        teams.insert(
            teo().to_string(),
            TeamMembership {
                team_id,
                admin: false,
            },
        );
    }
    let own = files(&["ci/**"]);

    let before = service.control_files(&teo()).await.expect("read");
    service
        .set_control_files(julia(), Some(own.clone()))
        .await
        .expect("set");
    let member = service.control_files(&teo()).await.expect("read");
    let reset = service
        .set_control_files(julia(), None)
        .await
        .expect("reset");

    assert!(before.is_default);
    assert_eq!(before.patterns, ControlFiles::default_patterns());
    assert_eq!(before.team_id, Some(team_id));
    assert!(!before.can_edit);
    assert!(!member.is_default);
    assert_eq!(member.patterns, own);
    assert!(reset.is_default);
    assert!(reset.can_edit);
}

/// A member who does not administer the team, someone on no team, and
/// patterns that do not read, keep nothing.
#[tokio::test]
async fn only_a_team_admin_sets_patterns_that_read() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        teo().to_string(),
        TeamMembership {
            team_id,
            admin: false,
        },
    );

    let member = service
        .set_control_files(teo(), Some(files(&["ci/**"])))
        .await;
    let no_team = service
        .set_control_files(julia(), Some(files(&["ci/**"])))
        .await;
    let invalid = service
        .set_control_files(julia(), Some(files(&["./ci.yml"])))
        .await;

    assert!(matches!(member, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(no_team, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(
        invalid,
        Err(FactoryError::InvalidControlFiles(
            ControlFilesError::InvalidPattern(_)
        ))
    ));
    assert!(repo.control_files.lock().expect("patterns").is_empty());
}

/// A Run keeps the patterns its owner's team had when it was created, and
/// the team they came from: a change to the team's list later does not
/// change it. A Run whose owner's team set none keeps the default.
#[tokio::test]
async fn a_run_keeps_the_patterns_it_was_created_with() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        julia().to_string(),
        TeamMembership {
            team_id,
            admin: true,
        },
    );
    let create = || {
        service.create_run(
            julia(),
            "reply".to_string(),
            "Fix the login bug".to_string(),
            None,
            None,
            true,
        )
    };

    let with_default = create().await.expect("create");
    repo.set_team_control_files(team_id, Some(&files(&["ci/**"])))
        .await
        .expect("set");
    let with_own = create().await.expect("create");
    repo.set_team_control_files(team_id, Some(&files(&["Makefile"])))
        .await
        .expect("change");

    let read = |id| {
        repo.runs
            .lock()
            .expect("runs")
            .get(&id)
            .map(|run| (run.control_file_patterns.clone(), run.control_files_team_id))
            .expect("kept")
    };
    assert_eq!(
        read(with_default.run.id),
        (Some(ControlFiles::default_patterns()), None)
    );
    assert_eq!(
        read(with_own.run.id),
        (Some(files(&["ci/**"])), Some(team_id))
    );
}

/// An agent's account signs in as a person does, yet creates no Run in
/// person, answers no Gate and sets no team's patterns, though it
/// administers the team.
#[tokio::test]
async fn an_agent_account_is_never_in_person() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = service(&repo, &workflows);
    repo.agent_accounts
        .lock()
        .expect("agent accounts")
        .insert(julia().to_string());
    repo.teams.lock().expect("teams").insert(
        julia().to_string(),
        TeamMembership {
            team_id: Uuid::now_v7(),
            admin: true,
        },
    );
    let run_id = julia_s_run(&repo).await;

    let created = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Fix ci.yml".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("create");
    let answered = service
        .answer_gate(
            julia(),
            run_id,
            1,
            approve(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;
    let set = service
        .set_control_files(julia(), Some(files(&["ci/**"])))
        .await;
    let read = service.control_files(&julia()).await.expect("read");

    assert!(!created.run.created_in_person);
    assert!(matches!(answered, Err(FactoryError::NotAPerson)));
    assert!(matches!(set, Err(FactoryError::NotAPerson)));
    assert!(!read.can_edit);
    assert!(workflows.answers.lock().expect("answers").is_empty());
}
