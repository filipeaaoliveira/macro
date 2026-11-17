//! A Tampered Run's Gate, answered only with its Control Files approved, and
//! a person's Control File patterns.

use factory_records::domain::models::{Outcome, RunId, StepKey, Tampering};
use factory_records::domain::ports::RunRepository;

use super::{FakeExecutor, FakeWorkflows, julia, julia_s_run, nobody, service};
use crate::domain::control_files::{ControlFiles, ControlFilesError};
use crate::domain::fakes::FakeRepo;
use crate::domain::gate::{AnswerGate, GateAnswered};
use crate::domain::models::FactoryError;
use crate::domain::ports::{FactoryService, StepService};
use crate::domain::service::StepServiceImpl;

fn approve() -> Outcome {
    Outcome::try_from("approve".to_string()).expect("valid")
}

fn revise() -> Outcome {
    Outcome::try_from("revise".to_string()).expect("valid")
}

fn tampered() -> Tampering {
    Tampering {
        files: vec![".github/workflows/ci.yml".to_string()],
        unlisted: false,
    }
}

async fn tampered_run(repo: &FakeRepo) -> RunId {
    let run_id = julia_s_run(repo).await;
    repo.set_tampering(run_id, Some(&tampered()))
        .await
        .expect("keep");
    run_id
}

/// Approving a Tampered Run's Gate without approving its Control Files is
/// refused before the workflow hears of it; nothing else can approve them.
#[tokio::test]
async fn approving_a_tampered_run_needs_its_control_files_approved() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = tampered_run(&repo).await;

    let refused = service(&repo, &workflows)
        .answer_gate(julia(), run_id, 4, approve(), None, false)
        .await;

    assert!(matches!(
        refused,
        Err(FactoryError::ControlFilesNotApproved)
    ));
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

/// An approval of the Control Files goes to the workflow with the files
/// the Run keeps; sending it back approves none, and needs no approval.
#[tokio::test]
async fn an_approval_carries_the_tampered_files_to_the_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = tampered_run(&repo).await;
    let service = service(&repo, &workflows);

    service
        .answer_gate(julia(), run_id, 4, approve(), None, true)
        .await
        .expect("approve");
    service
        .answer_gate(julia(), run_id, 6, revise(), None, false)
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
                approved_control_files: Some(tampered().files),
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

/// A Run that changed no Control File is approved as before; approving
/// files it does not have records none.
#[tokio::test]
async fn a_run_that_is_not_tampered_approves_no_control_files() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;

    service(&repo, &workflows)
        .answer_gate(julia(), run_id, 4, approve(), None, true)
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
                approved_control_files: Some(tampered().files),
            },
        })
        .await
        .expect("record");

    let approvals = repo.approvals.lock().expect("approvals").clone();
    assert_eq!(approvals, [(key, approve(), Some(tampered().files))]);
}

/// A person reads the default until they set their own patterns, which
/// replace it until they go back to it.
#[tokio::test]
async fn a_person_sets_their_own_control_files_and_goes_back_to_the_default() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let own = vec!["ci/**".to_string()];

    let before = service.control_files(&julia()).await.expect("read");
    let set = service
        .set_control_files(julia(), Some(own.clone()))
        .await
        .expect("set");
    let reset = service
        .set_control_files(julia(), None)
        .await
        .expect("reset");

    assert!(before.is_default);
    assert_eq!(before.patterns, ControlFiles::default_patterns());
    assert!(!set.is_default);
    assert_eq!(set.patterns, own);
    assert_eq!(reset, before);
}

/// Patterns that do not read, or a person with no account, keep nothing.
#[tokio::test]
async fn patterns_that_do_not_read_are_refused() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());

    let invalid = service
        .set_control_files(julia(), Some(vec!["/ci.yml".to_string()]))
        .await;
    let unknown = service
        .set_control_files(nobody(), Some(vec!["ci/**".to_string()]))
        .await;

    assert!(matches!(
        invalid,
        Err(FactoryError::InvalidControlFiles(
            ControlFilesError::InvalidPattern(_)
        ))
    ));
    assert!(matches!(unknown, Err(FactoryError::UnknownUser)));
    assert!(repo.control_files.lock().expect("patterns").is_empty());
}
