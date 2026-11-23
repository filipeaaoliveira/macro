//! Retrying a Run that ended `failed`, `infrastructure` or `blocked`.

use factory_records::domain::models::{
    EntryId, KeptFlow, NewRun, Outcome, Repository, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;

use super::{FakeExecutor, FakeWorkflows, StepServiceImpl, code_service, julia, reply_step, teo};
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::StepUse;
use crate::domain::models::{FactoryError, StartAt, StepError, StepRequest, run_branch};
use crate::domain::ports::{FactoryService, StepService};

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid")
}

fn outcome(word: &str) -> Outcome {
    Outcome::try_from(word.to_string()).expect("valid")
}

/// A `code` Run of Julia's that keeps its Flow, on a repository.
async fn code_run(repo: &FakeRepo) -> RunId {
    let flow = BuiltInFlow::named("code")
        .expect("built in")
        .read(|_| Some(StepUse::Work))
        .expect("valid flow");
    let kept = serde_json::to_value(&flow).expect("flows serialize");
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    match repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Collapse repeated dashes in slugs",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: Some(&repository),
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: Some(KeptFlow {
                definition: &kept,
                asks: true,
            }),
        })
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run.id,
        _ => panic!("a run with no task is kept"),
    }
}

/// Record that the Run `run_id` took the Step `seq`, the `visit` of
/// `entry`, which ended with `ended`.
async fn took(repo: &FakeRepo, run_id: RunId, seq: u16, entry_id: &str, visit: u16, ended: &str) {
    let key = StepKey { run_id, seq };
    repo.start_step(key, &entry(entry_id), visit)
        .await
        .expect("start");
    repo.end_step(key, &outcome(ended), &format!("The {entry_id}"))
        .await
        .expect("end");
}

/// A `code` Run that planned, had its plan approved, and then lost its
/// writer's session: it ended `infrastructure` at `write`.
async fn lost_at_write(repo: &FakeRepo) -> RunId {
    let run_id = code_run(repo).await;
    took(repo, run_id, 1, "plan", 1, "done").await;
    let plan = StepKey { run_id, seq: 1 };
    repo.open_gate(
        plan,
        "Approve the plan?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    repo.answer_gate(
        plan,
        &outcome("approve"),
        Some("Keep it small"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    took(repo, run_id, 2, "write", 1, "failed").await;
    repo.end_run(run_id, RunOutcome::Infrastructure)
        .await
        .expect("end");
    run_id
}

/// A Run that lost a session is retried at the entry it stopped at: the
/// new Run keeps the plan and its approval, names the Run it retries,
/// works on its branch, and its workflow starts at `write`, past the plan.
#[tokio::test]
async fn a_run_that_lost_a_session_is_retried_at_the_entry_it_stopped_at() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let lost = lost_at_write(&repo).await;

    let retried = service
        .retry_run(&julia(), lost, None, true)
        .await
        .expect("retry");

    let retry = retried
        .run
        .retry
        .expect("the retry names the run it retries");
    assert_eq!(retry.retried_from, lost);
    assert_eq!(retry.branch_of, lost);
    assert_eq!(retry.first_seq, 2);
    assert_ne!(retried.run.id, lost);
    assert_eq!(retried.run.request, "Collapse repeated dashes in slugs");
    assert!(retried.run.end.is_none());
    // The plan is kept, with its output and its approval; the write that
    // failed is not.
    let kept = repo.list_steps(retried.run.id).await.expect("read");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].entry, entry("plan"));
    assert_eq!(
        kept[0].end.as_ref().map(|end| end.output.as_str()),
        Some("The plan")
    );
    let answer = kept[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the approval is kept");
    assert_eq!(answer.note.as_deref(), Some("Keep it small"));
    let started = workflows.started.lock().expect("started").clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].run_id, retried.run.id);
    assert_eq!(
        started[0].start_at,
        Some(StartAt {
            entry: entry("write"),
            first_seq: 2,
            branch_of: lost,
        })
    );
    // Its branch is the Run's it retries.
    assert_eq!(
        run_branch(retried.run.branch_of()),
        run_branch(lost),
        "the retry pushes to the retried run's branch"
    );
    // The Run it retries is left as it ended.
    let old = repo.get_run(lost).await.expect("read").expect("kept");
    assert_eq!(
        old.end.map(|end| end.outcome),
        Some(RunOutcome::Infrastructure)
    );
}

