use factory_records::domain::standing_rules::RunRules;
use std::sync::LazyLock;

use chrono::{DateTime, TimeDelta, Utc};
use factory_records::domain::main_hold::{MainHoldRepository, RequiredChecksState};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, GateKind, NewRun, Outcome, Repository, RunId, Step, StepEnd, StepKey,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{MainLook, MainState, MainWatch, MainWatchServiceImpl};
use crate::domain::fakes::{FakeGithub, FakeHoldTasks, FakeRepo, FakeWatches, ci, suite};
use crate::domain::models::{
    Check, CheckState, CommitStatus, HeadChecks, PullRequestState, RequiredCheck, RequiredChecks,
    StepInput,
};
use crate::domain::ports::MainWatchService;
use crate::domain::pull_request_steps::{HOLD_WAIT, PullRequestSteps};

const RED_HEAD: &str = "2222222222222222222222222222222222222222";

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static BOB: LazyLock<MacroUserIdStr<'static>> =
    LazyLock::new(|| MacroUserIdStr::try_from("macro|bob@example.com".to_string()).expect("valid"));

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

static OTHER: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/other".to_string(),
    base_branch: "main".to_string(),
});

fn run_task() -> Uuid {
    Uuid::parse_str("00000000-0000-7000-8000-000000000101").expect("valid")
}

fn a_minute_ago() -> DateTime<Utc> {
    Utc::now() - TimeDelta::minutes(1)
}

/// A `code` Run of `owner`'s on the repository, working on a task.
async fn run_of(repo: &FakeRepo, owner: &MacroUserIdStr<'static>) -> RunId {
    run_of_on(repo, owner, &REPOSITORY).await
}

/// A `code` Run of `owner`'s on `repository`, working on a task.
async fn run_of_on(
    repo: &FakeRepo,
    owner: &MacroUserIdStr<'static>,
    repository: &Repository,
) -> RunId {
    repo.tasks.lock().expect("tasks").insert(run_task());
    let run_id = RunId::generate();
    repo.insert_run(NewRun {
        id: run_id,
        flow: "code",
        request: "Add dark mode",
        user_id: owner,
        task_id: Some(run_task()),
        repository: Some(repository),
        created_in_person: false,
        control_file_patterns: &[],
        control_files_team_id: None,
        standing_rules: RunRules::DEFAULT,
    })
    .await
    .expect("insert");
    run_id
}

/// A `code` Run of Julia's on the repository, working on her task.
async fn code_run(repo: &FakeRepo) -> RunId {
    run_of(repo, &JULIA).await
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
        created_in_person: false,
        control_file_patterns: None,
        review: None,
        reviewer: false,
        may_ask: false,
        questions_left: 0,
        resumes: None,
        owner_notes: &[],
    }
}

/// GitHub with the Run's pull request open and passing, its branch changing
/// no Control File, so the Run is not Tampered.
fn open_pull_request() -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().changed = Some(Vec::new());
    github
}

/// The base's checks: the CI suite with its one run, `test`, concluded
/// `conclusion`, or running when none.
fn base_checks(conclusion: Option<&str>) -> HeadChecks {
    let mut checks = ci(conclusion);
    checks.runs[0].url = Some("https://github.com/macro/app/actions/runs/1".to_string());
    checks
}

fn requiring(github: &FakeGithub, names: &[&str]) {
    github.state().required = Some(RequiredChecks {
        checks: names
            .iter()
            .map(|name| RequiredCheck {
                name: (*name).to_string(),
                app_id: Some(FakeGithub::ACTIONS),
            })
            .collect(),
        complete: true,
    });
}

fn requiring_test(github: &FakeGithub) {
    requiring(github, &["test"]);
}

fn base_at(github: &FakeGithub, checks: HeadChecks) {
    github.state().base = Some((RED_HEAD.to_string(), checks));
}

fn looks(
    repo: &FakeRepo,
    github: &FakeGithub,
    tasks: &FakeHoldTasks,
) -> MainWatchServiceImpl<FakeRepo, FakeGithub, FakeHoldTasks> {
    MainWatchServiceImpl::new(repo.clone(), github.clone(), tasks.clone())
}

async fn look(
    repo: &FakeRepo,
    github: &FakeGithub,
    tasks: &FakeHoldTasks,
    watch: &MainWatch,
) -> MainLook {
    looks(repo, github, tasks)
        .look(watch.clone())
        .await
        .expect("look")
}

