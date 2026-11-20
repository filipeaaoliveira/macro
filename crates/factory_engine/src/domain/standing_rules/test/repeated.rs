//! Repeated Blockers: a review round that finds the round before's
//! Blockers again, unchanged, after the rule sent those to a fix, waits
//! for the Run's owner when the owner section is on.

use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, GateKind, Outcome, Role, Step, StepEnd,
};
use factory_records::domain::review_rounds::{FindingKind, Lines, ReviewFinding, ReviewRound};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, StandInKinds, StandingRule,
};

use super::{facts, fix_or_stop};
use crate::domain::review_rounds::same_blockers;
use crate::domain::standing_rules::{GateFacts, RuleAnswer, repeated_blockers, rule_answer};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

/// A Blocker the reviewer of `role` found in `path`, at `lines`.
fn blocker(seq: u16, role: &str, path: &str, lines: Option<(u32, u32)>) -> ReviewFinding {
    ReviewFinding {
        seq,
        place: 1,
        role: Some(Role::try_from(role.to_string()).expect("valid role")),
        path: path.to_string(),
        lines: lines.map(|(start, end)| Lines { start, end }),
        problem: "Drops the error".to_string(),
        why: "The caller never learns it failed".to_string(),
        blocks_anywhere: false,
        repeats_dropped: false,
        fix_status: None,
        dispute: None,
        ruling: None,
        ruled_by: None,
        kind: Some(FindingKind::Blocker),
        follow_up_task_id: None,
        follow_up_key: None,
        follow_up_commented: false,
    }
}

/// A round of one visit of its entry, from the place `seq`.
fn round(visit: u16, seq: u16, findings: Vec<ReviewFinding>) -> ReviewRound {
    ReviewRound {
        round: visit,
        visit,
        seq,
        reviewers: 2,
        head_sha: Some("aaa".to_string()),
        diff_base: Some("main".to_string()),
        fix_diff: visit > 1,
        failure: None,
        started_at: Utc::now(),
        end: None,
        findings,
        answered: Vec::new(),
    }
}

/// The first Step, at `seq`, of a round of the review panel `entry`.
fn reviewed(seq: u16, entry: &str) -> Step {
    Step {
        seq,
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: None,
        gate: None,
        role: None,
        agent: None,
        passed_over: Vec::new(),
        checks: Vec::new(),
        terminal: Default::default(),
    }
}

/// The Step at `seq` whose Blockers Gate was answered `fix`, by `rule` or
/// by a person.
fn fixed(seq: u16, rule: Option<StandingRule>) -> Step {
    Step {
        seq,
        entry: EntryId::try_from("review".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: outcome("changes"),
            output: "Blockers".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            kind: GateKind::Flow,
            question: "Fix the Blockers?".to_string(),
            answers: vec![outcome("fix"), outcome("stop")],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                answer: outcome("fix"),
                note: None,
                user_id: None,
                rule,
                approved_control_files: None,
                reply_confidence: None,
                by_stand_in: false,
                agent_session_id: None,
                answered_at: Utc::now(),
            }),
            asked_back: None,
            stand_in: None,
            stand_in_declined: None,
        }),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        checks: Vec::new(),
        terminal: Default::default(),
    }
}

const OWNER_GATES: OrchestratorDefinition = OrchestratorDefinition {
    plan: false,
    blockers: true,
    merge: false,
    owner_gates: true,
    quiet_gates: false,
    task_thread: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
    stand_in: StandInKinds {
        plan: false,
        blockers: false,
    },
};

/// With the owner section on, repeated Blockers wait for the owner; with
/// it off, the rule fixes them as before, up to its cap.
#[test]
fn repeated_blockers_go_to_the_owner_only_with_the_owner_section_on() {
    let repeated = GateFacts {
        fix_rounds: 1,
        repeated_blockers: true,
        ..facts(RuleGateKind::Blockers)
    };
    assert_eq!(
        rule_answer(Some(OWNER_GATES), &repeated, &fix_or_stop()),
        None
    );

    let off = OrchestratorDefinition {
        owner_gates: false,
        task_thread: false,
        queue_limit: None,
        token_budget: None,
        orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
        ..OWNER_GATES
    };
    assert_eq!(
        rule_answer(Some(off), &repeated, &fix_or_stop()),
        Some(RuleAnswer {
            rule: StandingRule::FixBlockers,
            answer: outcome("fix"),
        })
    );

    let changed = GateFacts {
        repeated_blockers: false,
        ..repeated
    };
    assert!(rule_answer(Some(OWNER_GATES), &changed, &fix_or_stop()).is_some());
}

