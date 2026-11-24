//! What a session's runtime reported its model calls used, read from the
//! frames it sent: each turn's prompt response carries that turn's token
//! counts, and its usage updates carry the session's cost so far.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use super::models::RoutineUsage;
use crate::domain::model::StoredAgentSessionLog;

/// The currency a cost is summed in.
const USD: &str = "USD";

/// Sum what `entries`, a session's frames oldest first, report it used. A
/// prompt response's `usage` counts its own turn, once per request id. A
/// usage update's `cost` is the session's total so far, as the runtime's
/// process counts it: a process that starts over (a resumed session) counts
/// from zero again, so a total lower than the one before starts a new
/// count, and the session's cost is the sum of each count's last total.
/// None when no frame reported any usage.
pub(super) fn session_usage(entries: Vec<StoredAgentSessionLog>) -> Option<RoutineUsage> {
    let mut usage = RoutineUsage::default();
    let mut turns = 0_u32;
    let mut counted: HashSet<String> = HashSet::new();
    let mut by_model: BTreeMap<String, u64> = BTreeMap::new();
    // The cost of the counts before the current one, and the current one's
    // latest total.
    let mut banked = 0.0_f64;
    let mut current: Option<f64> = None;
    let mut other_currency = false;

    for row in entries {
        let Ok(frame) = serde_json::to_value(&row.entry.content) else {
            continue;
        };
        if frame.get("direction").and_then(Value::as_str) != Some("to_server") {
            continue;
        }
        let Some(content) = frame.get("content") else {
            continue;
        };
        if let Some(reported) = content.pointer("/result/usage") {
            let id = content.get("id").map(Value::to_string).unwrap_or_default();
            if !counted.insert(id) {
                continue;
            }
            let count = |key: &str| reported.get(key).and_then(Value::as_u64).unwrap_or(0);
            turns += 1;
            usage.input_tokens = usage.input_tokens.saturating_add(count("inputTokens"));
            usage.output_tokens = usage.output_tokens.saturating_add(count("outputTokens"));
            usage.cache_read_tokens = usage
                .cache_read_tokens
                .saturating_add(count("cachedReadTokens"));
            usage.cache_write_tokens = usage
                .cache_write_tokens
                .saturating_add(count("cachedWriteTokens"));
            let models = content
                .pointer("/result/_meta/quota/model_usage")
                .and_then(Value::as_array);
            for model in models.into_iter().flatten() {
                let (Some(name), Some(tokens)) = (
                    model.get("model").and_then(Value::as_str),
                    model
                        .pointer("/token_count/totalTokens")
                        .and_then(Value::as_u64),
                ) else {
                    continue;
                };
                let total = by_model.entry(name.to_string()).or_default();
                *total = total.saturating_add(tokens);
            }
            continue;
        }
        if content.get("method").and_then(Value::as_str) != Some("session/update") {
            continue;
        }
        let Some(update) = content.pointer("/params/update") else {
            continue;
        };
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("usage_update") {
            continue;
        }
        let Some(cost) = update.get("cost").filter(|cost| !cost.is_null()) else {
            continue;
        };
        let amount = cost.get("amount").and_then(Value::as_f64);
        let currency = cost.get("currency").and_then(Value::as_str);
        match (amount, currency) {
            (Some(amount), Some(USD)) if amount.is_finite() && amount >= 0.0 => {
                if let Some(previous) = current
                    && amount < previous
                {
                    banked += previous;
                }
                current = Some(amount);
            }
            _ => other_currency = true,
        }
    }

    if turns == 0 && current.is_none() {
        return None;
    }
    usage.cost_usd = current
        .filter(|_| !other_currency)
        .map(|current| banked + current);
    usage.model = by_model
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(model, _)| model);
    Some(usage)
}