/// A red base is held, and files one task naming the factory merges since
/// it was last green, oldest first, with their pull requests; a second red
/// look files none. A green look, as a re-run that passes on the same
/// head, clears the hold, and the next red period files its own task.
#[tokio::test]
async fn a_red_base_is_held_once_and_a_green_rerun_clears_it() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);

    // A merge before the base was last green is no suspect.
    let before = code_run(&repo).await;
    repo.record_merge(
        before,
        1,
        "https://github.com/macro/app/pull/1",
        a_minute_ago(),
    )
    .await
    .expect("merge");
    base_at(&github, base_checks(Some("success")));
    let green = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((green.state, green.held), (MainState::Green, false));

    let first = code_run(&repo).await;
    let second = code_run(&repo).await;
    repo.record_merge(first, 2, "https://github.com/macro/app/pull/2", Utc::now())
        .await
        .expect("merge");
    repo.record_merge(second, 3, "https://github.com/macro/app/pull/3", Utc::now())
        .await
        .expect("merge");

    base_at(&github, base_checks(Some("failure")));
    for _ in 0..2 {
        let red = look(&repo, &github, &tasks, &watch).await;
        assert_eq!((red.state, red.held), (MainState::Red, true));
        assert!(!red.ends_watch());
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
    base_at(&github, base_checks(Some("success")));
    let cleared = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(cleared.state, MainState::Green);
    assert!(cleared.ends_watch());
    assert_eq!(repo.main_hold(&REPOSITORY).await.expect("read"), None);

    base_at(&github, base_checks(Some("failure")));
    look(&repo, &github, &tasks, &watch).await;
    assert_eq!(tasks.filed().len(), 2);
}

/// GitHub reads as the owner of the Run that merged to the base last, and
/// files the hold's task as them, not as the Run that started the watch.
#[tokio::test]
async fn the_latest_merge_s_owner_reads_github_and_files_the_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    let bobs = run_of(&repo, &BOB).await;
    repo.record_merge(bobs, 4, "https://github.com/macro/app/pull/4", Utc::now())
        .await
        .expect("merge");

    base_at(&github, base_checks(Some("failure")));
    look(&repo, &github, &tasks, &watch).await;
    assert_eq!(tasks.filed()[0].0, "macro|bob@example.com");
}

/// A task that could not be filed is filed by the next look, once.
#[tokio::test]
async fn a_task_that_could_not_be_filed_is_filed_by_the_next_look() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    base_at(&github, base_checks(Some("failure")));
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
        look(&repo, &github, &tasks, &watch).await;
    }
    assert_eq!(tasks.filed().len(), 1);
    // The next look files under the id the failed one asked for, so a
    // filing whose answer was lost is found, not made twice.
    let ids = tasks.ids.lock().expect("ids").clone();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    let held = repo
        .main_hold(&REPOSITORY)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(held.task_id, Some(ids[0]));
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

    base_at(&github, base_checks(None));
    let pending = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((pending.state, pending.held), (MainState::Pending, false));

    // A check that is not required fails: the required one decides.
    let mut checks = base_checks(Some("success"));
    checks.runs.push(Check {
        name: "lint".to_string(),
        state: CheckState::of_check("completed", Some("failure")),
        url: None,
        app_id: Some(FakeGithub::ACTIONS),
    });
    base_at(&github, checks);
    let green = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(green.state, MainState::Green);

    base_at(&github, base_checks(Some("failure")));
    look(&repo, &github, &tasks, &watch).await;
    base_at(&github, base_checks(None));
    let still = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((still.state, still.held), (MainState::Pending, true));
}

/// A required check that never runs on the base, as one that runs only on
/// pull requests, does not apply once the head's other checks finished:
/// the base is green, and a hold clears. Until then it is pending.
#[tokio::test]
async fn a_required_check_that_never_runs_on_the_base_does_not_apply() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring(&github, &["test", "cla"]);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");

    let mut running = base_checks(Some("success"));
    running.runs.push(Check {
        name: "build".to_string(),
        state: CheckState::Waiting,
        url: None,
        app_id: Some(FakeGithub::ACTIONS),
    });
    base_at(&github, running);
    let pending = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((pending.state, pending.held), (MainState::Pending, true));

    base_at(&github, base_checks(Some("success")));
    let green = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((green.state, green.held), (MainState::Green, false));
    assert_eq!(repo.main_hold(&REPOSITORY).await.expect("read"), None);
}

