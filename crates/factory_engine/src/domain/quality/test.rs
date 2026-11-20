use ai_usage::domain::session_usage::SessionTokens;
use chrono::{DateTime, NaiveDate, Utc};
use factory_records::domain::insights::RunSummary;
use factory_records::domain::models::{RunId, RunOutcome};
use factory_records::domain::quality::{
    BlockerRecord, CiJob, CiJobResult, CiVerdict, CiWaitRecord, QualityRecords, QualityRun,
    ReviewRoundRecord, ReviewerRecord,
};
use factory_records::domain::review_rounds::{
    BlockerResolutions, DisputeRuling, FixStatus, ReviewRoundOutcome,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{CiCause, QualityQuery, ci_job, generic_step, summarize_quality};
use crate::domain::insights::fakes::{FakeRecords, FakeUsage};
use crate::domain::insights::{InsightsError, InsightsQuery, InsightsService, InsightsServiceImpl};
use crate::domain::models::{CheckState, FailureAnnotation, FinishedJob};

const APP: &str = "https://github.com/macro/app";

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn run_id(n: u128) -> RunId {
    RunId::from_uuid(Uuid::from_u128(n))
}

fn session(n: u128) -> Uuid {
    Uuid::from_u128(1000 + n)
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn query(repository: Option<&str>) -> QualityQuery {
    let period = InsightsQuery::new(
        NaiveDate::from_ymd_opt(2026, 11, 1).expect("day"),
        NaiveDate::from_ymd_opt(2026, 11, 30).expect("day"),
        "UTC",
        None,
    )
    .expect("valid period");
    QualityQuery::new(period, repository.map(str::to_string)).expect("valid query")
}

/// Run `n`, created on day `n` of November, on its own pull request, merged
/// with no person stepping in unless the test says otherwise.
fn run(n: u128) -> QualityRun {
    QualityRun {
        run: RunSummary {
            id: run_id(n),
            title: format!("Change {n}"),
            flow: Some("code".to_string()),
            created_at: at(&format!("2026-11-{n:02}T09:00:00Z")),
            end: Some((
                RunOutcome::Success,
                at(&format!("2026-11-{n:02}T12:00:00Z")),
            )),
        },
        repository: Some(APP.to_string()),
        pull_request: Some(format!("{APP}/pull/{n}")),
        merged: true,
        person_answered: false,
        steered: false,
        sessions: vec![session(n)],
    }
}

fn job(name: &str, result: CiJobResult, step: Option<&str>) -> CiJob {
    CiJob {
        name: name.to_string(),
        result,
        failed_step: step.map(str::to_string),
        failure: None,
        replaced: false,
    }
}

/// The job `name` that failed in `step`, in the test `failure`.
fn failing(name: &str, step: &str, failure: &str) -> CiJob {
    CiJob {
        failure: Some(failure.to_string()),
        ..job(name, CiJobResult::Failed, Some(step))
    }
}

/// A wait of Run `n` at place `seq`, from `started` to `ended` (hours of
/// day `n`), with `jobs`.
fn wait(
    n: u128,
    seq: u16,
    hours: (u32, u32),
    verdict: CiVerdict,
    jobs: Vec<CiJob>,
) -> CiWaitRecord {
    CiWaitRecord {
        run_id: run_id(n),
        seq,
        started_at: at(&format!("2026-11-{n:02}T{:02}:00:00Z", hours.0)),
        ended_at: at(&format!("2026-11-{n:02}T{:02}:00:00Z", hours.1)),
        verdict,
        jobs,
    }
}

fn tokens(n: u128, input: u64, cost_usd: f64, unpriced_calls: u64) -> SessionTokens {
    SessionTokens {
        session_id: session(n),
        input,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cost_usd,
        unpriced_calls,
    }
}

fn finished(state: CheckState, step: Option<&str>, detail: Option<&str>) -> FinishedJob {
    FinishedJob {
        name: "test".to_string(),
        state,
        failed_step: step.map(str::to_string),
        detail: detail.map(str::to_string),
        annotations: Vec::new(),
    }
}

fn annotation(path: &str, title: Option<&str>, message: &str) -> FailureAnnotation {
    FailureAnnotation {
        path: path.to_string(),
        title: title.map(str::to_string),
        message: message.to_string(),
    }
}

/// A failure is the CI's setup when a step that sets the job up failed, or
/// GitHub Actions says it could not get an action; the job's own otherwise.
#[test]
fn a_finished_job_is_classed_by_the_step_that_failed_and_its_output() {
    let failure = || CheckState::Failed("failure".to_string());
    let result = |job: FinishedJob| ci_job(&job).map(|job| job.result);

    assert_eq!(
        result(finished(failure(), Some("Run tests"), None)),
        Some(CiJobResult::Failed)
    );
    assert_eq!(
        result(finished(failure(), Some("Set up job"), None)),
        Some(CiJobResult::Setup)
    );
    assert_eq!(
        result(finished(failure(), Some("Run actions/setup-node@v4"), None)),
        Some(CiJobResult::Setup)
    );
    assert_eq!(
        result(finished(
            failure(),
            None,
            Some(
                "Error: Unable to resolve action `actions/checkout@v9`, unable to find version `v9`"
            )
        )),
        Some(CiJobResult::Setup)
    );
    let unresolved = FinishedJob {
        annotations: vec![annotation(
            ".github",
            None,
            "Unable to download action 'https://api.github.com/repos/actions/cache/tarball/v4'",
        )],
        ..finished(failure(), None, None)
    };
    assert_eq!(result(unresolved), Some(CiJobResult::Setup));
    assert_eq!(
        result(finished(
            CheckState::Failed("cancelled".to_string()),
            None,
            None
        )),
        Some(CiJobResult::Cancelled)
    );
    assert_eq!(
        result(finished(CheckState::Passed, None, None)),
        Some(CiJobResult::Passed)
    );
    assert_eq!(result(finished(CheckState::Waiting, None, None)), None);
}

/// A test's own output that mentions a 404, a run of tests, or a network
/// error is the pull request's failure, not the CI's setup: only a setup
/// step, or GitHub Actions' own words about an action, is setup.
#[test]
fn a_tests_output_is_never_read_as_setup() {
    let failure = || CheckState::Failed("failure".to_string());
    let result = |job: FinishedJob| ci_job(&job).map(|job| job.result);
    for detail in [
        "Ran 1404 tests: 1 failed",
        "assertion failed: expected 200, got 404",
        "Error: connect ETIMEDOUT 10.0.0.7:5432",
        "read ECONNRESET",
        "Could not resolve host: api.example.com",
        "Error: failed to download fixture from the test server: 404 Not Found",
        "the job says: unable to download action, but not at the start of a line",
    ] {
        assert_eq!(
            result(finished(failure(), Some("Run tests"), Some(detail))),
            Some(CiJobResult::Failed),
            "{detail}"
        );
        assert_eq!(
            result(finished(failure(), None, Some(detail))),
            Some(CiJobResult::Failed),
            "{detail}"
        );
        let annotated = FinishedJob {
            annotations: vec![annotation("src/client.rs", Some("client::fetch"), detail)],
            ..finished(failure(), None, None)
        };
        assert_eq!(result(annotated), Some(CiJobResult::Failed), "{detail}");
    }
}

/// A failed job names the first test its check run annotated, by file and
/// title, in the same order however GitHub lists them; GitHub Actions' own
/// errors, and an annotation with no title, name none.
#[test]
fn a_failed_job_names_the_first_test_it_annotated() {
    let failure = || CheckState::Failed("failure".to_string());
    let named = |annotations: Vec<FailureAnnotation>| {
        ci_job(&FinishedJob {
            annotations,
            ..finished(failure(), Some("Run tests"), None)
        })
        .and_then(|job| job.failure)
    };

    assert_eq!(
        named(vec![
            annotation(".github", None, "Process completed with exit code 101."),
            annotation(
                "src/parser.rs",
                Some("parser::test::empty"),
                "assertion failed"
            ),
            annotation(
                "src/lexer.rs",
                None,
                "\nlexer::test::spaces panicked\nat line 3"
            ),
        ]),
        Some("src/parser.rs: parser::test::empty".to_string())
    );
    // A message is log output, never kept: an annotation with no title
    // names no test.
    assert_eq!(
        named(vec![annotation(
            "src/lexer.rs",
            None,
            "token=abc123 lexer::test::spaces panicked"
        )]),
        None
    );
    // A long title is cut to a test's name's length.
    let long = named(vec![annotation("src/a.rs", Some(&"é".repeat(300)), "")]).expect("named");
    assert!(long.len() <= 200, "{}", long.len());
    assert!(long.starts_with("src/a.rs: é"));
    assert_eq!(
        named(vec![annotation(
            ".github",
            None,
            "Process completed with exit code 1."
        )]),
        None
    );
    let passed = ci_job(&FinishedJob {
        annotations: vec![annotation("src/parser.rs", Some("parser"), "fine")],
        ..finished(CheckState::Passed, None, None)
    })
    .expect("kept");
    assert_eq!(passed.failure, None);
}

/// Only steps that set the job up are setup: GitHub Actions' own, checkout,
/// cache, installing dependencies or a known toolchain. A step named "Setup"
/// that runs the pull request's own work, as its database migrations, is
/// the pull request's failure.
#[test]
fn only_steps_that_set_the_job_up_are_setup() {
    let failure = || CheckState::Failed("failure".to_string());
    let result = |step: &str| ci_job(&finished(failure(), Some(step), None)).map(|job| job.result);
    for step in [
        "Set up job",
        "Checkout",
        "Run actions/checkout@v4",
        "Run actions/setup-python@v5",
        "Run actions/cache@v4",
        "Install dependencies",
        "Setup Node.js",
        "Set up Go 1.23",
        "Install Rust",
        "Initialize containers",
    ] {
        assert_eq!(result(step), Some(CiJobResult::Setup), "{step}");
    }
    for step in [
        "Setup test database",
        "Set up database schema",
        "Install the app's migrations",
        "Download fixtures",
        "Setup",
    ] {
        assert_eq!(result(step), Some(CiJobResult::Failed), "{step}");
    }
}

/// A step that runs the project's work as a whole is generic; one named for
/// something narrower, or for an action, is not.
#[test]
fn generic_steps_are_told_from_named_ones() {
    for step in [
        "Run tests",
        "Run cargo test --workspace",
        "Run npm ci && npm test",
        "Test",
        "Build",
        "cargo clippy",
        "Typecheck",
        "Run e2e",
    ] {
        assert!(generic_step(step), "{step}");
    }
    for step in [
        "Upload coverage",
        "Deploy preview",
        "Run codecov/codecov-action@v4",
        "Wait for Postgres",
    ] {
        assert!(!generic_step(step), "{step}");
    }
}

/// Two pull requests that broke two different tests under the same generic
/// step each failed in their own work: neither is flaky, and nothing recurs.
#[test]
fn different_tests_failing_under_the_same_step_are_each_their_own() {
    let records = QualityRecords {
        runs: vec![run(2), run(1)],
        ci_waits: vec![
            wait(
                1,
                5,
                (9, 10),
                CiVerdict::Red,
                vec![failing(
                    "test",
                    "Run tests",
                    "src/parser.rs: parser::test::empty",
                )],
            ),
            wait(
                2,
                5,
                (9, 10),
                CiVerdict::Red,
                vec![failing(
                    "test",
                    "Run tests",
                    "src/lexer.rs: lexer::test::spaces",
                )],
            ),
        ],
        ..QualityRecords::default()
    };

    let ci = summarize_quality(&query(None), records, &[]).ci;

    assert_eq!(ci.causes.own, 2);
    assert_eq!(ci.causes.flaky, 0);
    assert!(ci.recurring.is_empty());
}

/// A failure with nothing finer than its job, a generic step, or no step
/// read at all (a commit status, another app's check, or a job past the
/// reads' cap), is its pull request's own however many pull requests fail
/// so.
#[test]
fn a_failure_with_no_identity_is_its_pull_requests_own() {
    let records = QualityRecords {
        runs: vec![run(3), run(2), run(1)],
        ci_waits: [1, 2, 3]
            .into_iter()
            .map(|n| {
                wait(
                    n,
                    5,
                    (9, 10),
                    CiVerdict::Red,
                    vec![
                        job("test", CiJobResult::Failed, Some("Run tests")),
                        job("ci/status", CiJobResult::Failed, None),
                    ],
                )
            })
            .collect(),
        ..QualityRecords::default()
    };

    let ci = summarize_quality(&query(None), records, &[]).ci;

    assert_eq!(ci.causes.own, 6);
    assert_eq!(ci.causes.flaky, 0);
    assert!(ci.recurring.is_empty());
}

/// A setup failure on two pull requests is listed once, as setup; the same
/// test, or the same named step, failing on two pull requests is flaky; a
/// failure on one pull request is that pull request's own; a cancelled run
/// is neither red nor the pull request's.
#[test]
fn ci_failures_are_classed_by_how_many_pull_requests_they_hit() {
    let records = QualityRecords {
        runs: vec![run(3), run(2), run(1)],
        ci_waits: vec![
            wait(
                1,
                5,
                (9, 10),
                CiVerdict::Red,
                vec![
                    job("build", CiJobResult::Setup, Some("Set up job")),
                    failing("test", "Run tests", "src/net.rs: net::test::retry"),
                    job("upload", CiJobResult::Failed, Some("Upload coverage")),
                    job("lint", CiJobResult::Passed, None),
                ],
            ),
            wait(
                2,
                5,
                (9, 10),
                CiVerdict::Red,
                vec![
                    job("build", CiJobResult::Setup, Some("Set up job")),
                    failing("test", "Run tests", "src/net.rs: net::test::retry"),
                    job("upload", CiJobResult::Failed, Some("Upload coverage")),
                    job("e2e", CiJobResult::Failed, Some("Run e2e")),
                    job("lint", CiJobResult::Cancelled, None),
                ],
            ),
        ],
        ..QualityRecords::default()
    };

    let ci = summarize_quality(&query(None), records, &[]).ci;

    assert_eq!(ci.causes.setup, 2);
    assert_eq!(ci.causes.flaky, 4);
    assert_eq!(ci.causes.own, 1);
    assert_eq!(ci.causes.cancelled, 1);
    let recurring: Vec<_> = ci
        .recurring
        .iter()
        .map(|failure| {
            (
                failure.job.as_str(),
                failure.step.as_deref(),
                failure.failure.as_deref(),
                failure.cause,
                failure.pull_requests,
            )
        })
        .collect();
    assert_eq!(
        recurring,
        vec![
            ("build", Some("Set up job"), None, CiCause::Setup, 2),
            (
                "test",
                Some("Run tests"),
                Some("src/net.rs: net::test::retry"),
                CiCause::Flaky,
                2
            ),
            ("upload", Some("Upload coverage"), None, CiCause::Flaky, 2),
        ]
    );
    let e2e = ci.jobs.iter().find(|job| job.name == "e2e").expect("e2e");
    assert_eq!(e2e.causes.own, 1);
    assert_eq!(e2e.pull_requests_failed, 1);
    let lint = ci.jobs.iter().find(|job| job.name == "lint").expect("lint");
    assert_eq!((lint.finished, lint.red, lint.red_rate), (1, 0, 0.0));
    assert_eq!(lint.causes.cancelled, 1);
    let build = ci
        .jobs
        .iter()
        .find(|job| job.name == "build")
        .expect("build");
    assert_eq!((build.finished, build.red, build.red_rate), (2, 2, 1.0));
}

/// The same failure twice on one pull request, in two Runs, is still that
/// pull request's own.
#[test]
fn two_runs_on_one_pull_request_are_one_pull_request() {
    let mut again = run(2);
    again.pull_request = Some(format!("{APP}/pull/1"));
    let failing = |n| {
        wait(
            n,
            5,
            (9, 10),
            CiVerdict::Red,
            vec![job("test", CiJobResult::Failed, Some("Run tests"))],
        )
    };
    let records = QualityRecords {
        runs: vec![again, run(1)],
        ci_waits: vec![failing(1), failing(2)],
        ..QualityRecords::default()
    };

    let ci = summarize_quality(&query(None), records, &[]).ci;

    assert_eq!(ci.causes.own, 2);
    assert_eq!(ci.causes.flaky, 0);
    assert!(ci.recurring.is_empty());
}

/// From a pull request's first wait to its first green wait, and the fix
/// rounds Runs spent on CI apart from those spent on review.
#[test]
fn time_to_green_and_fix_rounds_are_counted_per_run() {
    let records = QualityRecords {
        runs: vec![run(2), run(1)],
        ci_waits: vec![
            // Red at 10, fixed, green at 12: three hours from 9.
            wait(1, 5, (9, 10), CiVerdict::Red, Vec::new()),
            wait(1, 7, (11, 12), CiVerdict::Green, Vec::new()),
            wait(1, 9, (13, 14), CiVerdict::Green, Vec::new()),
            // Green at once, in an hour.
            wait(2, 5, (9, 10), CiVerdict::Green, Vec::new()),
        ],
        review_rounds: vec![
            ReviewRoundRecord {
                run_id: run_id(1),
                seq: 2,
                started_at: at("2026-11-01T08:00:00Z"),
                outcome: Some(ReviewRoundOutcome::Changes),
            },
            ReviewRoundRecord {
                run_id: run_id(1),
                seq: 4,
                started_at: at("2026-11-01T08:30:00Z"),
                outcome: Some(ReviewRoundOutcome::Pass),
            },
        ],
        ..QualityRecords::default()
    };

    let ci = summarize_quality(&query(None), records, &[]).ci;

    assert_eq!((ci.waits, ci.red_waits, ci.runs), (4, 1, 2));
    assert_eq!(ci.ci_fix_rounds, 1);
    assert_eq!(ci.runs_with_ci_fixes, 1);
    assert_eq!(ci.review_fix_rounds, 1);
    let to_green = ci.first_push_to_green.expect("measured");
    assert_eq!(to_green.count, 2);
    assert_eq!(to_green.median_seconds, 2.0 * 3600.0);
}

/// Rounds per Run, the share clean after their first round, Blockers by
/// reviewer and by Agent, new against repeated, and the texts that recur
/// across Runs.
#[test]
fn review_rounds_and_blockers_are_counted() {
    let round = |n: u128, seq: u16, outcome| ReviewRoundRecord {
        run_id: run_id(n),
        seq,
        started_at: at(&format!("2026-11-{n:02}T{:02}:00:00Z", seq)),
        outcome: Some(outcome),
    };
    let reviewer = |n: u128, seq: u16, round_seq: u16, role: &str, agent: &str| ReviewerRecord {
        run_id: run_id(n),
        seq,
        round_seq,
        role: Some(role.to_string()),
        agent: Some(agent.to_string()),
    };
    let blocker = |n: u128, seq: u16, round_seq: u16, problem: &str| BlockerRecord {
        run_id: run_id(n),
        seq,
        round_seq,
        problem: problem.to_string(),
        fix_status: None,
        ruling: None,
        dismissed: false,
    };
    let records = QualityRecords {
        runs: vec![run(3), run(2), run(1)],
        review_rounds: vec![
            round(1, 2, ReviewRoundOutcome::Changes),
            round(1, 5, ReviewRoundOutcome::Pass),
            round(2, 2, ReviewRoundOutcome::Changes),
            round(3, 2, ReviewRoundOutcome::Pass),
        ],
        reviewers: vec![
            reviewer(1, 2, 2, "correctness-reviewer", "primary"),
            reviewer(1, 3, 2, "scope-reviewer", "backup"),
            reviewer(1, 5, 5, "correctness-reviewer", "primary"),
            reviewer(2, 2, 2, "correctness-reviewer", "backup"),
            reviewer(3, 2, 2, "correctness-reviewer", "primary"),
        ],
        blockers: vec![
            blocker(1, 2, 2, "The empty string panics."),
            blocker(1, 3, 2, "The change renames a public type"),
            // Found again by the next round: not fixed.
            blocker(1, 5, 5, "the empty  string panics"),
            blocker(2, 2, 2, "The empty string panics"),
        ],
        ..QualityRecords::default()
    };

    let review = summarize_quality(&query(None), records, &[]).review;

    assert_eq!((review.reviewed_runs, review.rounds), (3, 4));
    assert_eq!(review.rounds_per_run, Some(4.0 / 3.0));
    assert_eq!((review.first_rounds, review.clean_first_rounds), (3, 1));
    assert_eq!(review.clean_first_rate, Some(1.0 / 3.0));
    assert_eq!(
        (
            review.blockers,
            review.new_blockers,
            review.repeated_blockers
        ),
        (4, 3, 1)
    );
    let counts = |counts: &[super::BlockerCount]| {
        counts
            .iter()
            .map(|count| (count.name.clone(), count.reviews, count.blockers))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        counts(&review.by_reviewer),
        vec![
            ("correctness-reviewer".to_string(), 4, 3),
            ("scope-reviewer".to_string(), 1, 1),
        ]
    );
    assert_eq!(
        counts(&review.by_agent),
        vec![("backup".to_string(), 2, 2), ("primary".to_string(), 3, 2)]
    );
    assert_eq!(review.recurring.len(), 1);
    let recurring = &review.recurring[0];
    assert_eq!(recurring.text, "The empty string panics");
    assert_eq!((recurring.runs, recurring.blockers), (2, 3));
    assert_eq!(
        recurring.reviewers,
        vec!["correctness-reviewer".to_string()]
    );
    assert_eq!(
        recurring
            .run_links
            .iter()
            .map(|link| link.id)
            .collect::<Vec<_>>(),
        vec![run_id(2), run_id(1)]
    );
}

/// The Blockers given to a fix are counted fixed, disputed and upheld, or
/// disputed and dropped; a dispute no round ruled on counts as upheld, and
/// a Blocker no fix was given counts in none. A dismissed finding counts
/// only as dismissed.
#[test]
fn blockers_fixed_upheld_and_dropped_are_counted() {
    let blocker = |seq: u16, fix_status, ruling| BlockerRecord {
        run_id: run_id(1),
        seq,
        round_seq: 2,
        problem: format!("Problem {seq}"),
        fix_status,
        ruling,
        dismissed: false,
    };
    let records = QualityRecords {
        runs: vec![run(1)],
        blockers: vec![
            blocker(2, Some(FixStatus::Fixed), None),
            blocker(3, Some(FixStatus::Fixed), None),
            blocker(4, Some(FixStatus::Disputed), Some(DisputeRuling::Upheld)),
            blocker(5, Some(FixStatus::Disputed), None),
            blocker(6, Some(FixStatus::Disputed), Some(DisputeRuling::Dropped)),
            blocker(7, None, None),
            BlockerRecord {
                dismissed: true,
                ..blocker(8, None, None)
            },
        ],
        ..QualityRecords::default()
    };

    let review = summarize_quality(&query(None), records, &[]).review;

    assert_eq!(
        review.resolutions,
        BlockerResolutions {
            fixed: 2,
            disputed_upheld: 2,
            disputed_dropped: 1,
            dismissed: 1,
        }
    );
    // A dismissed finding is no Blocker.
    assert_eq!(review.blockers, 6);
}

/// A Run whose Gate a person answered, one its owner steered and one left
/// to a person are not autonomous; one that merged with none of these is.
#[test]
fn a_run_a_person_stepped_into_is_not_autonomous() {
    let mut answered = run(2);
    answered.person_answered = true;
    let mut steered = run(3);
    steered.steered = true;
    let mut manual = run(4);
    manual.merged = false;
    manual.run.end = Some((RunOutcome::Manual, at("2026-11-04T12:00:00Z")));
    let mut going = run(5);
    going.merged = false;
    going.run.end = None;
    let records = QualityRecords {
        runs: vec![going, manual, steered, answered, run(1)],
        ..QualityRecords::default()
    };

    let delivery = summarize_quality(&query(None), records, &[]).delivery;

    assert_eq!(delivery.ended_runs, 4);
    assert_eq!(delivery.merged, 3);
    assert_eq!(delivery.autonomous, 1);
    assert_eq!(delivery.autonomy_rate, Some(0.25));
    assert_eq!(
        (delivery.person_answered, delivery.steered, delivery.manual),
        (1, 1, 1)
    );
}

/// Every Run's usage per merged pull request: in money when every call was
/// priced, in tokens otherwise.
#[test]
fn cost_per_merged_pull_request_is_money_where_priced_and_tokens_otherwise() {
    let mut unmerged = run(2);
    unmerged.merged = false;
    unmerged.run.end = Some((RunOutcome::Failed, at("2026-11-02T12:00:00Z")));
    let records = || QualityRecords {
        runs: vec![unmerged.clone(), run(1)],
        ..QualityRecords::default()
    };

    let priced = summarize_quality(
        &query(None),
        records(),
        &[tokens(1, 1000, 1.5, 0), tokens(2, 500, 0.5, 0)],
    )
    .delivery;
    let unpriced = summarize_quality(
        &query(None),
        records(),
        &[tokens(1, 1000, 1.5, 0), tokens(2, 500, 0.0, 3)],
    )
    .delivery;

    assert_eq!(priced.merged, 1);
    assert_eq!(priced.cost_per_merged_usd, Some(2.0));
    assert_eq!(priced.tokens_per_merged, Some(1500));
    assert_eq!(unpriced.cost_per_merged_usd, None);
    assert_eq!(unpriced.unpriced_calls, 3);
    assert_eq!(unpriced.tokens_per_merged, Some(1500));
}

/// The service reads the repository asked for, by its URL, and the usage
/// of the Runs' sessions; a repository that names none is refused.
#[tokio::test]
async fn the_service_reads_one_repository_and_its_runs_usage() {
    let records = FakeRecords::new(Default::default()).with_quality(QualityRecords {
        runs: vec![run(1)],
        repositories: vec![APP.to_string()],
        ..QualityRecords::default()
    });
    let asked = records.asked_quality.clone();
    let service = InsightsServiceImpl::new(records, FakeUsage::new(vec![tokens(1, 100, 0.25, 0)]));

    let quality = service
        .quality(&julia(), query(Some("macro/app")))
        .await
        .expect("quality");

    assert_eq!(quality.repository.as_deref(), Some(APP));
    assert_eq!(quality.repositories, vec![APP.to_string()]);
    assert_eq!(quality.delivery.cost_per_merged_usd, Some(0.25));
    let asked = asked.lock().expect("asked").clone();
    assert_eq!(asked[0].repository.as_deref(), Some(APP));
    assert_eq!(asked[0].runs.user_id, julia());

    let period = InsightsQuery::new(
        NaiveDate::from_ymd_opt(2026, 11, 1).expect("day"),
        NaiveDate::from_ymd_opt(2026, 11, 30).expect("day"),
        "UTC",
        None,
    )
    .expect("valid period");
    assert!(matches!(
        QualityQuery::new(period.clone(), Some("not a repository".to_string())),
        Err(InsightsError::InvalidRepository(_))
    ));
    assert!(matches!(
        QualityQuery::new(period, Some("https://gitlab.com/macro/app".to_string())),
        Err(InsightsError::InvalidRepository(_))
    ));
}

/// A repository named in another case, by URL with a `.git` ending or a
/// closing slash, or without its scheme, is the same repository, and reads
/// back as the Runs name it.
#[tokio::test]
async fn a_repository_is_matched_however_it_is_written() {
    for named in [
        "Macro/App",
        "macro/app.git",
        "https://GitHub.com/Macro/App/",
        "http://github.com/macro/app.git",
        "github.com/macro/app",
        " https://github.com/macro/app ",
    ] {
        let records = FakeRecords::new(Default::default()).with_quality(QualityRecords {
            runs: vec![run(1)],
            repositories: vec![APP.to_string()],
            ..QualityRecords::default()
        });
        let asked = records.asked_quality.clone();
        let service = InsightsServiceImpl::new(records, FakeUsage::new(Vec::new()));

        let quality = service
            .quality(&julia(), query(Some(named)))
            .await
            .expect("quality");

        let asked = asked.lock().expect("asked").clone();
        assert_eq!(asked[0].repository.as_deref(), Some(APP), "{named}");
        assert_eq!(quality.repository.as_deref(), Some(APP), "{named}");
    }
}