/// Blockers are the same when each of either round has one in the other
/// found by a reviewer of the same Role, in the same file, at lines that
/// meet; what they say is not compared.
#[test]
fn the_same_blockers_are_the_same_role_file_and_lines() {
    let before = round(
        1,
        3,
        vec![
            blocker(3, "correctness-reviewer", "src/a.rs", Some((10, 14))),
            blocker(4, "tests-reviewer", "src/a_test.rs", None),
        ],
    );
    let mut reworded = blocker(8, "correctness-reviewer", "./src/a.rs", Some((12, 20)));
    reworded.problem = "Still drops the error".to_string();
    let again = round(
        2,
        8,
        vec![
            reworded,
            blocker(9, "tests-reviewer", "src/a_test.rs", Some((1, 2))),
        ],
    );
    assert!(same_blockers(&before, &again));

    let other_role = round(
        2,
        8,
        vec![
            blocker(8, "scope-reviewer", "src/a.rs", Some((10, 14))),
            blocker(9, "tests-reviewer", "src/a_test.rs", None),
        ],
    );
    assert!(!same_blockers(&before, &other_role));

    let moved = round(
        2,
        8,
        vec![
            blocker(8, "correctness-reviewer", "src/a.rs", Some((40, 42))),
            blocker(9, "tests-reviewer", "src/a_test.rs", None),
        ],
    );
    assert!(!same_blockers(&before, &moved));

    let one_fixed = round(
        2,
        8,
        vec![blocker(
            8,
            "correctness-reviewer",
            "src/a.rs",
            Some((10, 14)),
        )],
    );
    assert!(!same_blockers(&before, &one_fixed));

    let one_more = round(
        2,
        8,
        vec![
            blocker(8, "correctness-reviewer", "src/a.rs", Some((10, 14))),
            blocker(9, "tests-reviewer", "src/a_test.rs", None),
            blocker(9, "tests-reviewer", "src/b.rs", None),
        ],
    );
    assert!(!same_blockers(&before, &one_more));

    assert!(!same_blockers(
        &round(1, 3, Vec::new()),
        &round(2, 8, Vec::new())
    ));
}

/// Only a round whose round before went to a fix by the rule repeats it:
/// not after a person's fix, nor after a reset of its entry's visits.
#[test]
fn blockers_repeat_only_after_the_rule_sent_the_round_before_to_a_fix() {
    let found = || {
        vec![blocker(
            3,
            "correctness-reviewer",
            "src/a.rs",
            Some((10, 14)),
        )]
    };
    let rounds = [round(1, 3, found()), round(2, 8, found())];

    let by_rule = [
        reviewed(3, "review"),
        fixed(4, Some(StandingRule::FixBlockers)),
        reviewed(8, "review"),
    ];
    assert!(repeated_blockers(&rounds, &by_rule, 9));

    let by_person = [reviewed(3, "review"), fixed(4, None), reviewed(8, "review")];
    assert!(!repeated_blockers(&rounds, &by_person, 9));

    let reset = [round(1, 3, found()), round(1, 8, found())];
    assert!(!repeated_blockers(&reset, &by_rule, 9));

    let first = [round(1, 3, found())];
    assert!(!repeated_blockers(&first, &by_rule, 4));
}

/// With two review panels, a round is compared with the round before of
/// its own panel, never with another panel's round between them.
#[test]
fn blockers_are_compared_with_the_same_panel_s_round_before() {
    let x = || blocker(3, "correctness-reviewer", "src/a.rs", Some((10, 14)));
    let y = || blocker(6, "correctness-reviewer", "src/b.rs", Some((1, 2)));
    let steps = [
        reviewed(3, "review"),
        fixed(5, Some(StandingRule::FixBlockers)),
        reviewed(6, "security-review"),
        reviewed(8, "review"),
    ];

    // The panel's own round before found the same Blocker; the other
    // panel's, just before it, found another.
    let own_same = [
        round(1, 3, vec![x()]),
        round(1, 6, vec![y()]),
        round(2, 8, vec![x()]),
    ];
    assert!(repeated_blockers(&own_same, &steps, 9));

    // The other panel's round found the same Blocker; the panel's own did
    // not.
    let other_same = [
        round(1, 3, vec![y()]),
        round(1, 6, vec![x()]),
        round(2, 8, vec![x()]),
    ];
    assert!(!repeated_blockers(&other_same, &steps, 9));
}