/// Required checks no owner can read hold nothing: an unheld base is
/// marked unread and the watch may end, but a hold a red look set stands,
/// as does one when the checks were read only in part.
#[tokio::test]
async fn unread_required_checks_keep_a_hold_and_hold_nothing_new() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    base_at(&github, base_checks(Some("failure")));

    let unread = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((unread.state, unread.held), (MainState::Unread, false));
    assert!(unread.ends_watch());
    let state = repo.base_state(&REPOSITORY).await.expect("read");
    assert_eq!(state.required, Some(RequiredChecksState::Unread));

    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    for required in [
        None,
        Some(RequiredChecks {
            checks: Vec::new(),
            complete: false,
        }),
    ] {
        github.state().required = required;
        let kept = look(&repo, &github, &tasks, &watch).await;
        assert_eq!((kept.state, kept.held), (MainState::Pending, true));
        assert!(!kept.ends_watch());
        assert!(repo.main_hold(&REPOSITORY).await.expect("read").is_some());
    }
    assert!(tasks.filed().is_empty());
}

/// A base that requires no checks, as GitHub says in full, holds nothing:
/// a hold is lifted, and its task is told why.
#[tokio::test]
async fn a_base_that_requires_no_checks_lifts_its_hold_and_says_so() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    base_at(&github, base_checks(Some("failure")));
    look(&repo, &github, &tasks, &watch).await;
    let task_id = repo
        .main_hold(&REPOSITORY)
        .await
        .expect("read")
        .and_then(|hold| hold.task_id)
        .expect("filed");

    github.state().required = Some(RequiredChecks {
        checks: Vec::new(),
        complete: true,
    });
    let lifted = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((lifted.state, lifted.held), (MainState::Unread, false));
    let state = repo.base_state(&REPOSITORY).await.expect("read");
    assert_eq!(state.hold, None);
    assert_eq!(state.required, Some(RequiredChecksState::None));
    let comments = tasks.comments.lock().expect("comments").clone();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].1, task_id);
    assert_eq!(comments[0].2, "Failing checks:");
    assert!(
        comments[0].3.contains("no longer requires any checks"),
        "{}",
        comments[0].3
    );
}

/// A fresh head, whose suites GitHub made but filled with no run yet,
/// stays pending: a required check that has not appeared is waited for.
#[tokio::test]
async fn a_fresh_head_with_no_runs_yet_stays_pending() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    let merging = code_run(&repo).await;
    repo.record_merge(
        merging,
        8,
        "https://github.com/macro/app/pull/8",
        a_minute_ago(),
    )
    .await
    .expect("merge");

    let mut fresh = HeadChecks::default();
    let mut suite = suite(1, "github-actions", Some("success"));
    suite.check_runs = 0;
    suite.state = CheckState::Waiting;
    fresh.suites.push(suite);
    base_at(&github, fresh);
    let pending = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(pending.state, MainState::Pending);
    assert!(!pending.ends_watch());

    base_at(&github, HeadChecks::default());
    let nothing = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(nothing.state, MainState::Pending);
}

/// A required status a CI outside GitHub posts late is waited for after
/// the base's latest merge, and stops applying only once the grace passed.
#[tokio::test]
async fn a_required_status_posted_late_is_waited_for() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    github.state().required = Some(RequiredChecks {
        checks: vec![
            RequiredCheck {
                name: "test".to_string(),
                app_id: Some(FakeGithub::ACTIONS),
            },
            RequiredCheck {
                name: "deploy".to_string(),
                app_id: None,
            },
        ],
        complete: true,
    });
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    base_at(&github, base_checks(Some("success")));

    let recent = code_run(&repo).await;
    repo.record_merge(
        recent,
        9,
        "https://github.com/macro/app/pull/9",
        a_minute_ago(),
    )
    .await
    .expect("merge");
    let waiting = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(waiting.state, MainState::Pending);

    let red = HeadChecks {
        statuses: vec![CommitStatus {
            context: "deploy".to_string(),
            state: CheckState::of_status("failure"),
            url: Some("https://ci.example.com/deploy".to_string()),
        }],
        ..base_checks(Some("success"))
    };
    base_at(&github, red);
    let held = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((held.state, held.held), (MainState::Red, true));

    let old = MainWatch::of(code_run(&repo).await, &OTHER);
    let other = run_of_on(&repo, &JULIA, &OTHER).await;
    repo.record_merge(
        other,
        10,
        "https://github.com/macro/other/pull/10",
        Utc::now() - TimeDelta::hours(1),
    )
    .await
    .expect("merge");
    base_at(&github, base_checks(Some("success")));
    let green = look(&repo, &github, &tasks, &old).await;
    assert_eq!(green.state, MainState::Green);
}

