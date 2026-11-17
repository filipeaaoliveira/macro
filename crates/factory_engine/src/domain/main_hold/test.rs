use std::sync::LazyLock;

use chrono::{TimeDelta, Utc};
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{Repository, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{MainLook, MainState, MainWatch, MainWatchServiceImpl};
use crate::domain::fakes::{FakeGithub, FakeHoldTasks, FakeRepo, FakeWatches, ci};
use crate::domain::models::{
    Check, CheckState, HeadChecks, PullRequestState, RequiredCheck, RequiredChecks, StepInput,
};
use crate::domain::ports::MainWatchService;
use crate::domain::pull_request_steps::{HOLD_WAIT, PullRequestSteps};

const RED_HEAD: &str = "2222222222222222222222222222222222222222";

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

fn run_task() -> Uuid {
    Uuid::parse_str("00000000-0000-7000-8000-000000000101").expect("valid")
}

/// A `code` Run of Julia's on the repository, working on her task.
async fn code_run(repo: &FakeRepo) -> RunId {
    repo.tasks.lock().expect("tasks").insert(run_task());
    let run_id = RunId::generate();
    repo.insert_run(
        run_id,
        "code",
        "Add dark mode",
        &JULIA,
        Some(run_task()),
        Some(&REPOSITORY),
    )
    .await
    .expect("insert");
    run_id
}

fn input(run_id: RunId) -> StepInput<'static> {
    StepInput {
        key: StepKey { run_id, seq: 9 },
        role: None,
        owner: &JULIA,
        request: "Add dark mode",
        task_id: Some(run_task()),
        repository: Some(&REPOSITORY),
        earlier: &[],
    }
}

/// The base's checks, of which `test` is required: one run of it,
/// concluded `conclusion`, or running when none.
fn base_checks(conclusion: Option<&str>) -> HeadChecks {
    let mut checks = ci(conclusion);
    checks.runs[0].url = Some("https://github.com/macro/app/actions/runs/1".to_string());
    checks
}

fn requiring_test(github: &FakeGithub) {
    github.state().required = Some(RequiredChecks {
        checks: vec![RequiredCheck {
            name: "test".to_string(),
            app_id: Some(FakeGithub::ACTIONS),
        }],
        complete: true,
    });
}

fn looks(
    repo: &FakeRepo,
    github: &FakeGithub,
    tasks: &FakeHoldTasks,
) -> MainWatchServiceImpl<FakeRepo, FakeGithub, FakeHoldTasks> {
    MainWatchServiceImpl::new(repo.clone(), github.clone(), tasks.clone())
}

/// A red base is held, and files one task naming the factory merges since
/// it was last green, oldest first, with their pull requests, beside the
/// watch's Run's task; a second red look files none. A green look, as a
/// re-run that passes on the same head, clears the hold.
#[tokio::test]
async fn a_red_base_is_held_once_and_a_green_rerun_clears_it() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watcher = code_run(&repo).await;
    let watch = MainWatch::of(watcher, &REPOSITORY);

    // A merge before the base was last green is no suspect.
    let before = code_run(&repo).await;
    repo.record_merge(before, 1, "https://github.com/macro/app/pull/1", Utc::now())
        .await
        .expect("merge");
    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("success"))));
    let green = looks(&repo, &github, &tasks)
        .look(watch.clone())
        .await
        .expect("look");
    assert_eq!(
        green,
        MainLook {
            state: MainState::Green,
            held: false
        }
    );

    let first = code_run(&repo).await;
    let second = code_run(&repo).await;
    let later = Utc::now() + TimeDelta::seconds(1);
    repo.record_merge(second, 3, "https://github.com/macro/app/pull/3", later)
        .await
        .expect("merge");
    repo.record_merge(first, 2, "https://github.com/macro/app/pull/2", Utc::now())
        .await
        .expect("merge");

    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("failure"))));
    for _ in 0..2 {
        let red = looks(&repo, &github, &tasks)
            .look(watch.clone())
            .await
            .expect("look");
        assert_eq!(
            red,
            MainLook {
                state: MainState::Red,
                held: true
            }
        );
    }
    let hold = repo
        .main_hold(&REPOSITORY)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(hold.head_sha, RED_HEAD);
    assert!(hold.failed.contains("test (failure)"), "{}", hold.failed);
    assert!(hold.task_id.is_some());

    let filed = tasks.filed();
    assert_eq!(filed.len(), 1, "one task per red period");
    let (owner, beside, task) = &filed[0];
    assert_eq!(owner, "macro|julia@example.com");
    assert_eq!(*beside, Some(run_task()));
    assert_eq!(task.title, "main is red in macro/app");
    let pull_2 = task
        .markdown
        .find("[Pull request #2](https://github.com/macro/app/pull/2)")
        .expect("names #2");
    let pull_3 = task
        .markdown
        .find("[Pull request #3](https://github.com/macro/app/pull/3)")
        .expect("names #3");
    assert!(pull_2 < pull_3, "oldest first:\n{}", task.markdown);
    assert!(!task.markdown.contains("pull/1)"), "{}", task.markdown);

    // The failed run is re-run on the same head, and passes.
    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("success"))));
    let cleared = looks(&repo, &github, &tasks)
        .look(watch.clone())
        .await
        .expect("look");
    assert_eq!(cleared.state, MainState::Green);
    assert_eq!(repo.main_hold(&REPOSITORY).await.expect("read"), None);

    // The next red period files its own task.
    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("failure"))));
    looks(&repo, &github, &tasks)
        .look(watch)
        .await
        .expect("look");
    assert_eq!(tasks.filed().len(), 2);
}

