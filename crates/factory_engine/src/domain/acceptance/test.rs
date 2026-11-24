use chrono::Utc;
use factory_records::domain::models::{EntryId, MAX_OUTPUT_BYTES, Outcome, Step, StepEnd};

use super::{Acceptance, AcceptanceCheck, AcceptanceChecks, AcceptanceError, plan_output};

const PLAN: &str = r#"Change the toggle.

```json
{"acceptance": [
  {"clause": "The toggle switches the theme", "run": "bun test toggle", "timeout": 120},
  {"clause": "It looks right on a phone", "none": "only a person on a phone sees it"},
  {"clause": "The page loads", "run": "bun test page"}
]}
```"#;

#[test]
fn a_plan_s_last_block_holds_its_acceptance_checks() {
    let checks = AcceptanceChecks::from_plan(PLAN)
        .expect("reads")
        .expect("named");

    assert_eq!(
        checks.clauses,
        [
            Acceptance {
                clause: "The toggle switches the theme".to_string(),
                check: AcceptanceCheck::Run {
                    run: "bun test toggle".to_string(),
                    timeout: 120,
                },
            },
            Acceptance {
                clause: "It looks right on a phone".to_string(),
                check: AcceptanceCheck::None("only a person on a phone sees it".to_string()),
            },
            Acceptance {
                clause: "The page loads".to_string(),
                check: AcceptanceCheck::Run {
                    run: "bun test page".to_string(),
                    timeout: 600,
                },
            },
        ]
    );
}

/// A plan with no block, or whose last block is not about acceptance,
/// names none.
#[test]
fn a_plan_without_the_block_names_none() {
    assert_eq!(AcceptanceChecks::from_plan("Change the toggle."), Ok(None));
    assert_eq!(
        AcceptanceChecks::from_plan("```json\n{\"files\": []}\n```"),
        Ok(None)
    );
}