/// When GitHub refuses the latest merge's owner, the next owner reads the
/// base and files the hold's task.
#[tokio::test]
async fn a_refused_owner_passes_the_look_to_the_next() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    let bobs = run_of(&repo, &BOB).await;
    repo.record_merge(bobs, 4, "https://github.com/macro/app/pull/4", Utc::now())
        .await
        .expect("merge");
    github
        .state()
        .refused_owners
        .insert("macro|bob@example.com".to_string());
    base_at(&github, base_checks(Some("failure")));

    let red = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(red.state, MainState::Red);
    assert_eq!(tasks.filed()[0].0, "macro|julia@example.com");
}

/// GitHub that cannot be read decides nothing: a held base stays held, and
/// the watch goes on.
#[tokio::test]
async fn a_look_that_cannot_read_github_keeps_the_hold() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    github.state().base_unreadable = true;

    let unseen = look(&repo, &github, &tasks, &watch).await;
    assert_eq!((unseen.state, unseen.held), (MainState::Pending, true));
    assert!(!unseen.ends_watch());
    assert!(repo.main_hold(&REPOSITORY).await.expect("read").is_some());
}

/// A green look does not end the watch when a merge landed after its read
/// began: the head it saw may not hold that merge.
#[tokio::test]
async fn a_merge_after_the_read_keeps_the_watch_going() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    let merging = code_run(&repo).await;
    repo.record_merge(
        merging,
        5,
        "https://github.com/macro/app/pull/5",
        Utc::now() + TimeDelta::minutes(1),
    )
    .await
    .expect("merge");
    base_at(&github, base_checks(Some("success")));

    let later = look(&repo, &github, &tasks, &watch).await;
    assert_eq!(later.state, MainState::Pending);
    assert!(!later.ends_watch());
}

/// While checks run with nothing held, the watch goes on until the base's
/// latest merge is hours old; a held base is never idle.
#[tokio::test]
async fn an_unheld_watch_idles_hours_after_the_latest_merge() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    requiring_test(&github);
    let tasks = FakeHoldTasks::default();
    let watch = MainWatch::of(code_run(&repo).await, &REPOSITORY);
    base_at(&github, base_checks(None));

    let old = code_run(&repo).await;
    repo.record_merge(
        old,
        6,
        "https://github.com/macro/app/pull/6",
        Utc::now() - TimeDelta::hours(7),
    )
    .await
    .expect("merge");
    let idle = look(&repo, &github, &tasks, &watch).await;
    assert!(idle.idle);
    assert!(idle.ends_watch());

    let recent = code_run(&repo).await;
    repo.record_merge(
        recent,
        7,
        "https://github.com/macro/app/pull/7",
        a_minute_ago(),
    )
    .await
    .expect("merge");
    let busy = look(&repo, &github, &tasks, &watch).await;
    assert!(!busy.idle);
    assert!(!busy.ends_watch());

    let held = MainLook {
        state: MainState::Pending,
        held: true,
        idle: true,
    };
    assert!(!held.ends_watch());
}

/// A watch that does not start after a merge leaves the Step merged.
#[tokio::test]
async fn a_watch_that_does_not_start_leaves_the_merge_merged() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    let watches = FakeWatches::default();
    *watches.failing.lock().expect("failing") = true;
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");

    let report = PullRequestSteps::new(github.clone(), repo.clone(), watches)
        .merge(input(run_id))
        .await
        .expect("merge");
    assert_eq!(report.outcome.as_str(), "merged");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
    assert_eq!(repo.main().merges.len(), 1);
}

/// While the base is held, an approved merge merges nothing: it waits, has
/// the base watched, and ends `held` when the hold outlasts its wait, which
/// the `code` Flow sends back to its merge Gate.
#[tokio::test(start_paused = true)]
async fn a_held_base_merges_nothing_even_once_approved() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(
        &REPOSITORY,
        RED_HEAD,
        "- test (failure): https://ci/1",
        a_minute_ago(),
    )
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
    let github = open_pull_request();
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
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

