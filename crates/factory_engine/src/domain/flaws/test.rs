use factory_records::domain::models::{FlawMarker, NewRun, RunId, RunInsert, RunOutcome};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;
use macro_user_id::user_id::MacroUserIdStr;

use super::mark_flawed;
use crate::domain::fakes::FakeRepo;
use crate::domain::models::FactoryError;

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

/// A Run of Julia's that ended `outcome`.
async fn ended(repo: &FakeRepo, outcome: RunOutcome) -> RunId {
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan the export",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    repo.end_run(id, outcome).await.expect("end");
    id
}

/// A Scorer marks a succeeded Run flawed by its name, with its reason
/// trimmed; marking it again keeps the first mark.
#[tokio::test]
async fn a_scorer_marks_a_succeeded_run_flawed() {
    let repo = FakeRepo::default();
    let run = ended(&repo, RunOutcome::Success).await;
    let scorer = FlawMarker::Scorer {
        name: "tests-pass".to_string(),
    };

    let end = mark_flawed(&repo, run, scorer.clone(), " Two tests fail \n")
        .await
        .expect("marked");
    let again = mark_flawed(&repo, run, FlawMarker::Person { user_id: julia() }, "Wrong")
        .await
        .expect("marked again");

    assert_eq!(end.outcome, RunOutcome::Flawed);
    let flaw = end.flaw.clone().expect("flaw");
    assert_eq!(flaw.by, scorer);
    assert_eq!(flaw.reason, "Two tests fail");
    assert_eq!(again, end);
}

/// A reason must say something and fit; a Scorer is named as a Flow names
/// things; only a succeeded Run turns flawed; a Run not recorded is
/// missing.
#[tokio::test]
async fn a_mark_that_does_not_hold_is_refused() {
    let repo = FakeRepo::default();
    let succeeded = ended(&repo, RunOutcome::Success).await;
    let failed = ended(&repo, RunOutcome::Failed).await;
    let person = || FlawMarker::Person { user_id: julia() };

    let blank = mark_flawed(&repo, succeeded, person(), "  ").await;
    let long = mark_flawed(&repo, succeeded, person(), &"x".repeat(4097)).await;
    let badly_named = mark_flawed(
        &repo,
        succeeded,
        FlawMarker::Scorer {
            name: "Tests Pass".to_string(),
        },
        "Wrong",
    )
    .await;
    let not_succeeded = mark_flawed(&repo, failed, person(), "Wrong").await;
    let missing = mark_flawed(&repo, RunId::generate(), person(), "Wrong").await;

    assert!(matches!(blank, Err(FactoryError::FlawReasonSize { .. })));
    assert!(matches!(long, Err(FactoryError::FlawReasonSize { .. })));
    assert!(matches!(badly_named, Err(FactoryError::InvalidScorer(_))));
    assert!(matches!(not_succeeded, Err(FactoryError::NotSucceeded)));
    assert!(matches!(missing, Err(FactoryError::NotFound)));
    let kept = repo.get_run(succeeded).await.expect("read").expect("kept");
    assert_eq!(kept.end.map(|end| end.outcome), Some(RunOutcome::Success));
}