/// Only a Run that ended `failed`, `infrastructure` or `blocked` is
/// retried: one still running, or that ended `success`, `manual` or
/// `cancelled`, is not, and starts nothing.
#[tokio::test]
async fn only_a_run_that_failed_on_infrastructure_or_was_blocked_is_retried() {
    for (ending, allowed) in [
        (Some(RunOutcome::Failed), true),
        (Some(RunOutcome::Infrastructure), true),
        (Some(RunOutcome::Blocked), true),
        (Some(RunOutcome::Success), false),
        (Some(RunOutcome::Manual), false),
        (Some(RunOutcome::Cancelled), false),
        (None, false),
    ] {
        let repo = FakeRepo::default();
        let workflows = FakeWorkflows::default();
        let service = code_service(&repo, &workflows, &FakeGithub::default());
        let run_id = code_run(&repo).await;
        took(&repo, run_id, 1, "plan", 1, "done").await;
        if let Some(ending) = ending {
            repo.end_run(run_id, ending).await.expect("end");
        }

        let retried = service.retry_run(&julia(), run_id, None, true).await;

        if allowed {
            assert!(retried.is_ok(), "{ending:?}: {retried:?}");
            assert_eq!(workflows.started.lock().expect("started").len(), 1);
        } else {
            assert!(
                matches!(retried, Err(FactoryError::NotRetriable)),
                "{ending:?}: {retried:?}"
            );
            assert!(workflows.started.lock().expect("started").is_empty());
            assert_eq!(repo.runs.lock().expect("runs").len(), 1, "{ending:?}");
        }
    }
}

/// A retry starts at the latest visit's first Step: a review panel's whole
/// round runs again, from its first reviewer, and the Steps before it are
/// kept.
#[tokio::test]
async fn a_retry_runs_the_whole_visit_it_stopped_in_again() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = code_run(&repo).await;
    took(&repo, run_id, 1, "plan", 1, "done").await;
    took(&repo, run_id, 2, "write", 1, "done").await;
    took(&repo, run_id, 3, "review", 1, "pass").await;
    took(&repo, run_id, 4, "review", 1, "failed").await;
    took(&repo, run_id, 5, "review", 1, "pass").await;
    repo.end_run(run_id, RunOutcome::Failed).await.expect("end");

    let retried = service
        .retry_run(&julia(), run_id, None, true)
        .await
        .expect("retry");

    assert_eq!(
        workflows.started.lock().expect("started")[0].start_at,
        Some(StartAt {
            entry: entry("review"),
            first_seq: 3,
            branch_of: run_id,
        })
    );
    let kept: Vec<u16> = repo
        .list_steps(retried.run.id)
        .await
        .expect("read")
        .iter()
        .map(|step| step.seq)
        .collect();
    assert_eq!(kept, [1, 2]);
}

/// A visit number does not name a visit: a reset counts an entry's visits
/// from 1 again. A Run sent back from `accept` to `fix` reviews again at
/// `review`'s visit 1; a retry of the second review starts at its own
/// round, not at the first one's.
#[tokio::test]
async fn a_retry_after_a_reset_starts_at_the_latest_visit_not_an_earlier_one() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = code_run(&repo).await;
    took(&repo, run_id, 1, "plan", 1, "done").await;
    took(&repo, run_id, 2, "write", 1, "done").await;
    took(&repo, run_id, 3, "review", 1, "pass").await;
    took(&repo, run_id, 4, "accept", 1, "fix").await;
    took(&repo, run_id, 5, "fix", 1, "done").await;
    took(&repo, run_id, 6, "review", 1, "failed").await;
    repo.end_run(run_id, RunOutcome::Failed).await.expect("end");

    let retried = service
        .retry_run(&julia(), run_id, None, true)
        .await
        .expect("retry");

    assert_eq!(
        workflows.started.lock().expect("started")[0].start_at,
        Some(StartAt {
            entry: entry("review"),
            first_seq: 6,
            branch_of: run_id,
        })
    );
    let kept: Vec<u16> = repo
        .list_steps(retried.run.id)
        .await
        .expect("read")
        .iter()
        .map(|step| step.seq)
        .collect();
    assert_eq!(kept, [1, 2, 3, 4, 5]);
}