/// A task that could not be filed is filed by the next look, once.
#[tokio::test]
async fn a_task_that_could_not_be_filed_is_filed_by_the_next_look() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("failure"))));
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);

    *tasks.failing.lock().expect("failing") = true;
    let failed = looks(&repo, &github, &tasks).look(watch.clone()).await;
    assert!(failed.is_err());
    assert!(
        repo.main_hold(&REPOSITORY)
            .await
            .expect("read")
            .is_some_and(|hold| !hold.task_claimed),
        "the claim is given back"
    );

    *tasks.failing.lock().expect("failing") = false;
    for _ in 0..2 {
        looks(&repo, &github, &tasks)
            .look(watch.clone())
            .await
            .expect("look");
    }
    assert_eq!(tasks.filed().len(), 1);
}

/// A required check that has not finished holds nothing, and keeps a hold
/// that is set: the base is pending.
#[tokio::test]
async fn a_required_check_that_runs_keeps_the_base_as_it_was() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);

    github.state().base = Some((RED_HEAD.to_string(), base_checks(None)));
    let pending = looks(&repo, &github, &tasks)
        .look(watch.clone())
        .await
        .expect("look");
    assert_eq!(
        pending,
        MainLook {
            state: MainState::Pending,
            held: false
        }
    );

    // A check that is not required fails: the required one decides.
    let mut checks = base_checks(Some("success"));
    checks.runs.push(Check {
        name: "lint".to_string(),
        state: CheckState::of_check("completed", Some("failure")),
        url: None,
        app_id: Some(FakeGithub::ACTIONS),
    });
    github.state().base = Some((RED_HEAD.to_string(), checks));
    let green = looks(&repo, &github, &tasks)
        .look(watch.clone())
        .await
        .expect("look");
    assert_eq!(green.state, MainState::Green);

    github.state().base = Some((RED_HEAD.to_string(), base_checks(Some("failure"))));
    looks(&repo, &github, &tasks)
        .look(watch.clone())
        .await
        .expect("look");
    github.state().base = Some((RED_HEAD.to_string(), base_checks(None)));
    let still = looks(&repo, &github, &tasks)
        .look(watch)
        .await
        .expect("look");
    assert_eq!(
        still,
        MainLook {
            state: MainState::Pending,
            held: true
        }
    );
}

/// While the base is held, an approved merge merges nothing: it waits, has
/// the base watched, and ends `held` for a person when the hold outlasts
/// its wait.
#[tokio::test(start_paused = true)]
async fn a_held_base_merges_nothing_even_once_approved() {
    let repo = FakeRepo::default();
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure): https://ci/1")
        .await
        .expect("hold");

    let started = tokio::time::Instant::now();
    let report = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone())
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(report.outcome.as_str(), "held");
    assert!(
        report.output.contains("test (failure)"),
        "{}",
        report.output
    );
    assert!(started.elapsed() >= HOLD_WAIT - std::time::Duration::from_secs(30));
    assert!(github.state().merges.is_empty(), "nothing merged");
    assert!(
        github
            .state()
            .pull_request
            .as_ref()
            .is_some_and(|pull_request| pull_request.state == PullRequestState::Open)
    );
    assert_eq!(watches.started(), vec![MainWatch::of(run_id, &REPOSITORY)]);
    assert!(repo.main().merges.is_empty());
}

/// A hold that clears while the merge waits lets it merge; the merge is
/// recorded and the base watched, and a retry records it once.
#[tokio::test(start_paused = true)]
async fn a_merge_waits_out_a_hold_that_clears() {
    let repo = FakeRepo::default();
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)")
        .await
        .expect("hold");

    let steps = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone());
    let clearing = async {
        tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;
        repo.clear_main_hold(&REPOSITORY, Utc::now())
            .await
            .expect("clear");
    };
    let (report, ()) = tokio::join!(steps.merge(input(run_id)), clearing);
    let report = report.expect("merge");
    assert_eq!(report.outcome.as_str(), "merged");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);

    let again = steps.merge(input(run_id)).await.expect("merge again");
    assert_eq!(again.outcome.as_str(), "merged");
    let merges = repo.main().merges.clone();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0].1.number, 7);
    assert_eq!(merges[0].1.url, "https://github.com/macro/app/pull/7");
    // Once for the hold it found, then after each merge it saw.
    assert_eq!(watches.started().len(), 3);
}
