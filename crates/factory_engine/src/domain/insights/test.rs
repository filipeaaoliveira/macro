use ai_usage::domain::session_usage::SessionTokens;
use chrono::{DateTime, NaiveDate, Utc};
use factory_records::domain::insights::{
    DurationSpread, EntryCount, EntryCountKind, EntrySummary, EntryTrouble, InsightsRecords,
    InsightsScope, RunSummary, StepSessionRef,
};
use factory_records::domain::models::{EntryId, RunId, RunOutcome};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::fakes::{FakeRecords, FakeUsage};
use super::{
    DurationStats, INSIGHTS_RUN_CAP, InsightsError, InsightsQuery, InsightsService,
    InsightsServiceImpl, NamedCount, OutcomeCounts, TokenCounts,
};

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn day(date: &str) -> NaiveDate {
    date.parse().expect("valid date")
}

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

fn run_id(n: u128) -> RunId {
    RunId::from_uuid(Uuid::from_u128(n))
}

fn summary(
    n: u128,
    flow: Option<&str>,
    created_at: &str,
    end: Option<(RunOutcome, &str)>,
) -> RunSummary {
    RunSummary {
        id: run_id(n),
        title: format!("Run {n}"),
        flow: flow.map(str::to_string),
        created_at: at(created_at),
        end: end.map(|(outcome, ended_at)| (outcome, at(ended_at))),
    }
}

fn entry_summary(id: &str, steps: u32) -> EntrySummary {
    EntrySummary {
        entry: entry(id),
        steps,
        runs: steps,
        revisited_runs: 0,
        failed_runs: 0,
        troubled_runs: 0,
        duration: None,
        gates_opened: 0,
        gates_answered: 0,
        gate_wait: None,
    }
}

fn service(
    records: InsightsRecords,
    tokens: Vec<SessionTokens>,
) -> InsightsServiceImpl<FakeRecords, FakeUsage> {
    InsightsServiceImpl::new(FakeRecords::new(records), FakeUsage::new(tokens))
}

fn query(from: &str, to: &str, zone: &str) -> InsightsQuery {
    InsightsQuery::new(day(from), day(to), zone, None).expect("valid query")
}

#[tokio::test]
async fn the_period_is_read_from_midnight_to_midnight_in_its_time_zone() {
    let service = service(InsightsRecords::default(), Vec::new());

    let insights = service
        .insights(
            &julia(),
            query("2026-11-10", "2026-11-12", "America/New_York"),
        )
        .await
        .expect("insights");

    let asked = service.repo.asked.lock().expect("asked").clone();
    assert_eq!(
        asked,
        vec![InsightsScope {
            user_id: julia(),
            // Midnight in New York is 05:00 UTC in November.
            from: at("2026-11-10T05:00:00Z"),
            to: at("2026-11-13T05:00:00Z"),
            flow: None,
            cap: INSIGHTS_RUN_CAP,
        }]
    );
    // Every day is listed, even with no Runs.
    assert_eq!(
        insights.days.iter().map(|day| day.date).collect::<Vec<_>>(),
        vec![day("2026-11-10"), day("2026-11-11"), day("2026-11-12")]
    );
    assert_eq!(insights.time_zone, "America/New_York");
}

