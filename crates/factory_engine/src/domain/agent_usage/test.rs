use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use factory_records::domain::insights::{
    EntrySummary, InsightsRecords, RunSummary, StepSessionRef,
};
use factory_records::domain::models::{EntryId, RunId};
use factory_records::domain::session_usage::{SessionUsage, SessionUsageRepository};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::AgentUsageReader;
use crate::domain::fakes::FakeRepo;
use crate::domain::insights::fakes::{FakeRecords, FakeUsage};
use crate::domain::insights::{InsightsQuery, InsightsService, InsightsServiceImpl, TokenCounts};

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn session(n: u128) -> Uuid {
    Uuid::from_u128(100 + n)
}

/// What Julia's session `n` reported, with its cost if any.
fn reported(n: u128, input: u64, output: u64, cost_usd: Option<f64>) -> SessionUsage {
    SessionUsage {
        session_id: session(n),
        user_id: julia(),
        bot_id: Uuid::from_u128(7),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 1000,
        cache_write_tokens: 10,
        cost_usd,
        model: Some("claude-sonnet-5-5".to_string()),
    }
}

/// What the usage log recorded for session `n`, priced.
fn logged(n: u128, input: u64, cost_usd: f64) -> SessionTokens {
    SessionTokens {
        session_id: session(n),
        input,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cost_usd,
        unpriced_calls: 0,
    }
}

async fn kept(usage: &[SessionUsage]) -> FakeRepo {
    let repo = FakeRepo::default();
    for usage in usage {
        repo.record_session_usage(usage).await.expect("record");
    }
    repo
}

/// A session whose runtime reported its usage reads as it reported it,
/// and only the others are read from the usage log, so none counts twice.
#[tokio::test]
async fn kept_usage_is_read_and_the_rest_from_the_usage_log() {
    let repo = kept(&[reported(1, 6, 1165, Some(0.27))]).await;
    // The usage log has session 1 too, as a runtime on the model router
    // would record it: it is not read for it.
    let log = FakeUsage::new(vec![logged(1, 999, 9.0), logged(2, 40, 0.01)]);
    let reader = AgentUsageReader::new(log, repo);

    let mut read = reader
        .session_tokens(&julia(), &[session(1), session(2), session(3)])
        .await
        .expect("read");
    read.sort_by_key(|tokens| tokens.session_id);

    assert_eq!(
        read,
        vec![
            SessionTokens {
                session_id: session(1),
                input: 6,
                output: 1165,
                cache_read: 1000,
                cache_write: 10,
                cost_usd: 0.27,
                unpriced_calls: 0,
            },
            logged(2, 40, 0.01),
        ]
    );
    assert_eq!(*reader.log.asked.lock().expect("asked"), vec![(julia(), 2)]);
}

/// A session whose runtime reported tokens but no cost keeps its tokens,
/// and its cost is unknown: one call with no price.
#[tokio::test]
async fn a_session_that_reported_no_cost_is_unpriced() {
    let repo = kept(&[reported(1, 6, 1165, None)]).await;
    let reader = AgentUsageReader::new(FakeUsage::new(Vec::new()), repo);

    let read = reader
        .session_tokens(&julia(), &[session(1)])
        .await
        .expect("read");

    assert_eq!(read.len(), 1);
    assert_eq!((read[0].input, read[0].output), (6, 1165));
    assert_eq!(read[0].unpriced_calls, 1);
}

/// Someone else's kept session reads as no usage, as in the usage log.
#[tokio::test]
async fn someone_elses_kept_session_is_not_read() {
    let repo = kept(&[reported(1, 6, 1165, Some(0.27))]).await;
    let reader = AgentUsageReader::new(FakeUsage::new(Vec::new()), repo);
    let mallory = MacroUserIdStr::try_from("macro|mallory@example.com".to_string()).expect("id");

    let read = reader
        .session_tokens(&mallory, &[session(1)])
        .await
        .expect("read");

    assert!(read.is_empty());
}

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

/// One Run, its plan Step in session 1 and its write Step in session 2.
fn records() -> InsightsRecords {
    let run_id = RunId::from_uuid(Uuid::from_u128(1));
    let entry_summary = |id: &str| EntrySummary {
        entry: entry(id),
        steps: 1,
        runs: 1,
        revisited_runs: 0,
        failed_runs: 0,
        troubled_runs: 0,
        duration: None,
        gates_opened: 0,
        gates_answered: 0,
        gate_wait: None,
    };
    let step = |seq: u16, id: &str, n: u128| StepSessionRef {
        run_id,
        seq,
        entry: entry(id),
        session_id: session(n),
    };
    InsightsRecords {
        runs: vec![RunSummary {
            id: run_id,
            title: "Run 1".to_string(),
            flow: Some("code".to_string()),
            created_at: "2026-11-24T10:00:00Z".parse().expect("time"),
            end: None,
        }],
        entries: vec![entry_summary("plan"), entry_summary("write")],
        sessions: vec![step(1, "plan", 1), step(2, "write", 2)],
        ..InsightsRecords::default()
    }
}

fn query() -> InsightsQuery {
    InsightsQuery::new(
        "2026-11-24".parse().expect("day"),
        "2026-11-24".parse().expect("day"),
        "UTC",
        None,
    )
    .expect("valid query")
}

/// Insights sum the usage Steps' runtimes reported: tokens per entry and
/// in all, and each entry's cost.
#[tokio::test]
async fn insights_sum_the_kept_usage() {
    let repo = kept(&[
        reported(1, 6, 1165, Some(0.27)),
        reported(2, 20, 2287, Some(0.14)),
    ])
    .await;
    let service = InsightsServiceImpl::new(
        FakeRecords::new(records()),
        AgentUsageReader::new(FakeUsage::new(Vec::new()), repo),
    );

    let insights = service.insights(&julia(), query()).await.expect("insights");

    assert_eq!(insights.usage.agent_steps, 2);
    assert_eq!(insights.usage.measured_steps, 2);
    assert_eq!(
        insights.usage.tokens,
        TokenCounts {
            input: 26,
            output: 3452,
            cache_read: 2000,
            cache_write: 20,
        }
    );
    let cost = |id: &str| {
        insights
            .entries
            .iter()
            .find(|entry| entry.entry.as_str() == id)
            .expect("entry")
            .cost_usd
    };
    assert_eq!(cost("plan"), Some(0.27));
    assert_eq!(cost("write"), Some(0.14));
}

/// An entry with a session whose runtime reported no cost gives no cost,
/// though its tokens count.
#[tokio::test]
async fn insights_withhold_the_cost_of_an_entry_with_no_cost_reported() {
    let repo = kept(&[
        reported(1, 6, 1165, Some(0.27)),
        reported(2, 20, 2287, None),
    ])
    .await;
    let service = InsightsServiceImpl::new(
        FakeRecords::new(records()),
        AgentUsageReader::new(FakeUsage::new(Vec::new()), repo),
    );

    let insights = service.insights(&julia(), query()).await.expect("insights");

    let write = insights
        .entries
        .iter()
        .find(|entry| entry.entry.as_str() == "write")
        .expect("entry");
    assert_eq!(write.cost_usd, None);
    assert_eq!(write.tokens.map(|tokens| tokens.output), Some(2287));
}
