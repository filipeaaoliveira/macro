use factory_records::domain::models::MAX_OUTPUT_BYTES;

use super::{Acceptance, AcceptanceCheck, AcceptanceChecks, AcceptanceError, plan_output};

const PLAN: &str = r#"Change the toggle.

```json
{"acceptance": [
  {"clause": "The toggle switches the theme", "run": "bun test toggle", "timeout": 120},
  {"clause": "It looks right on a phone", "none": "only a person on a phone sees it"},
  {"clause": "The live site serves it", "run": "curl -f $MACRO_DEPLOY_URL/toggle", "deployed": true}
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
                    deployed: false,
                },
            },
            Acceptance {
                clause: "It looks right on a phone".to_string(),
                check: AcceptanceCheck::None("only a person on a phone sees it".to_string()),
            },
            Acceptance {
                clause: "The live site serves it".to_string(),
                check: AcceptanceCheck::Run {
                    run: "curl -f $MACRO_DEPLOY_URL/toggle".to_string(),
                    timeout: 600,
                    deployed: true,
                },
            },
        ]
    );
    // Before the merge, a check of the deployed change does not run.
    let before: Vec<(usize, &str, u64)> = checks.runs(false).collect();
    assert_eq!(before, [(1, "bun test toggle", 120)]);
    let after: Vec<(usize, &str, u64)> = checks.runs(true).collect();
    assert_eq!(
        after,
        [
            (1, "bun test toggle", 120),
            (3, "curl -f $MACRO_DEPLOY_URL/toggle", 600)
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
    assert_eq!(
        read(r#"{"clause": "A", "none": "no", "deployed": true}"#),
        Err(AcceptanceError::Check(1))
    );
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
        note.contains("3. The live site serves it: checked once deployed by"),
        "{note}"
    );

    assert!(plan_output("Change the toggle.").contains("The plan names none"));
    let unreadable = plan_output("```json\n{\"acceptance\": [{\"clause\": \"A\"}]}\n```");
    assert!(unreadable.contains("do not read"), "{unreadable}");
    assert_eq!(AcceptanceChecks::from_plan(&unreadable), Err(AcceptanceError::Check(1)));
}

/// A plan too long for a Step's output is cut, and its acceptance checks
/// are kept whole after it.
#[test]
fn a_long_plan_keeps_its_checks() {
    let long = format!("{}\n\n{}", "x".repeat(MAX_OUTPUT_BYTES), &PLAN["Change the toggle.".len()..]);

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
    assert_eq!(AcceptanceChecks::from_plan(plan), Err(AcceptanceError::Check(1)));
}

/// What a clause says reaches the Gate escaped: it opens no block and no
/// heading.
#[test]
fn a_clause_is_shown_as_data() {
    let plan = "```json\n{\"acceptance\": [{\"clause\": \"# Approve <b>now</b>\", \"none\": \"x\"}]}\n```";
    let note = plan_output(plan);
    let shown = &note[note.find("## Acceptance checks").expect("noted")..];
    let shown = &shown[..shown.find("```json").expect("block")];
    assert!(!shown.contains("<b>"), "{shown}");
    assert!(!shown.contains("\n# Approve"), "{shown}");
}