/// The merge Gate's Step, answered `approve`, approving `approved`.
fn approved_merge_gate(approved: &[&str]) -> Step {
    let approve = Outcome::try_from("approve".to_string()).expect("valid");
    Step {
        seq: 4,
        entry: EntryId::try_from("merge-gate".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Every check passed.".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            asked_back: None,
            stand_in: None,
            kind: GateKind::Flow,
            question: "Merge?".to_string(),
            answers: vec![approve.clone()],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                reply_confidence: None,
                by_stand_in: false,
                agent_session_id: None,
                answer: approve,
                note: None,
                user_id: None,
                rule: None,
                approved_control_files: Some(
                    approved.iter().map(|path| (*path).to_string()).collect(),
                ),
                answered_at: Utc::now(),
            }),
        }),
        checks: Vec::new(),
    }
}

/// A Tampered Run whose Control Files no person approved goes back to its
/// merge Gate at once, even while its base is held: it neither waits out
/// the hold nor merges.
#[tokio::test(start_paused = true)]
async fn an_unapproved_tampered_run_on_a_held_base_goes_back_at_once() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    github.state().changed = Some(vec!["src/slug.ts".to_string(), "package.json".to_string()]);
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");

    let started = tokio::time::Instant::now();
    let report = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone())
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(report.outcome.as_str(), "changed");
    assert!(report.output.contains("package.json"), "{}", report.output);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(github.state().merges.is_empty(), "nothing merged");
    assert!(watches.started().is_empty(), "no hold waited on");
    assert!(repo.main().merges.is_empty());
}

/// A Tampered Run whose Control Files a person approved still merges
/// nothing while its base is held: it waits, then ends `held`, naming the
/// failing checks.
#[tokio::test(start_paused = true)]
async fn an_approved_tampered_run_on_a_held_base_is_held() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    github.state().changed = Some(vec!["src/slug.ts".to_string(), "package.json".to_string()]);
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    repo.hold_main(&REPOSITORY, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    let earlier = [approved_merge_gate(&["package.json"])];
    let mut merging = input(run_id);
    merging.earlier = &earlier;

    let report = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone())
        .merge(merging)
        .await
        .expect("merge");

    assert_eq!(report.outcome.as_str(), "held");
    assert!(
        report.output.contains("test (failure)"),
        "{}",
        report.output
    );
    assert!(github.state().merges.is_empty(), "nothing merged");
    assert_eq!(watches.started(), vec![MainWatch::of(run_id, &REPOSITORY)]);
}

/// A pull request merged outside the Run is recorded as the Run's merge,
/// so its base is watched, and the Step names the Control Files the Run
/// last kept.
#[tokio::test]
async fn a_pull_request_merged_outside_is_recorded_and_names_its_control_files() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    github.state().changed = Some(vec!["package.json".to_string()]);
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    let steps = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone());
    let ready = steps.merge(input(run_id)).await.expect("read once");
    assert_eq!(ready.outcome.as_str(), "changed");
    github.state().pull_request.as_mut().expect("opened").state = PullRequestState::Merged;

    let report = steps.merge(input(run_id)).await.expect("merge");

    assert_eq!(report.outcome.as_str(), "merged");
    assert!(report.output.contains("package.json"), "{}", report.output);
    assert_eq!(repo.main().merges.len(), 1);
    assert_eq!(watches.started(), vec![MainWatch::of(run_id, &REPOSITORY)]);
}

/// A pull request merged outside the Run before its merge Gate, as while
/// its checks ran, is recorded as the Run's merge and its base watched,
/// as one the merge Step finds is.
#[tokio::test]
async fn a_pull_request_merged_before_the_merge_gate_is_recorded_and_watched() {
    let repo = FakeRepo::default();
    let github = open_pull_request();
    github.state().pull_request.as_mut().expect("opened").state = PullRequestState::Merged;
    let watches = FakeWatches::default();
    let run_id = code_run(&repo).await;

    let report = PullRequestSteps::new(github.clone(), repo.clone(), watches.clone())
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(report.outcome.as_str(), "merged");
    assert!(
        github.state().merges.is_empty(),
        "nothing merged by the Run"
    );
    let merges = repo.main().merges.clone();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0].1.number, 7);
    assert_eq!(watches.started(), vec![MainWatch::of(run_id, &REPOSITORY)]);
}
