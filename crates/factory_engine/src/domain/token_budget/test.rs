use ai_usage::domain::session_usage::SessionTokens;
use factory_records::domain::models::BudgetStop;
use uuid::Uuid;

use super::*;

fn session(input: u64, output: u64, cache_read: u64, cache_write: u64) -> SessionTokens {
    SessionTokens {
        session_id: Uuid::now_v7(),
        input,
        output,
        cache_read,
        cache_write,
        cost_usd: 0.0,
        unpriced_calls: 0,
        reported_by_runtime: false,
        api_key: None,
    }
}

/// A Run's use counts every token Insights counts: input, output, and
/// input read from or written to the prompt cache, across its sessions.
#[test]
fn a_run_s_use_counts_what_insights_counts() {
    let used = tokens_used(&[session(1, 20, 300, 4_000), session(50_000, 0, 0, 0)]);
    assert_eq!(used, 54_321);
    assert_eq!(tokens_used(&[]), 0);
    assert_eq!(
        tokens_used(&[session(u64::MAX, 1, 0, 0), session(1, 0, 0, 0)]),
        u64::MAX
    );
}

/// The budget stops a Run once it used as much or more; an unread log
/// lets it go on until the last unread check it may take.
#[test]
fn the_verdict_stops_at_the_budget_and_after_too_many_unread() {
    assert_eq!(verdict(100, Some(99), 0), Ok(BudgetVerdict::Within));
    assert_eq!(verdict(100, Some(99), 2), Ok(BudgetVerdict::Within));
    assert_eq!(
        verdict(100, Some(100), 0),
        Err(BudgetStop::Spent { used: 100 })
    );
    assert_eq!(
        verdict(100, Some(250), 0),
        Err(BudgetStop::Spent { used: 250 })
    );
    assert_eq!(verdict(100, None, 0), Ok(BudgetVerdict::Unread));
    assert_eq!(
        verdict(100, None, MAX_UNREAD_CHECKS - 2),
        Ok(BudgetVerdict::Unread)
    );
    assert_eq!(
        verdict(100, None, MAX_UNREAD_CHECKS - 1),
        Err(BudgetStop::Unread)
    );
    assert_eq!(verdict(100, None, u8::MAX), Err(BudgetStop::Unread));
}

/// The notice fits the notification's excerpt whole, so the owner reads
/// why without opening the Run.
#[test]
fn the_notice_says_why_in_full() {
    for stop in [
        BudgetStop::Spent {
            used: 10_000_000_000,
        },
        BudgetStop::Unread,
    ] {
        let text = stop_text(10_000_000_000, stop);
        assert_eq!(FactoryGateWaitingMetadata::excerpt(&text), text);
    }
}

/// A budget check's activity input and result keep their names: Runs in
/// flight carry them in their histories.
#[test]
fn a_check_reads_as_its_history_keeps_it() {
    let run_id = RunId::generate();
    let check = BudgetCheck { run_id, unread: 2 };
    let json = serde_json::to_value(check).expect("serialize");
    assert_eq!(json["unread"], 2);
    assert_eq!(
        serde_json::from_value::<BudgetCheck>(json).expect("read"),
        check
    );
    assert_eq!(
        serde_json::to_value(BudgetVerdict::Stopped).expect("serialize"),
        "stopped"
    );
}

/// Sessions that recorded no usage at all are not measured; a Run with no
/// sessions yet used nothing; any recorded usage is measured.
#[test]
fn use_is_measured_only_when_a_session_recorded_some() {
    let sessions = [Uuid::now_v7(), Uuid::now_v7()];
    assert_eq!(measured(&sessions, &[]), None);
    assert_eq!(measured(&[], &[]), Some(0));
    assert_eq!(
        measured(&sessions, &[session(0, 0, 0, 0)]),
        Some(0),
        "a session that recorded calls of no tokens is measured"
    );
    assert_eq!(measured(&sessions, &[session(10, 5, 0, 0)]), Some(15));
}