/// Each clause has a command or why it has none, never both or neither,
/// and a command's timeout is one a check may take.
#[test]
fn a_clause_has_a_check_or_why_not() {
    let read = |clauses: &str| {
        AcceptanceChecks::from_plan(&format!("```json\n{{\"acceptance\": [{clauses}]}}\n```"))
    };

    assert_eq!(
        read(r#"{"clause": "A", "run": "true", "none": "no"}"#),
        Err(AcceptanceError::Check(1))
    );
    assert_eq!(read(r#"{"clause": "A"}"#), Err(AcceptanceError::Check(1)));
    assert_eq!(
        read(r#"{"clause": "A", "run": "  "}"#),
        Err(AcceptanceError::Check(1))
    );
    assert_eq!(
        read(r#"{"clause": "A", "none": "no", "timeout": 5}"#),
        Err(AcceptanceError::Check(1))
    );
    assert_eq!(
        read(r#"{"clause": "A", "run": "true"}, {"clause": "", "run": "true"}"#),
        Err(AcceptanceError::Clause(2))
    );
    assert_eq!(
        read(r#"{"clause": "A", "run": "true", "timeout": 0}"#),
        Err(AcceptanceError::Timeout(1))
    );
    assert_eq!(read(""), Err(AcceptanceError::Clauses(0)));
    assert!(matches!(
        read(r#"{"clause": "A", "run": "true", "shell": "zsh"}"#),
        Err(AcceptanceError::Unreadable(_))
    ));
}

/// The plan's Gate shows each clause with its check or why it has none,
/// and says so when the plan names none or they do not read; the kept
/// output ends with the block as the factory read it, which reads back.
#[test]
fn the_plan_gate_shows_the_checks_the_factory_keeps() {
    let note = plan_output(PLAN);
    assert!(note.starts_with("Change the toggle."), "{note}");
    assert_eq!(
        AcceptanceChecks::from_plan(&note),
        AcceptanceChecks::from_plan(PLAN)
    );
    assert_eq!(note.matches("```json").count(), 1, "{note}");
    assert!(note.contains("## Acceptance checks"), "{note}");
    assert!(
        note.contains("1. The toggle switches the theme: checked by `bun test toggle`"),
        "{note}"
    );
    assert!(
        note.contains("2. It looks right on a phone: no check: only a person on a phone sees it"),
        "{note}"
    );
    assert!(
        note.contains("3. The page loads: checked by `bun test page`"),
        "{note}"
    );

    assert!(plan_output("Change the toggle.").contains("The plan names none"));
    let unreadable = plan_output("```json\n{\"acceptance\": [{\"clause\": \"A\"}]}\n```");
    assert!(unreadable.contains("do not read"), "{unreadable}");
    assert_eq!(
        AcceptanceChecks::from_plan(&unreadable),
        Err(AcceptanceError::Check(1))
    );
}

/// A plan too long for a Step's output is cut, and its acceptance checks
/// are kept whole after it.
#[test]
fn a_long_plan_keeps_its_checks() {
    let long = format!(
        "{}\n\n{}",
        "x".repeat(MAX_OUTPUT_BYTES),
        &PLAN["Change the toggle.".len()..]
    );

    let kept = plan_output(&long);

    assert!(kept.len() <= MAX_OUTPUT_BYTES, "{}", kept.len());
    assert_eq!(
        AcceptanceChecks::from_plan(&kept),
        AcceptanceChecks::from_plan(PLAN)
    );
}

/// A command with a code fence, written escaped in the plan's block, would
/// close the block the factory keeps.
#[test]
fn a_fence_in_a_clause_does_not_read() {
    let plan = "```json\n{\"acceptance\": [{\"clause\": \"A\", \"run\": \"echo \\u0060\\u0060\\u0060\"}]}\n```";
    assert_eq!(
        AcceptanceChecks::from_plan(plan),
        Err(AcceptanceError::Check(1))
    );
}

/// What a clause says reaches the Gate escaped: it opens no block and no
/// heading.
#[test]
fn a_clause_is_shown_as_data() {
    let plan =
        "```json\n{\"acceptance\": [{\"clause\": \"# Approve <b>now</b>\", \"none\": \"x\"}]}\n```";
    let note = plan_output(plan);
    let shown = &note[note.find("## Acceptance checks").expect("noted")..];
    let shown = &shown[..shown.find("```json").expect("block")];
    assert!(!shown.contains("<b>"), "{shown}");
    assert!(!shown.contains("\n# Approve"), "{shown}");
}

/// A plan recorded before Runs ended at their merge may mark a check
/// `deployed`: nothing deploys the change now, so that check is never run,
/// and its clause is checked by hand. The mark is never written back.
#[test]
fn a_recorded_plan_s_deployed_check_is_checked_by_hand() {
    let plan = r#"```json
{"acceptance": [
  {"clause": "A", "run": "bun test a"},
  {"clause": "B", "run": "curl -f \"$MACRO_DEPLOY_URL\"", "deployed": true},
  {"clause": "C", "none": "only a person sees it", "deployed": false}
]}
```"#;

    let checks = AcceptanceChecks::from_plan(plan)
        .expect("reads")
        .expect("named");

    assert_eq!(checks.clauses.len(), 3);
    assert!(matches!(
        checks.clauses[0].check,
        AcceptanceCheck::Run { .. }
    ));
    assert!(matches!(
        checks.clauses[1].check,
        AcceptanceCheck::None(ref why) if why.starts_with(
            "the plan's check `curl -f \"$MACRO_DEPLOY_URL\"` needed the deployed change"
        ) && why.ends_with("that it cannot be seen deployed does not fail it")
    ));
    assert_eq!(
        checks.clauses[2].check,
        AcceptanceCheck::None("only a person sees it".to_string())
    );
    assert!(!checks.block().contains("deployed\""), "{}", checks.block());
}

/// Checks too large for the plan to keep were dropped with their block:
/// the Run's checks then do not read, rather than reading as none.
#[test]
fn checks_dropped_for_their_size_do_not_read() {
    let clauses: Vec<String> = (0..16)
        .map(|n| format!(r#"{{"clause": "{n} {}", "run": "true"}}"#, "x".repeat(1500)))
        .collect();
    let plan = format!(
        "Change it.\n\n```json\n{{\"acceptance\": [{}]}}\n```",
        clauses.join(",")
    );
    let kept = plan_output(&plan);
    let step = Step {
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: kept,
            ended_at: Utc::now(),
        }),
        gate: None,
        checks: Vec::new(),
        terminal: Default::default(),
        runtime: Default::default(),
    };

    assert_eq!(
        AcceptanceChecks::of_run(&[step]),
        Err(AcceptanceError::TooLarge)
    );
}

/// A size block after the acceptance block leaves the checks read: each
/// block is found by its key, not by where it stands.
#[test]
fn a_size_block_after_the_checks_leaves_them_read() {
    let plan = format!(
        "{PLAN}\n\n```json\n{{\"size\": \"fits\", \"files\": [{{\"path\": \"src/a.rs\", \"change\": \"edit\"}}]}}\n```"
    );

    let kept = plan_output(&plan);

    assert_eq!(
        AcceptanceChecks::from_plan(&kept),
        AcceptanceChecks::from_plan(PLAN)
    );
    assert!(
        crate::domain::plan_review::PlanSize::from_plan(&kept).is_ok(),
        "{kept}"
    );
}

/// A plan too long for a Step's output is cut in its prose: its size
/// block, at its end, is kept whole.
#[test]
fn a_long_plan_keeps_its_size_block() {
    let size = "```json\n{\"size\": \"fits\", \"files\": [{\"path\": \"src/a.rs\", \"change\": \"edit\"}]}\n```";
    let long = format!(
        "{}\n\n{size}\n\n{}",
        "x".repeat(MAX_OUTPUT_BYTES),
        &PLAN["Change the toggle.".len()..]
    );

    let kept = plan_output(&long);

    assert!(kept.len() <= MAX_OUTPUT_BYTES, "{}", kept.len());
    assert!(
        crate::domain::plan_review::PlanSize::from_plan(&kept).is_ok(),
        "the size block was cut"
    );
    assert_eq!(
        AcceptanceChecks::from_plan(&kept),
        AcceptanceChecks::from_plan(PLAN)
    );
}