#[tokio::test]
async fn runs_are_counted_on_their_local_day_by_outcome_and_flow() {
    // In New York, 03:00 UTC on the 11th is still the evening of the 10th.
    let records = InsightsRecords {
        runs: vec![
            summary(4, None, "2026-11-12T15:00:00Z", None),
            summary(
                3,
                Some("plan"),
                "2026-11-11T15:00:00Z",
                Some((RunOutcome::Cancelled, "2026-11-11T15:10:00Z")),
            ),
            summary(
                2,
                Some("plan"),
                "2026-11-11T03:00:00Z",
                Some((RunOutcome::Failed, "2026-11-11T03:10:00Z")),
            ),
            summary(
                1,
                Some("reply"),
                "2026-11-10T15:00:00Z",
                Some((RunOutcome::Success, "2026-11-10T15:30:00Z")),
            ),
        ],
        ..InsightsRecords::default()
    };
    let service = service(records, Vec::new());

    let insights = service
        .insights(
            &julia(),
            query("2026-11-10", "2026-11-12", "America/New_York"),
        )
        .await
        .expect("insights");

    let counts = |running, success, failed, cancelled| OutcomeCounts {
        running,
        success,
        failed,
        cancelled,
        ..OutcomeCounts::default()
    };
    assert_eq!(insights.totals, counts(1, 1, 1, 1));
    assert_eq!(
        insights
            .days
            .iter()
            .map(|day| day.counts)
            .collect::<Vec<_>>(),
        vec![counts(0, 1, 1, 0), counts(0, 0, 0, 1), counts(1, 0, 0, 0)]
    );
    assert_eq!(
        insights
            .flows
            .iter()
            .map(|flow| (flow.flow.as_deref(), flow.counts))
            .collect::<Vec<_>>(),
        vec![
            (Some("plan"), counts(0, 0, 1, 1)),
            (None, counts(1, 0, 0, 0)),
            (Some("reply"), counts(0, 1, 0, 0)),
        ]
    );
    // The cancelled Run and the one still going are not timed: 10 and 30
    // minutes remain.
    assert_eq!(
        insights.run_duration,
        Some(DurationStats {
            count: 2,
            median_seconds: 1200.0,
            p90_seconds: 1680.0,
        })
    );
    assert!(!insights.capped);
}

/// A Run that failed for the factory's plumbing and a Run whose result was
/// found wrong count apart from a Run that failed for its work, and from
/// one that succeeded.
#[tokio::test]
async fn infrastructure_and_flawed_runs_count_apart_from_failed_and_success() {
    let records = InsightsRecords {
        runs: vec![
            summary(
                4,
                Some("code"),
                "2026-11-11T12:00:00Z",
                Some((RunOutcome::Flawed, "2026-11-11T13:00:00Z")),
            ),
            summary(
                3,
                Some("code"),
                "2026-11-11T11:00:00Z",
                Some((RunOutcome::Infrastructure, "2026-11-11T11:30:00Z")),
            ),
            summary(
                2,
                Some("code"),
                "2026-11-11T10:00:00Z",
                Some((RunOutcome::Failed, "2026-11-11T10:30:00Z")),
            ),
            summary(
                1,
                Some("code"),
                "2026-11-11T09:00:00Z",
                Some((RunOutcome::Success, "2026-11-11T09:30:00Z")),
            ),
        ],
        ..InsightsRecords::default()
    };
    let service = service(records, Vec::new());

    let insights = service
        .insights(&julia(), query("2026-11-11", "2026-11-11", "UTC"))
        .await
        .expect("insights");

    let kinds = OutcomeCounts {
        success: 1,
        failed: 1,
        infrastructure: 1,
        flawed: 1,
        ..OutcomeCounts::default()
    };
    assert_eq!(insights.totals, kinds);
    assert_eq!(insights.days[0].counts, kinds);
    assert_eq!(insights.flows[0].counts, kinds);
}

#[tokio::test]
async fn entries_follow_their_flows_order_with_their_outcomes_and_troubled_runs() {
    let records = InsightsRecords {
        runs: vec![
            summary(
                2,
                Some("code"),
                "2026-11-11T10:00:00Z",
                Some((RunOutcome::Manual, "2026-11-11T11:00:00Z")),
            ),
            summary(1, Some("code"), "2026-11-10T10:00:00Z", None),
        ],
        entries: vec![
            entry_summary("ci", 3),
            entry_summary("fix", 2),
            EntrySummary {
                gates_opened: 2,
                gates_answered: 1,
                gate_wait: Some(DurationSpread {
                    count: 1,
                    median_seconds: 60.0,
                    p90_seconds: 60.0,
                }),
                ..entry_summary("plan", 2)
            },
            EntrySummary {
                revisited_runs: 1,
                ..entry_summary("review", 3)
            },
        ],
        counts: vec![
            EntryCount {
                entry: entry("review"),
                kind: EntryCountKind::Outcome,
                value: "blockers".to_string(),
                count: 2,
            },
            EntryCount {
                entry: entry("review"),
                kind: EntryCountKind::Outcome,
                value: "clean".to_string(),
                count: 1,
            },
            EntryCount {
                entry: entry("plan"),
                kind: EntryCountKind::Answer,
                value: "approve".to_string(),
                count: 1,
            },
        ],
        trouble: vec![EntryTrouble {
            entry: entry("review"),
            run_id: run_id(2),
            failed: false,
            visits: 2,
        }],
        sessions: Vec::new(),
        capped: false,
    };
    let service = service(records, Vec::new());

    let insights = service
        .insights(&julia(), query("2026-11-10", "2026-11-12", "UTC"))
        .await
        .expect("insights");

    // The code Flow's order: plan, write, review, fix, publish, ci, ...
    assert_eq!(
        insights
            .entries
            .iter()
            .map(|entry| entry.entry.as_str())
            .collect::<Vec<_>>(),
        vec!["plan", "review", "fix", "ci"]
    );
    let review = &insights.entries[1];
    assert_eq!(
        review.outcomes,
        vec![
            NamedCount {
                name: "blockers".to_string(),
                count: 2
            },
            NamedCount {
                name: "clean".to_string(),
                count: 1
            },
        ]
    );
    assert_eq!(review.trouble.len(), 1);
    assert_eq!(review.trouble[0].run.title, "Run 2");
    assert_eq!(review.trouble[0].run.outcome, Some(RunOutcome::Manual));
    assert_eq!(review.gate, None);
    let gate = insights.entries[0].gate.as_ref().expect("plan's gate");
    assert_eq!((gate.opened, gate.answered), (2, 1));
    assert_eq!(
        gate.answers,
        vec![NamedCount {
            name: "approve".to_string(),
            count: 1
        }]
    );
}

