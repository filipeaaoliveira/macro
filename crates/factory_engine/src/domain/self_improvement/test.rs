use chrono::{TimeDelta, Utc};
use factory_records::domain::models::RunId;
use factory_records::domain::self_improvement::{
    Failure, FailureSource, Improvement, ImprovementState,
};
use uuid::Uuid;

use super::{
    MAX_UNMERGED_PAUSE, UNMERGED_PAUSE, WORD_JOINER, improvement_request, is_due, paused_until,
};
use crate::domain::evaluation::{EvaluationDefinition, SelfImprovementDefinition};

fn failure(source: FailureSource, reason: &str) -> Failure {
    Failure {
        run_id: RunId::generate(),
        flow: "code".to_string(),
        source,
        reason: reason.to_string(),
        failed_at: Utc::now(),
    }
}

fn scorer(name: &str, score: f64) -> FailureSource {
    FailureSource::Scorer {
        name: name.to_string(),
        score,
    }
}

/// The request names how many runs failed on its first line (the pull
/// request's title), then lists them under "Regressions addressed", by
/// Scorer and then the flawed Runs, each linking its Run, with the
/// Scorers' judgments linked too.
#[test]
fn the_request_lists_the_regressions_it_addresses() {
    let failures = vec![
        failure(scorer("tests", 0.2), "No test covers the change"),
        failure(FailureSource::Flawed, "The button is the wrong colour"),
        failure(scorer("tests", 0.1), "Tests were\n## deleted"),
    ];
    let request = improvement_request(
        &failures,
        |run_id| format!("https://app.example/runs/{run_id}"),
        "https://app.example/insights",
    );

    let first = request.lines().next().expect("a first line");
    assert_eq!(
        first,
        "Improve the factory definition to address 3 failed runs"
    );
    let section = request
        .split_once("## Regressions addressed\n")
        .expect("the section")
        .1;
    assert!(section.contains("### Scorer `tests` (2, [judgments](https://app.example/insights))"));
    assert!(section.contains("### Runs marked flawed (1)"));
    for failure in &failures {
        assert!(
            section.contains(&format!(
                "[Run {}](https://app.example/runs/{})",
                failure.run_id, failure.run_id
            )),
            "{}",
            failure.run_id
        );
    }
    assert!(section.contains("scored 0.20: > No test covers the change"));
    // A reason's lines are joined, so it never opens a section.
    assert!(section.contains(&format!(
        "> Tests were #{WORD_JOINER}#{WORD_JOINER} deleted"
    )));
    assert!(!section.contains("\n## deleted"));
}

/// A team's failures are due once they reach the count, or once the
/// oldest is old enough, and never while the Definition turns it off or
/// none is pending.
#[test]
fn failures_are_due_at_their_count_or_age() {
    let now = Utc::now();
    let thresholds = SelfImprovementDefinition {
        enabled: true,
        failures: 3,
        oldest_days: 7,
    };
    let fresh = Some(now - TimeDelta::hours(1));
    let old = Some(now - TimeDelta::days(7));

    assert!(!is_due(thresholds, 2, fresh, now));
    assert!(is_due(thresholds, 3, fresh, now));
    assert!(is_due(thresholds, 1, old, now));
    assert!(!is_due(thresholds, 0, old, now));
    let off = SelfImprovementDefinition {
        enabled: false,
        ..thresholds
    };
    assert!(!is_due(off, 30, old, now));
}

/// A Definition's `scorers.json` sets the thresholds; left out, they are
/// the safe defaults (on, 25 failures, 7 days), and out of range, the file
/// does not read.
#[test]
fn the_thresholds_read_from_scorers_json_with_defaults() {
    let defaults: EvaluationDefinition =
        serde_json::from_str(r#"{ "scorers": [] }"#).expect("reads");
    assert_eq!(
        defaults.self_improvement(),
        SelfImprovementDefinition {
            enabled: true,
            failures: 25,
            oldest_days: 7,
        }
    );
    // A Run keeping the defaults keeps the JSON it kept before.
    assert_eq!(
        serde_json::to_value(&defaults).expect("writes"),
        serde_json::json!({ "scorers": [], "experiments": [], "benchmarks": [] })
    );

    let set: EvaluationDefinition =
        serde_json::from_str(r#"{ "selfImprovement": { "failures": 5, "enabled": false } }"#)
            .expect("reads");
    assert_eq!(
        set.self_improvement(),
        SelfImprovementDefinition {
            enabled: false,
            failures: 5,
            oldest_days: 7,
        }
    );
    let kept: EvaluationDefinition =
        serde_json::from_value(serde_json::to_value(&set).expect("writes")).expect("reads back");
    assert_eq!(kept, set);

    for bad in [
        r#"{ "selfImprovement": { "failures": 0 } }"#,
        r#"{ "selfImprovement": { "oldestDays": 400 } }"#,
        r#"{ "selfImprovement": { "every": 3 } }"#,
    ] {
        assert!(
            serde_json::from_str::<EvaluationDefinition>(bad).is_err(),
            "{bad}"
        );
    }
}

/// A reason quoted into the pull request's description mentions no one
/// and links no issue: each `@` and `#` is followed by a word joiner.
#[test]
fn a_reason_mentions_no_one_and_links_no_issue() {
    let failures = vec![failure(
        scorer("tests", 0.2),
        "Ask @octocat about #123 and owner/repo#4",
    )];
    let request = improvement_request(&failures, |_| String::new(), "");

    assert!(!request.contains("@octocat"), "{request}");
    assert!(!request.contains("#123"), "{request}");
    assert!(!request.contains("repo#4"), "{request}");
    assert!(request.contains(&format!("@{WORD_JOINER}octocat about #{WORD_JOINER}123")));
}

fn settled(state: ImprovementState, hours_ago: i64) -> Improvement {
    let settled_at = Utc::now() - TimeDelta::hours(hours_ago);
    Improvement {
        id: Uuid::now_v7(),
        team_id: Uuid::nil(),
        repo_url: "https://github.com/macro/factory-definition".to_string(),
        run_id: None,
        state,
        refusal: None,
        created_at: settled_at,
        settled_at: Some(settled_at),
        failures: 1,
    }
}

/// After a self-improvement Run that did not merge (closed, cancelled,
/// refused), the next waits a day; each more in a row since the last that
/// merged doubles the wait, up to a week; a merged one ends it.
#[test]
fn the_pause_doubles_after_each_unmerged_improvement_and_a_merge_resets_it() {
    use ImprovementState::{Closed, Merged, Refused};
    let until = |recent: &[Improvement]| {
        paused_until(recent).map(|until| until - recent[0].settled_at.expect("settled"))
    };

    assert_eq!(paused_until(&[]), None);
    assert_eq!(until(&[settled(Merged, 1)]), None);
    assert_eq!(until(&[settled(Closed, 1)]), Some(UNMERGED_PAUSE));
    assert_eq!(
        until(&[settled(Refused, 1), settled(Closed, 30)]),
        Some(UNMERGED_PAUSE * 2)
    );
    assert_eq!(
        until(&[
            settled(Closed, 1),
            settled(Closed, 50),
            settled(Refused, 100)
        ]),
        Some(UNMERGED_PAUSE * 4)
    );
    let many: Vec<Improvement> = (0..8).map(|n| settled(Closed, 1 + n * 200)).collect();
    assert_eq!(until(&many), Some(MAX_UNMERGED_PAUSE));
    // A merge ends the streak: only the unmerged ones after it count.
    assert_eq!(
        until(&[settled(Closed, 1), settled(Merged, 30), settled(Closed, 60)]),
        Some(UNMERGED_PAUSE)
    );
}
