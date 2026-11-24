use super::*;
use agent_fold::testing::parse_log;
use serde_json::{Value, json};

fn frame(direction: &str, mut content: Value) -> StoredAgentSessionLog {
    content["type"] = json!("acp");
    content["jsonrpc"] = json!("2.0");
    StoredAgentSessionLog {
        id: macro_uuid::generate_uuid_v7(),
        created_at: chrono::Utc::now(),
        entry: parse_log(&json!({"direction": direction, "content": content}).to_string())
            .pop()
            .unwrap(),
    }
}

/// A prompt response reporting its turn's tokens, as claude-agent-acp
/// sends it through macrod.
fn turn(id: &str, input: u64, output: u64, read: u64, write: u64) -> StoredAgentSessionLog {
    let count = json!({
        "inputTokens": input,
        "outputTokens": output,
        "cachedInputTokens": read,
        "cachedWriteTokens": write,
        "totalTokens": input + output + read + write,
    });
    frame(
        "to_server",
        json!({"id": id, "result": {
            "stopReason": "end_turn",
            "usage": {
                "inputTokens": input,
                "outputTokens": output,
                "cachedReadTokens": read,
                "cachedWriteTokens": write,
                "totalTokens": input + output + read + write,
            },
            "_meta": {"quota": {
                "model_usage": [{"model": "claude-sonnet-5-5", "token_count": count}],
                "token_count": count,
            }},
        }}),
    )
}

/// A usage update, with the session's cost so far when given.
fn update(cost: Option<(f64, &str)>) -> StoredAgentSessionLog {
    let mut update = json!({"sessionUpdate": "usage_update", "used": 47410, "size": 1000000});
    if let Some((amount, currency)) = cost {
        update["cost"] = json!({"amount": amount, "currency": currency});
    }
    frame(
        "to_server",
        json!({"method": "session/update", "params": {"sessionId": "s", "update": update}}),
    )
}

fn approx(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("a cost");
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn sums_each_turns_tokens_and_takes_the_last_cost() {
    let usage = session_usage(vec![
        update(Some((0.1398, "USD"))),
        turn("a", 20, 2287, 273_477, 100),
        update(None),
        update(Some((0.1670, "USD"))),
        turn("b", 6, 1010, 75_582, 2385),
    ])
    .expect("usage");

    assert_eq!(usage.input_tokens, 26);
    assert_eq!(usage.output_tokens, 3297);
    assert_eq!(usage.cache_read_tokens, 349_059);
    assert_eq!(usage.cache_write_tokens, 2485);
    approx(usage.cost_usd, 0.1670);
    assert_eq!(usage.model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn a_cost_that_starts_over_adds_to_the_one_before() {
    // The runtime's process started over (a resumed session): its count
    // begins again from zero.
    let usage = session_usage(vec![
        update(Some((0.10, "USD"))),
        update(Some((0.30, "USD"))),
        update(Some((0.05, "USD"))),
        update(Some((0.20, "USD"))),
    ])
    .expect("usage");

    approx(usage.cost_usd, 0.50);
}

#[test]
fn a_turn_reported_twice_counts_once() {
    let usage =
        session_usage(vec![turn("a", 10, 20, 0, 0), turn("a", 10, 20, 0, 0)]).expect("usage");

    assert_eq!((usage.input_tokens, usage.output_tokens), (10, 20));
}

#[test]
fn tokens_with_no_cost_reported_have_no_cost() {
    let usage = session_usage(vec![turn("a", 10, 20, 0, 0), update(None)]).expect("usage");

    assert_eq!(usage.cost_usd, None);
    assert_eq!(usage.output_tokens, 20);
}

#[test]
fn a_cost_in_another_currency_is_not_known() {
    let usage = session_usage(vec![
        update(Some((0.10, "USD"))),
        update(Some((0.20, "EUR"))),
        turn("a", 1, 2, 0, 0),
    ])
    .expect("usage");

    assert_eq!(usage.cost_usd, None);
}

#[test]
fn a_log_that_reported_nothing_has_no_usage() {
    let prompt = frame(
        "to_runtime",
        json!({"id": "a", "method": "session/prompt", "params": {
            "sessionId": "s", "prompt": [{"type": "text", "text": "task"}]
        }}),
    );
    let ended = frame(
        "to_server",
        json!({"id": "a", "result": {"stopReason": "end_turn"}}),
    );

    assert_eq!(session_usage(vec![prompt, ended, update(None)]), None);
}