#[tokio::test]
async fn usage_is_summed_by_entry_and_run_for_the_caller() {
    let session = |n: u128| Uuid::from_u128(100 + n);
    let records = InsightsRecords {
        runs: vec![
            summary(2, Some("plan"), "2026-11-11T10:00:00Z", None),
            summary(1, Some("plan"), "2026-11-10T10:00:00Z", None),
        ],
        entries: vec![entry_summary("plan", 2), entry_summary("write", 1)],
        sessions: vec![
            StepSessionRef {
                run_id: run_id(1),
                seq: 1,
                entry: entry("plan"),
                session_id: session(1),
            },
            StepSessionRef {
                run_id: run_id(1),
                seq: 2,
                entry: entry("write"),
                session_id: session(2),
            },
            // An agent on the person's own computer records no usage.
            StepSessionRef {
                run_id: run_id(2),
                seq: 1,
                entry: entry("plan"),
                session_id: session(3),
            },
        ],
        ..InsightsRecords::default()
    };
    let used = |n, input, output| SessionTokens {
        session_id: session(n),
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cost_usd: 0.0,
        unpriced_calls: 0,
    };
    let service = service(records, vec![used(1, 100, 10), used(2, 50, 5)]);

    let insights = service
        .insights(&julia(), query("2026-11-10", "2026-11-12", "UTC"))
        .await
        .expect("insights");

    assert_eq!(
        *service.usage.asked.lock().expect("asked"),
        vec![(julia(), 3)]
    );
    let tokens = |input, output| TokenCounts {
        input,
        output,
        ..TokenCounts::default()
    };
    assert_eq!(insights.usage.agent_steps, 3);
    assert_eq!(insights.usage.measured_steps, 2);
    assert_eq!(insights.usage.measured_runs, 1);
    assert_eq!(insights.usage.tokens, tokens(150, 15));
    assert_eq!(insights.usage.heaviest.len(), 1);
    assert_eq!(insights.usage.heaviest[0].run.id, run_id(1));
    assert_eq!(insights.entries[0].tokens, Some(tokens(100, 10)));
    assert_eq!(insights.entries[1].tokens, Some(tokens(50, 5)));
}