/// The place of a retry's start, and the places of the Steps it kept.
async fn started(repo: &FakeRepo, workflows: &FakeWorkflows, retry: RunId) -> (StartAt, Vec<u16>) {
    let start = workflows
        .started
        .lock()
        .expect("started")
        .iter()
        .find(|start| start.run_id == retry)
        .and_then(|start| start.start_at.clone())
        .expect("the retry started");
    let kept = repo
        .list_steps(retry)
        .await
        .expect("read")
        .iter()
        .map(|step| step.seq)
        .collect();
    (start, kept)
}

/// A gap in the places ends a visit: Steps of the same entry and visit
/// number that are not at consecutive places are two visits, as when a
/// Step between them was never recorded.
#[tokio::test]
async fn a_retry_s_visit_ends_at_a_gap_in_its_places() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = code_run(&repo).await;
    took(&repo, run_id, 1, "plan", 1, "done").await;
    took(&repo, run_id, 2, "write", 1, "done").await;
    took(&repo, run_id, 3, "review", 1, "pass").await;
    took(&repo, run_id, 5, "review", 1, "failed").await;
    repo.end_run(run_id, RunOutcome::Failed).await.expect("end");

    let retried = service
        .retry_run(&julia(), run_id, None, true)
        .await
        .expect("retry");

    let (start, kept) = started(&repo, &workflows, retried.run.id).await;
    assert_eq!(start.first_seq, 5);
    assert_eq!(kept, [1, 2, 3]);
}

/// A retry that ended before taking a Step of its own (as `blocked`
/// before its first agent Step) is retried where it started, not at the
/// visit of the latest Step it kept.
#[tokio::test]
async fn a_retry_that_took_no_step_of_its_own_is_retried_where_it_started() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let lost = lost_at_write(&repo).await;
    let first = service
        .retry_run(&julia(), lost, None, true)
        .await
        .expect("retry");
    repo.end_run(first.run.id, RunOutcome::Blocked)
        .await
        .expect("end");

    let second = service
        .retry_run(&julia(), first.run.id, None, true)
        .await
        .expect("retry the retry");

    let (start, kept) = started(&repo, &workflows, second.run.id).await;
    assert_eq!(
        start,
        StartAt {
            entry: entry("write"),
            first_seq: 2,
            branch_of: lost,
        }
    );
    assert_eq!(kept, [1]);
}

/// Each retry counts an entry's visits from 1 again, so a retry's first
/// own Step may share its entry and visit number with the kept Step just
/// before it, as `merge` going round on `queued` does: a retry of the
/// retry starts at the retry's own Step, never in the Steps it kept.
#[tokio::test]
async fn a_retry_of_a_retry_never_starts_in_the_steps_it_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = code_run(&repo).await;
    for (seq, at, ended) in [
        (1, "plan", "done"),
        (2, "write", "done"),
        (3, "review", "pass"),
        (4, "accept", "pass"),
        (5, "publish", "published"),
        (6, "ci", "green"),
        (7, "merge-gate", "approve"),
    ] {
        took(&repo, run_id, seq, at, 1, ended).await;
    }
    took(&repo, run_id, 8, "merge", 1, "queued").await;
    took(&repo, run_id, 9, "merge", 2, "failed").await;
    repo.end_run(run_id, RunOutcome::Infrastructure)
        .await
        .expect("end");
    let first = service
        .retry_run(&julia(), run_id, None, true)
        .await
        .expect("retry");
    took(&repo, first.run.id, 9, "merge", 1, "failed").await;
    repo.end_run(first.run.id, RunOutcome::Infrastructure)
        .await
        .expect("end");

    let second = service
        .retry_run(&julia(), first.run.id, None, true)
        .await
        .expect("retry the retry");

    let (start, kept) = started(&repo, &workflows, second.run.id).await;
    assert_eq!(
        start,
        StartAt {
            entry: entry("merge"),
            first_seq: 9,
            branch_of: run_id,
        }
    );
    assert_eq!(kept, [1, 2, 3, 4, 5, 6, 7, 8]);
}

/// A Run that took no Step is retried from its Flow's first entry,
/// keeping nothing.
#[tokio::test]
async fn a_run_that_took_no_step_is_retried_from_its_first_entry() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = code_run(&repo).await;
    repo.end_run(run_id, RunOutcome::Infrastructure)
        .await
        .expect("end");

    service
        .retry_run(&julia(), run_id, None, true)
        .await
        .expect("retry");

    assert_eq!(
        workflows.started.lock().expect("started")[0].start_at,
        Some(StartAt {
            entry: entry("plan"),
            first_seq: 1,
            branch_of: run_id,
        })
    );
}

/// A Run is retried once; its retry, should it fail too, is retried in
/// turn, on the first Run's branch.
#[tokio::test]
async fn a_run_is_retried_once_and_a_retry_of_a_retry_keeps_the_first_branch() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let lost = lost_at_write(&repo).await;
    let first = service
        .retry_run(&julia(), lost, None, true)
        .await
        .expect("retry");

    let again = service.retry_run(&julia(), lost, None, true).await;
    took(&repo, first.run.id, 2, "write", 1, "failed").await;
    repo.end_run(first.run.id, RunOutcome::Infrastructure)
        .await
        .expect("end");
    let second = service
        .retry_run(&julia(), first.run.id, None, true)
        .await
        .expect("retry the retry");

    assert!(
        matches!(again, Err(FactoryError::AlreadyRetried)),
        "{again:?}"
    );
    let retry = second.run.retry.expect("a retry");
    assert_eq!(retry.retried_from, first.run.id);
    assert_eq!(retry.branch_of, lost);
    assert_eq!(workflows.started.lock().expect("started").len(), 2);
}

/// Someone else's Run reads as missing to a retry, as to every act.
#[tokio::test]
async fn someone_else_s_run_is_not_retried() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let lost = lost_at_write(&repo).await;

    let retried = service.retry_run(&teo(), lost, None, true).await;

    assert!(
        matches!(retried, Err(FactoryError::NotFound)),
        "{retried:?}"
    );
    assert!(workflows.started.lock().expect("started").is_empty());
}

/// A retry whose workflow does not start is not kept, so the Run can be
/// retried again.
#[tokio::test]
async fn a_retry_whose_workflow_does_not_start_is_not_kept() {
    let repo = FakeRepo::default();
    let failing = FakeWorkflows {
        fail: true,
        ..FakeWorkflows::default()
    };
    let lost = lost_at_write(&repo).await;

    let refused = code_service(&repo, &failing, &FakeGithub::default())
        .retry_run(&julia(), lost, None, true)
        .await;
    let workflows = FakeWorkflows::default();
    let retried = code_service(&repo, &workflows, &FakeGithub::default())
        .retry_run(&julia(), lost, None, true)
        .await;

    assert!(refused.is_err());
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(repo.runs.lock().expect("runs").len(), 2);
}

/// Whether a retry's request asks for the Control Files it names is the
/// retrier's to say: a Run created in person and retried by an agent's
/// program is not in person, and one retried in person is.
#[tokio::test]
async fn a_retry_is_in_person_only_when_its_owner_retried_it_so() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let by_program = lost_at_write(&repo).await;
    let by_person = lost_at_write(&repo).await;

    let program = service
        .retry_run(&julia(), by_program, None, false)
        .await
        .expect("retry");
    let person = service
        .retry_run(&julia(), by_person, None, true)
        .await
        .expect("retry");

    assert!(!program.run.created_in_person);
    assert!(person.run.created_in_person);
}

/// A retry's Step starts only from a request that names the retry's
/// branch: one without it came from a workflow that does not know the Run
/// is a retry, and would work on a branch of the retry's own.
#[tokio::test]
async fn a_retry_s_step_starts_only_when_its_request_names_the_branch() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let lost = lost_at_write(&repo).await;
    let retried = service
        .retry_run(&julia(), lost, None, true)
        .await
        .expect("retry");
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());

    let unnamed = steps.start_step(reply_step(retried.run.id, 2)).await;
    let named = steps
        .start_step(StepRequest {
            branch_of: Some(lost),
            ..reply_step(retried.run.id, 2)
        })
        .await;

    assert!(matches!(unnamed, Err(StepError::Final(_))), "{unnamed:?}");
    assert!(named.is_ok(), "{named:?}");
}