/// A Step that went on in a new session counts as one Step: one passed
/// to a backup Agent whose first session never started is measured by
/// the session that worked, and one whose first session was lost after
/// its agent worked in it sums both. A session two Steps of an entry
/// shared (one went on from the other's question) is two Steps, and its
/// tokens count once.
#[tokio::test]
async fn a_step_counts_once_however_many_sessions_it_worked_in() {
    let session = |n: u128| Uuid::from_u128(200 + n);
    let step = |run: u128, seq, name: &str, n| StepSessionRef {
        run_id: run_id(run),
        seq,
        entry: entry(name),
        session_id: session(n),
    };
    let records = InsightsRecords {
        runs: vec![
            summary(2, Some("plan"), "2026-11-11T10:00:00Z", None),
            summary(1, Some("plan"), "2026-11-10T10:00:00Z", None),
        ],
        entries: vec![entry_summary("plan", 3), entry_summary("write", 1)],
        sessions: vec![
            // Passed to the backup Agent: the primary's session never
            // started, so it recorded nothing.
            step(1, 1, "plan", 1),
            step(1, 1, "plan", 2),
            // Lost after its agent worked in it, then went on afresh.
            step(1, 2, "write", 3),
            step(1, 2, "write", 4),
            // Asked a question; the Step that went on shared its session.
            step(2, 1, "plan", 5),
            step(2, 2, "plan", 5),
        ],
        ..InsightsRecords::default()
    };
    let used = |n, input| SessionTokens {
        session_id: session(n),
        input,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cost_usd: 0.0,
        unpriced_calls: 0,
    };
    let service = service(
        records,
        vec![used(2, 100), used(3, 40), used(4, 60), used(5, 7)],
    );

    let insights = service
        .insights(&julia(), query("2026-11-10", "2026-11-12", "UTC"))
        .await
        .expect("insights");

    // Each session is read once.
    assert_eq!(
        *service.usage.asked.lock().expect("asked"),
        vec![(julia(), 5)]
    );
    let tokens = |input| TokenCounts {
        input,
        ..TokenCounts::default()
    };
    assert_eq!(insights.usage.agent_steps, 4);
    assert_eq!(insights.usage.measured_steps, 4);
    assert_eq!(insights.usage.measured_runs, 2);
    assert_eq!(insights.usage.tokens, tokens(207));
    assert_eq!(insights.entries[0].tokens, Some(tokens(107)));
    assert_eq!(insights.entries[1].tokens, Some(tokens(100)));
    let run_one = insights
        .usage
        .heaviest
        .iter()
        .find(|run| run.run.id == run_id(1))
        .expect("run 1");
    assert_eq!(run_one.tokens, tokens(200));
}

#[test]
fn a_period_that_ends_first_or_lasts_too_long_is_refused() {
    let backwards = InsightsQuery::new(day("2026-11-12"), day("2026-11-10"), "UTC", None);
    // 367 days, both ends included; 366 is the most.
    let too_long = InsightsQuery::new(day("2025-01-01"), day("2026-01-02"), "UTC", None);
    let longest = InsightsQuery::new(day("2025-01-01"), day("2026-01-01"), "UTC", None);

    assert!(matches!(backwards, Err(InsightsError::InvalidPeriod)));
    assert!(matches!(too_long, Err(InsightsError::InvalidPeriod)));
    assert!(longest.is_ok());
}

#[test]
fn a_flow_that_is_no_name_is_refused() {
    let flow = InsightsQuery::new(
        day("2026-11-10"),
        day("2026-11-10"),
        "UTC",
        Some("Plan; DROP".to_string()),
    );

    assert!(matches!(flow, Err(InsightsError::InvalidFlow(_))));
}

#[tokio::test]
async fn a_time_zone_the_database_does_not_name_reads_as_utc() {
    let service = service(InsightsRecords::default(), Vec::new());

    let insights = service
        .insights(&julia(), query("2026-11-10", "2026-11-10", "Mars/Olympus"))
        .await
        .expect("insights");

    assert_eq!(insights.time_zone, "UTC");
    assert_eq!(
        service.repo.asked.lock().expect("asked")[0].from,
        at("2026-11-10T00:00:00Z")
    );
}

#[test]
fn a_day_whose_midnight_a_clock_change_skips_starts_at_its_first_hour() {
    // Sao Paulo moved its clocks from midnight to 01:00 on 2018-11-04.
    let start = super::start_of_day(chrono_tz::America::Sao_Paulo, day("2018-11-04"));

    assert_eq!(start, Some(at("2018-11-04T03:00:00Z")));
}

#[tokio::test]
async fn a_period_with_more_runs_than_the_cap_says_so() {
    let records = InsightsRecords {
        capped: true,
        runs: vec![summary(1, Some("plan"), "2026-11-10T10:00:00Z", None)],
        ..InsightsRecords::default()
    };
    let service = service(records, Vec::new());

    let insights = service
        .insights(&julia(), query("2026-11-10", "2026-11-10", "UTC"))
        .await
        .expect("insights");

    assert!(insights.capped);
    assert_eq!(insights.run_cap, INSIGHTS_RUN_CAP);
}
